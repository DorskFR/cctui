//! Environment for child processes the daemon exec's (`codex`, `claude`).
//!
//! Under launchd, a user agent inherits a minimal `PATH`
//! (`/usr/local/bin:/usr/bin:/bin`) that omits `/opt/homebrew/bin` and
//! `~/.local/bin` — so `Command::new("codex")` / `Command::new("claude")`
//! fail with ENOENT (CCT-138). The plist install path now bakes the
//! install-time `$PATH` in, but a daemon that *self-updated* keeps the old
//! plist until the next `service install`. To make spawning robust regardless
//! of how the daemon was launched, every exec'd child is given an explicit
//! `PATH` augmented with the usual tool locations.

/// The `PATH` to hand to exec'd children: the daemon's own `PATH` plus the
/// common tool directories that launchd may have stripped, deduplicated while
/// preserving order.
#[must_use]
pub fn child_path() -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut push = |dir: String| {
        if !dir.is_empty() && !entries.contains(&dir) {
            entries.push(dir);
        }
    };
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            push(dir.to_string());
        }
    }
    if let Some(home) = dirs::home_dir() {
        push(home.join(".local").join("bin").display().to_string());
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        push(dir.to_string());
    }
    entries.join(":")
}

#[cfg(test)]
mod tests {
    use super::child_path;

    #[test]
    fn augments_with_common_dirs_and_dedups() {
        let path = child_path();
        let dirs: Vec<&str> = path.split(':').collect();
        // The common tool locations are always present.
        for want in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            assert!(dirs.contains(&want), "expected {want} in {path}");
        }
        // No duplicates: a minimal launchd PATH already containing
        // `/usr/local/bin` must not appear twice after augmentation.
        let mut sorted = dirs.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "duplicate entries in {path}");
    }
}
