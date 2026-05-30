//! Discover the `claude daemon` control socket.
//!
//! Path scheme: `/tmp/cc-daemon-<uid>/<hash>/control.sock`. `<hash>` is a
//! `$HOME`-derived directory name we do not need to reverse — we enumerate
//! `/tmp/cc-daemon-<uid>/` and pick the unique subdirectory that contains
//! a `control.sock`. Tests override the base via [`Discovery::with_base`].

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Discovery {
    base: PathBuf,
}

impl Discovery {
    /// Default: `/tmp/cc-daemon-<uid>` for the current process.
    #[must_use]
    pub fn for_current_user() -> Self {
        let uid = rustix::process::getuid().as_raw();
        Self::with_base(PathBuf::from(format!("/tmp/cc-daemon-{uid}")))
    }

    #[must_use]
    pub const fn with_base(base: PathBuf) -> Self {
        Self { base }
    }

    /// Returns the first `<base>/<hash>/control.sock` that exists, or
    /// `None` if the daemon hasn't been spawned yet. Picks
    /// deterministically by lexicographically sorted hash directory.
    pub fn locate(&self) -> Option<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.base)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        entries.sort();
        for dir in entries {
            let candidate = dir.join("control.sock");
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_returns_none_when_base_missing() {
        let d = Discovery::with_base(PathBuf::from("/tmp/cctui-test-missing-xyz"));
        assert!(d.locate().is_none());
    }

    #[test]
    fn locate_finds_first_control_sock() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let hash_dir = base.join("06032065");
        std::fs::create_dir(&hash_dir).unwrap();
        // Need a real file at control.sock — a UDS bind is not necessary
        // for locate(); it only checks existence.
        std::fs::write(hash_dir.join("control.sock"), b"").unwrap();
        let d = Discovery::with_base(base);
        let p = d.locate().unwrap();
        assert!(p.ends_with("06032065/control.sock"));
    }

    #[test]
    fn locate_skips_dirs_without_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        std::fs::create_dir(base.join("empty")).unwrap();
        let good = base.join("00abcdef");
        std::fs::create_dir(&good).unwrap();
        std::fs::write(good.join("control.sock"), b"").unwrap();
        let d = Discovery::with_base(base);
        let p = d.locate().unwrap();
        assert!(p.ends_with("00abcdef/control.sock"));
    }
}
