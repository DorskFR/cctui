use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../channel/dist/channel.js");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");

    let git_hash = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".into(), |s| s.trim().to_string());

    println!("cargo:rustc-env=CCTUI_GIT_HASH={git_hash}");
}
