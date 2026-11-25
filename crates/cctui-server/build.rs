fn main() {
    println!("cargo:rerun-if-env-changed=CCTUI_GIT_HASH");
    let git_hash = std::env::var("CCTUI_GIT_HASH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CCTUI_GIT_HASH={git_hash}");
}
