fn main() {
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
