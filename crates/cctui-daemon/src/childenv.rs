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

/// Daemon-internal capability vars stripped from every exec'd agent child.
///
/// A `Command` inherits the daemon's full env by default, and the agent is
/// untrusted code that can read its own env. `CCTUI_MACHINE_KEY[_FILE]` is the
/// machine key the daemon authenticates to cctui-server with (impersonation);
/// `REPLY_URL` is the terminal result-callback bearer (completion spoofing).
pub const CHILD_ENV_REMOVALS: &[&str] =
    &["CCTUI_MACHINE_KEY", "CCTUI_MACHINE_KEY_FILE", "REPLY_URL"];

/// The capability vars stripped from every exec'd agent child. See
/// [`CHILD_ENV_REMOVALS`].
#[must_use]
pub const fn child_env_removals() -> &'static [&'static str] {
    CHILD_ENV_REMOVALS
}

/// A spawnable command whose environment can be scrubbed of capability vars.
///
/// Implemented for both `std` and `tokio` `Command` so
/// [`ScrubChildEnv::scrub_child_env`] applies uniformly at every spawn site.
pub trait ScrubChildEnv {
    /// Remove every var in [`CHILD_ENV_REMOVALS`] from the child's environment.
    fn scrub_child_env(&mut self) -> &mut Self;
}

impl ScrubChildEnv for std::process::Command {
    fn scrub_child_env(&mut self) -> &mut Self {
        for var in CHILD_ENV_REMOVALS {
            self.env_remove(var);
        }
        self
    }
}

impl ScrubChildEnv for tokio::process::Command {
    fn scrub_child_env(&mut self) -> &mut Self {
        for var in CHILD_ENV_REMOVALS {
            self.env_remove(var);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{CHILD_ENV_REMOVALS, ScrubChildEnv, child_env_removals, child_path};

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

    #[test]
    fn removals_cover_the_daemon_capability_vars() {
        for want in ["CCTUI_MACHINE_KEY", "CCTUI_MACHINE_KEY_FILE", "REPLY_URL"] {
            assert!(child_env_removals().contains(&want), "missing {want} from removal list");
        }
    }

    #[test]
    fn scrub_child_env_removes_every_capability_var() {
        use std::ffi::OsStr;
        let mut cmd = std::process::Command::new("true");
        for var in CHILD_ENV_REMOVALS {
            cmd.env(var, "leaked");
        }
        cmd.scrub_child_env();
        let envs: std::collections::HashMap<&OsStr, Option<&OsStr>> = cmd.get_envs().collect();
        for var in CHILD_ENV_REMOVALS {
            assert_eq!(
                envs.get(OsStr::new(var)),
                Some(&None),
                "{var} was not scrubbed from the child command"
            );
        }
    }
}
