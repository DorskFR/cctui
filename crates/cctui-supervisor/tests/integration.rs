//! Integration tests for cctui-supervisor.
//!
//! Tests run the compiled binary as a child. Privilege-drop is skipped
//! (`--no-privdrop`) because CI does not run as root. Kernel-dependent
//! assertions (Landlock, seccomp) detect support at runtime and skip with a
//! printed note rather than `#[ignore]`, per the ticket.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Path to the built `cctui-supervisor` binary (cargo sets `CARGO_BIN_EXE_*`).
const fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_cctui-supervisor")
}

/// Best-effort runtime detection of Landlock support via the kernel LSM list.
fn landlock_available() -> bool {
    fs::read_to_string("/sys/kernel/security/lsm")
        .is_ok_and(|s| s.split(',').any(|l| l.trim() == "landlock"))
}

#[test]
fn dry_run_lists_rules_and_seccomp() {
    let out = Command::new(bin())
        .args(["--ro", "/usr", "--ro", "/lib", "--rw", "/tmp", "--dry-run", "--", "echo", "hello"])
        .output()
        .expect("run supervisor");
    assert!(out.status.success(), "dry-run should succeed");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("/usr"), "expected /usr: {s}");
    assert!(s.contains("/lib"), "expected /lib: {s}");
    assert!(s.contains("/tmp"), "expected /tmp: {s}");
    assert!(s.contains("echo"), "expected echo: {s}");
    assert!(s.contains("Seccomp: enabled"), "expected seccomp enabled: {s}");
    assert!(s.contains("ptrace"), "expected ptrace in blocked list: {s}");
    assert!(s.contains("unshare"), "expected unshare in blocked list: {s}");
}

#[test]
fn dry_run_no_seccomp_hides_syscalls() {
    let out = Command::new(bin())
        .args(["--ro", "/usr", "--no-seccomp", "--dry-run", "--", "echo", "hi"])
        .output()
        .expect("run supervisor");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Seccomp: disabled"), "expected disabled: {s}");
    assert!(!s.contains("ptrace"), "expected no syscall list: {s}");
}

#[test]
fn missing_command_fails() {
    // No command and not --dry-run => clap rejects (required_unless_present).
    let status = Command::new(bin()).status().expect("run supervisor");
    assert!(!status.success(), "expected nonzero exit with no command");
}

#[test]
fn runs_command_without_privdrop() {
    // No privdrop (not root), no seccomp, generous landlock; just exec `true`.
    let status = Command::new(bin())
        .args(["--no-privdrop", "--no-seccomp", "--rw", "/", "--", "true"])
        .status()
        .expect("run supervisor");
    assert!(status.success(), "exec true should succeed");
}

