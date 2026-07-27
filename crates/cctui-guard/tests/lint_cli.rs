//! End-to-end coverage of the `cctui-guard lint` subcommand and `--check`
//! startup gate: drives the compiled binary against on-disk prompt + rules
//! fixtures and asserts exit status, diagnostics, and the `--explain` dump.

use std::io::Write;
use std::process::Command;

use tempfile::NamedTempFile;

const RULES: &str = "\
[code-read]: Read, Grep, Glob
[code-write]: Edit, Write
[all-read]: code-read
[net-claude]: api.example.com:443
[net-github]: github.example.com:443, github.example.com:22
";

fn write(contents: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    f
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn lint(prompt: &str, extra: &[&str]) -> Run {
    let rules = write(RULES);
    let prompt = write(prompt);
    let out = Command::new(env!("CARGO_BIN_EXE_cctui-guard"))
        .arg("lint")
        .arg(prompt.path())
        .arg("--rules")
        .arg(rules.path())
        .args(extra)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn clean_prompt_exits_zero() {
    let run = lint(
        "\
# Step 1: Research
[allowed]: all-read
[network]: net-claude, net-github
[transition]: 2, Exit

# Step 2: Implement
[allowed]: all-read, code-write
[network]: net-claude
[transition]: Exit
",
        &[],
    );
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(run.stderr.contains("lint passed"));
}

#[test]
fn broken_prompt_exits_one_with_diagnostics() {
    let run = lint(
        "\
# Step 1
[allowed]: all-reads
[network]: net-guthub
[transition]: 9, Exit
",
        &[],
    );
    assert_eq!(run.code, 1);
    assert!(run.stderr.contains("unknown set 'all-reads'"));
    assert!(run.stderr.contains("unknown network set 'net-guthub'"));
    assert!(run.stderr.contains("undefined Step 9"));
}

#[test]
fn explain_dumps_resolved_policy_to_stdout() {
    let run = lint(
        "\
# Step 1: Research
[allowed]: all-read
[network]: net-github
[transition]: Exit
",
        &["--explain"],
    );
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(run.stdout.contains("Step 1: Research"));
    assert!(run.stdout.contains("Read, Grep, Glob"));
    assert!(run.stdout.contains("github.example.com:443, github.example.com:22"));
}

#[test]
fn unreadable_rules_import_is_error() {
    let run = lint(
        "\
[rules]: ./definitely-missing-pack.md

# Step 1
[transition]: Exit
",
        &[],
    );
    assert_eq!(run.code, 1, "stdout: {} stderr: {}", run.stdout, run.stderr);
    assert!(run.stderr.contains("definitely-missing-pack.md"), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("unreadable"), "stderr: {}", run.stderr);
}

#[test]
fn inline_and_imported_sets_report_provenance_in_explain() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("net-common.md"), "[net-shared]: shared.example:443\n").unwrap();
    let prompt_path = dir.path().join("task.md");
    std::fs::write(
        &prompt_path,
        "\
[rules]: ./net-common.md
[net-yt]: yt.example.com:443

# Step 1
[allowed]: *
[network]: net-yt, net-shared
[transition]: Exit
",
    )
    .unwrap();
    let rules = write(RULES);
    let out = Command::new(env!("CARGO_BIN_EXE_cctui-guard"))
        .arg("lint")
        .arg(&prompt_path)
        .arg("--rules")
        .arg(rules.path())
        .arg("--explain")
        .output()
        .unwrap();
    assert_eq!(out.status.code().unwrap(), 0, "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("net-yt: inline"), "stdout: {stdout}");
    assert!(stdout.contains("net-shared: [rules] ./net-common.md"), "stdout: {stdout}");
}

#[test]
fn check_flag_refuses_to_start_on_errors() {
    let rules = write(RULES);
    let prompt = write(
        "\
# Step 1
[transition]: 9, Exit
",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_cctui-guard"))
        .arg("--prompt")
        .arg(prompt.path())
        .arg("--rules")
        .arg(rules.path())
        .arg("--check")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("refusing to start"), "stderr: {stderr}");
}
