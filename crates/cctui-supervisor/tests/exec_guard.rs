//! Kernel-level exec interception: cctui-guard + cctui-supervisor `--guard-exec`.
//!
//! Needs root with `CAP_SYS_PTRACE`, Landlock, kernel >= 5.9, `git` on PATH and
//! a built `cctui-guard` next to this crate's binary (or `CCTUI_GUARD_BIN`):
//!
//! ```text
//! cargo build -p cctui-guard -p cctui-supervisor
//! sudo -E cargo test -p cctui-supervisor --test exec_guard -- --ignored --nocapture
//! ```

#![cfg(target_os = "linux")]

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROMPT: &str = "\
# Step 1: Work
[allowed]: Read
[disallowed]: git push
[transition]: Exit
";

fn supervisor() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cctui-supervisor"))
}

fn guard_bin() -> PathBuf {
    std::env::var_os("CCTUI_GUARD_BIN")
        .map_or_else(|| supervisor().with_file_name("cctui-guard"), PathBuf::from)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start_guard(dir: &Path, port: u16) -> Child {
    fs::write(dir.join("prompt.md"), PROMPT).unwrap();
    fs::write(dir.join("rules.md"), "").unwrap();
    let socket = dir.join("exec.sock");
    let child = Command::new(guard_bin())
        .arg("--prompt")
        .arg(dir.join("prompt.md"))
        .arg("--rules")
        .arg(dir.join("rules.md"))
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .arg("--state")
        .arg(dir.join("state"))
        .arg("--policy-out")
        .arg(dir.join("policy.json"))
        .arg("--gate-cwd")
        .arg(dir)
        .arg("--decision-log")
        .arg(dir.join("decisions.jsonl"))
        .arg("--exec-socket")
        .arg(&socket)
        .spawn()
        .expect("spawn cctui-guard (build it first, or set CCTUI_GUARD_BIN)");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "guard never bound {}", socket.display());
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

fn supervised(dir: &Path, script: &str) -> Command {
    supervised_with(dir, script, &[])
}

fn supervised_with(dir: &Path, script: &str, extra: &[&str]) -> Command {
    let guard_dir = guard_bin().parent().unwrap().to_path_buf();
    let mut cmd = Command::new(supervisor());
    for ro in ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc"] {
        cmd.args(["--ro", ro]);
    }
    cmd.arg("--ro").arg(&guard_dir);
    cmd.args(["--rw", "/dev"]).arg("--rw").arg(dir);
    cmd.args(["--no-privdrop", "--guard-exec"]).arg(dir.join("exec.sock"));
    cmd.args(extra);
    cmd.args(["--", "/bin/sh", "-c", script]);
    cmd
}

fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap_or_default()
}

fn rc(dir: &Path, name: &str) -> i32 {
    read(dir, "rc")
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{name} ")))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no exit code recorded for {name}"))
}

#[test]
#[ignore = "needs root + CAP_SYS_PTRACE + Landlock + git; see module docs"]
fn denied_capability_is_blocked_however_it_is_invoked() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let port = free_port();
    let mut guard = start_guard(d, port);
    let guard_path = guard_bin();
    let script = format!(
        r#"
D={d}
run() {{ name=$1; shift; "$@" >"$D/$name.out" 2>"$D/$name.err"; echo "$name $?" >> "$D/rc"; }}
run direct git push
run absolute /usr/bin/git push
run pathvar sh -c 'PATH=/usr/bin git push'
run shc sh -c 'git push'
run shsplit sh -c 'a=pu; git ${{a}}sh'
run envi env -i git push
if command -v python3 >/dev/null; then
  run python python3 -c 'import subprocess,sys; sys.exit(subprocess.run(["git","push"]).returncode)'
fi
if command -v bash >/dev/null; then
  run disguise bash -c 'p=pu; exec -a harmless /usr/bin/git ${{p}}sh'
fi
run allowed git --version
cp /bin/true "$D/selfbuilt" && chmod +x "$D/selfbuilt"
run noexec "$D/selfbuilt"
git push 2>/dev/null
run why {guard} why --guard 127.0.0.1:{port}
"#,
        d = d.display(),
        guard = guard_path.display(),
    );
    let out = supervised(d, &script).output().unwrap();
    let _ = guard.kill();
    let _ = guard.wait();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "supervised run failed: {stderr}");

    let mut cases = vec!["direct", "absolute", "pathvar", "shc", "shsplit", "envi"];
    if read(d, "rc").contains("python ") {
        cases.push("python");
    }
    if read(d, "rc").contains("disguise ") {
        cases.push("disguise");
    }
    for case in cases {
        assert_ne!(rc(d, case), 0, "{case} must fail");
        let err = read(d, &format!("{case}.err"));
        assert!(err.contains("guard: denied"), "{case}: reason missing from stderr: {err:?}");
    }
    assert_eq!(rc(d, "allowed"), 0, "git --version: {}", read(d, "allowed.err"));
    assert_eq!(rc(d, "noexec"), 126, "RW paths must be noexec: {}", read(d, "noexec.err"));
    assert_eq!(rc(d, "why"), 0, "guard why: {}", read(d, "why.err"));
    assert!(read(d, "why.out").contains("git push"), "why: {}", read(d, "why.out"));

    let log = read(d, "decisions.jsonl");
    assert!(
        log.lines().any(|l| l.contains(r#""source":"exec""#) && l.contains(r#""verdict":"deny""#)),
        "exec denials must be logged: {log}"
    );
}

#[test]
#[ignore = "needs root + CAP_SYS_PTRACE + Landlock; see module docs"]
fn every_exec_fails_once_the_guard_dies() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let mut guard = start_guard(d, free_port());
    let script = format!(r#"sleep 2; /bin/true; echo "rc=$?" > {d}/death"#, d = d.display());
    let run = supervised(d, &script).stderr(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(700));
    guard.kill().unwrap();
    let _ = guard.wait();
    let out = run.wait_with_output().unwrap();
    let death = read(d, "death");
    assert!(
        death.starts_with("rc=") && death.trim() != "rc=0",
        "exec after guard death must fail: {death:?} / {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[ignore = "needs root + CAP_SYS_PTRACE + Landlock; see module docs"]
fn only_the_exec_rw_subtrees_of_rw_paths_execute() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let mut guard = start_guard(d, free_port());
    let bin = d.join("tools").join("bin");
    let script = format!(
        r#"
D={d}
cp /bin/true "$D/tools/bin/ok" && chmod +x "$D/tools/bin/ok"
cp /bin/true "$D/elsewhere" && chmod +x "$D/elsewhere"
"$D/tools/bin/ok"; echo "inside $?" >> "$D/rc"
"$D/elsewhere"; echo "outside $?" >> "$D/rc"
"#,
        d = d.display()
    );
    let out = supervised_with(d, &script, &["--exec-rw", bin.to_str().unwrap()]).output().unwrap();
    let _ = guard.kill();
    let _ = guard.wait();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "supervised run failed: {stderr}");
    assert!(bin.is_dir(), "the supervisor creates a missing exec subtree");
    assert_eq!(rc(d, "inside"), 0, "an exec-rw subtree executes: {stderr}");
    assert_eq!(rc(d, "outside"), 126, "the rest of the RW path stays noexec");
}