#[test]
fn seccomp_denies_syscall_with_eperm() {
    // Deterministic, privilege-independent: the self-test installs the filter
    // then calls setuid(geteuid()) — a no-op that succeeds for any user when
    // allowed, so an EPERM result proves the denylist is active. Exits 0 on
    // EPERM, 2 otherwise.
    let out = Command::new(bin())
        .env("CCTUI_SUPERVISOR_SELFTEST_SECCOMP", "1")
        .output()
        .expect("run self-test");
    assert!(
        out.status.success(),
        "seccomp self-test should report EPERM (exit 0); got {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn landlock_ro_blocks_write_rw_allows_write() {
    if !landlock_available() {
        eprintln!("SKIP landlock test: kernel reports no landlock LSM");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("cctui-sup-test-{}", std::process::id()));
    let readonly_dir = tmp.join("ro");
    let writable_dir = tmp.join("rw");
    fs::create_dir_all(&readonly_dir).unwrap();
    fs::create_dir_all(&writable_dir).unwrap();

    // The payload needs /usr + /bin etc. read access to even exec sh; grant
    // the whole fs read-only, the readonly_dir read-only (redundant but explicit),
    // and only writable_dir read-write.
    let run_write = |dir: &Path, file: &str| -> bool {
        Command::new(bin())
            .args(["--no-privdrop", "--no-seccomp"])
            .args(["--ro", "/"])
            .arg("--ro")
            .arg(&readonly_dir)
            .arg("--rw")
            .arg(&writable_dir)
            .arg("--")
            .args(["sh", "-c", &format!("echo data > {}/{}", dir.display(), file)])
            .status()
            .expect("run write")
            .success()
    };

    let rw_ok = run_write(&writable_dir, "ok.txt");
    let ro_blocked = !run_write(&readonly_dir, "denied.txt");

    let landlock_enforced = rw_ok && ro_blocked;
    if !landlock_enforced {
        // Kernel advertises landlock but the ABI we target is not enforced
        // (e.g. older kernel): runtime-skip rather than fail.
        if !ro_blocked && !rw_ok {
            eprintln!("SKIP landlock test: neither write behaved as expected (ABI unenforced)");
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
    }
    let _ = fs::remove_dir_all(&tmp);

    assert!(rw_ok, "write to RW path should succeed");
    assert!(ro_blocked, "write to RO path should be blocked by landlock");
}

#[test]
fn make_runs_a_recipe_under_full_sandbox() {
    // Regression for GNU Make's recipe-spawn child resets its
    // effective uid (`setresuid(-1, <uid>, -1)`) before exec. The seccomp
    // denylist used to turn that no-op into EPERM, so `make` aborted every
    // recipe with `/bin/sh: Operation not permitted` (exit 127). With the
    // conditional uid/gid guard, a no-op reset to the worker id is allowed, so
    // a trivial Makefile target must run to completion under the full sandbox.
    if Command::new("make").arg("--version").output().is_err() {
        eprintln!("SKIP make test: `make` not found on PATH");
        return;
    }
    if !landlock_available() {
        eprintln!("SKIP make test: kernel reports no landlock LSM");
        return;
    }

    let dir = std::env::temp_dir().join(format!("cctui-sup-make-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Makefile"), "all:\n\t@echo hello-from-make\n").unwrap();

    // Seccomp ON (default) + Landlock (rw=/ so the recipe shell + coreutils are
    // reachable). Privdrop is skipped (CI is not root), so the worker id the
    // seccomp guard must recognise is this process's own uid — pass it via
    // `--user` so make's `setresuid(-1, <our uid>, -1)` is treated as the
    // allowed no-op.
    let uid = nix::unistd::getuid().as_raw();
    let out = Command::new(bin())
        .args(["--no-privdrop", "--rw", "/", "--user"])
        .arg(uid.to_string())
        .arg("--")
        .args(["make", "-C"])
        .arg(&dir)
        .arg("all")
        .output()
        .expect("run make under supervisor");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = fs::remove_dir_all(&dir);

    assert!(
        out.status.success(),
        "make should succeed under the sandbox; status {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(stdout.contains("hello-from-make"), "recipe output missing: {stdout}");
}

#[test]
fn report_json_has_expected_shape() {
    let tmp = std::env::temp_dir().join(format!("cctui-sup-report-{}.json", std::process::id()));
    let _ = fs::remove_file(&tmp);

    let status = Command::new(bin())
        .args(["--no-privdrop", "--no-seccomp", "--rw", "/", "--user", "1234"])
        .arg("--report")
        .arg(&tmp)
        .args(["--", "true"])
        .status()
        .expect("run supervisor");
    assert!(status.success(), "supervisor with --report should succeed");

    let raw = fs::read_to_string(&tmp).expect("report file written");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    let _ = fs::remove_file(&tmp);

    assert!(v.get("landlock").and_then(|x| x.as_str()).is_some(), "landlock str: {v}");
    assert_eq!(v["seccomp_applied"].as_bool(), Some(false), "seccomp disabled in this run");
    assert!(v["seccomp_blocked"].is_array(), "seccomp_blocked array: {v}");
    assert_eq!(v["caps_dropped"].as_bool(), Some(false), "privdrop skipped");
    assert_eq!(v["uid"].as_u64(), Some(1234), "uid forwarded");
    assert!(v["ro_paths"].is_array(), "ro_paths array");
    assert!(v["rw_paths"].is_array(), "rw_paths array");
    assert_eq!(v["command"].as_array().map(Vec::len), Some(1), "command is [\"true\"]: {v}");
}

#[test]
fn report_records_seccomp_when_enabled() {
    let tmp = std::env::temp_dir().join(format!("cctui-sup-report2-{}.json", std::process::id()));
    let _ = fs::remove_file(&tmp);

    let status = Command::new(bin())
        .args(["--no-privdrop", "--rw", "/"])
        .arg("--report")
        .arg(&tmp)
        .args(["--", "true"])
        .status()
        .expect("run supervisor");
    assert!(status.success());

    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&tmp).unwrap()).unwrap();
    let _ = fs::remove_file(&tmp);

    assert_eq!(v["seccomp_applied"].as_bool(), Some(true));
    let blocked = v["seccomp_blocked"].as_array().expect("array");
    assert_eq!(blocked.len(), 21, "21 syscalls denied: {v}");
    assert!(blocked.iter().any(|s| s == "unshare"), "unshare listed");
}
