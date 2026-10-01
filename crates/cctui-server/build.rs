use std::process::Command;

fn main() {
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

    // The Claude Code release the Anthropic `User-Agent` claims; release builds
    // resolve it from upstream latest. Re-exported under another name so a
    // malformed value never reaches `option_env!`.
    println!("cargo:rerun-if-env-changed=CCTUI_CLAUDE_CLI_VERSION");
    if let Some(v) = std::env::var("CCTUI_CLAUDE_CLI_VERSION").ok().filter(|v| !v.trim().is_empty())
    {
        let v = v.trim();
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|n| !n.is_empty() && n.parse::<u32>().is_ok()) {
            println!("cargo:rustc-env=CCTUI_BUILD_CLAUDE_CLI_VERSION={v}");
        } else {
            println!("cargo:warning=ignoring CCTUI_CLAUDE_CLI_VERSION={v:?}: not x.y.z");
        }
    }
}
