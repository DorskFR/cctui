//! Environment for child processes the daemon exec's (`codex`, `claude`).
//!
//! Under launchd, a user agent inherits a minimal `PATH`
//! (`/usr/local/bin:/usr/bin:/bin`) that omits `/opt/homebrew/bin` and
//! `~/.local/bin` — so `Command::new("codex")` / `Command::new("claude")`
//! fail with ENOENT. The plist install path now bakes the
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

/// Non-secret origin of the cctui server (`scheme://host[:port]`), handed to
/// every agent child so plugin skills can address the webui the user is on.
pub const WEB_ORIGIN_VAR: &str = "CCTUI_WEB_ORIGIN";

/// The origin part of `server_url`: scheme + authority, no path, no trailing
/// slash. `None` when the url has no scheme or host.
#[must_use]
pub fn web_origin(server_url: &str) -> Option<String> {
    let url = server_url.trim();
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// Add [`WEB_ORIGIN_VAR`] derived from `server_url` unless the caller set it.
/// Tell the session which cctui session it is (non-secret), so tools such
/// as `cctui-daemon preview open` can act on its behalf.
pub fn with_session_id(env: &mut std::collections::BTreeMap<String, String>, session_id: &str) {
    if !session_id.trim().is_empty() {
        env.insert(crate::preview::SESSION_ID_VAR.to_owned(), session_id.to_owned());
    }
}

pub fn with_web_origin(env: &mut std::collections::BTreeMap<String, String>, server_url: &str) {
    if let Some(origin) = web_origin(server_url) {
        env.entry(WEB_ORIGIN_VAR.to_owned()).or_insert(origin);
    }
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
    use super::{
        CHILD_ENV_REMOVALS, ScrubChildEnv, WEB_ORIGIN_VAR, child_env_removals, child_path,
        web_origin, with_session_id, with_web_origin,
    };
    use std::collections::BTreeMap;

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
    fn web_origin_keeps_scheme_and_authority_only() {
        assert_eq!(
            web_origin("https://cctui.example.com").as_deref(),
            Some("https://cctui.example.com")
        );
        assert_eq!(
            web_origin("https://cctui.example.com/").as_deref(),
            Some("https://cctui.example.com")
        );
        assert_eq!(
            web_origin("http://localhost:8700/api/v1?x=1").as_deref(),
            Some("http://localhost:8700")
        );
        assert_eq!(web_origin("https://user:pw@host:1/p").as_deref(), Some("https://host:1"));
        assert_eq!(web_origin("localhost:8700"), None);
        assert_eq!(web_origin("https://"), None);
        assert_eq!(web_origin(""), None);
    }

    #[test]
    fn with_web_origin_sets_the_var_without_overriding() {
        let mut env = BTreeMap::new();
        with_web_origin(&mut env, "https://s.example.test/");
        assert_eq!(env.get(WEB_ORIGIN_VAR).map(String::as_str), Some("https://s.example.test"));
        let mut preset = BTreeMap::from([(WEB_ORIGIN_VAR.to_owned(), "https://mine".to_owned())]);
        with_web_origin(&mut preset, "https://s.example.test");
        assert_eq!(preset[WEB_ORIGIN_VAR], "https://mine");
        let mut none = BTreeMap::new();
        with_web_origin(&mut none, "not a url");
        assert!(none.is_empty());
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

    #[test]
    fn session_id_is_exported_when_known() {
        let mut env = std::collections::BTreeMap::new();
        with_session_id(&mut env, " ");
        assert!(env.is_empty());
        with_session_id(&mut env, "sess-1");
        assert_eq!(env.get(crate::preview::SESSION_ID_VAR).map(String::as_str), Some("sess-1"));
    }
}
