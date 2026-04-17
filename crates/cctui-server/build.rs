use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../channel/dist/channel.js");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");

    // Prefer an explicit build arg (set in CI / Docker) over `git` invocation,
    // since Docker builds don't have the .git directory in scope.
    let git_hash =
        std::env::var("CCTUI_GIT_HASH").ok().filter(|s| !s.trim().is_empty()).or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        });
    let git_hash = git_hash.unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=CCTUI_GIT_HASH={git_hash}");
}
