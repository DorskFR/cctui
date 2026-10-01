//! One-time repair of `~/.claude/settings.json`.
//!
//! cctui used to merge a block of `curl` hooks into the user's Claude Code
//! settings. The server routes they call were retired, so every one of them
//! now fails on each tool call; the daemon covers the same events through the
//! settings file it manages itself. Nothing is written here any more except
//! the removal of those dead entries.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

/// Bump whenever [`migrate_legacy_hooks`] has new work to do on machines that
/// already ran an older binary. Self-update compares this against the integer
/// in `~/.cctui/settings_schema`.
pub const SETTINGS_SCHEMA_VERSION: u32 = 4;

const SCHEMA_MARKER_FILENAME: &str = "settings_schema";

/// A hook command naming either of these talks to a route that no longer
/// exists, so it can only fail.
const DEAD_ENDPOINTS: [&str; 2] = ["/api/v1/hooks/", "/api/v1/check"];

fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

fn cctui_home() -> Option<PathBuf> {
    std::env::var_os("CCTUI_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".cctui")))
}

#[must_use]
pub fn schema_marker_path() -> Option<PathBuf> {
    cctui_home().map(|d| d.join(SCHEMA_MARKER_FILENAME))
}

#[must_use]
pub fn read_schema_marker() -> u32 {
    schema_marker_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

pub fn write_schema_marker(v: u32) -> Result<()> {
    let path = schema_marker_path().context("could not resolve ~/.cctui path")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, v.to_string())?;
    Ok(())
}

fn is_dead_hook(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|cmd| DEAD_ENDPOINTS.iter().any(|dead| cmd.contains(dead)))
}

/// Drops the dead entries of one matcher group. Returns false when the group
/// itself should go, which is only once its last hook has been removed — a
/// group that never held one is left exactly as it was.
fn retain_live_hooks(group: &mut Value) -> bool {
    let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
        return true;
    };
    let before = hooks.len();
    hooks.retain(|entry| !is_dead_hook(entry));
    before == hooks.len() || !hooks.is_empty()
}

/// Strips the dead hooks from every event, leaving nothing empty behind.
/// Returns whether anything changed. An event that was already empty is left
/// alone: only what this migration empties is removed.
fn strip_dead_hooks(hooks: &mut serde_json::Map<String, Value>) -> bool {
    let mut emptied: Vec<String> = Vec::new();
    let mut changed = false;
    for (name, event) in hooks.iter_mut() {
        let Some(groups) = event.as_array_mut() else { continue };
        let before = groups.clone();
        groups.retain_mut(retain_live_hooks);
        if *groups == before {
            continue;
        }
        changed = true;
        if groups.is_empty() {
            emptied.push(name.clone());
        }
    }
    for name in &emptied {
        hooks.remove(name);
    }
    changed
}

/// Removes cctui's retired `curl` hooks from the user's Claude Code settings,
/// touching nothing else.
///
/// A file that holds none of them is not rewritten, so a hand-formatted
/// settings.json keeps its formatting; a missing one is not created.
pub fn migrate_legacy_hooks() -> Result<()> {
    let home = home_dir().context("could not resolve $HOME")?;
    let path = home.join(".claude/settings.json");
    let Ok(bytes) = std::fs::read(&path) else { return Ok(()) };

    // Unreadable JSON is the user's file to fix: rewriting it would lose it.
    let Ok(mut settings) = serde_json::from_slice::<Value>(&bytes) else { return Ok(()) };
    let Some(root) = settings.as_object_mut() else { return Ok(()) };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    if !strip_dead_hooks(hooks) {
        return Ok(());
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    write_json_pretty(&path, &settings)
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        prev_home: Option<std::ffi::OsString>,
        prev_cctui: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let prev_home = std::env::var_os("HOME");
            let prev_cctui = std::env::var_os("CCTUI_HOME");
            // SAFETY: tests in this module run serially (single-threaded default);
            // env is only mutated through this guard.
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("HOME", home);
                std::env::set_var("CCTUI_HOME", home.join(".cctui"));
            }
            Self { prev_home, prev_cctui, _guard: guard }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            #[allow(unsafe_code)]
            unsafe {
                match &self.prev_home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match &self.prev_cctui {
                    Some(v) => std::env::set_var("CCTUI_HOME", v),
                    None => std::env::remove_var("CCTUI_HOME"),
                }
            }
        }
    }

    const LEGACY_CURL: &str = "KEY=\"$CCTUI_AGENT_TOKEN\"; cat | curl -sf -X POST \
'https://s.example/api/v1/hooks/post-tool-use' -d @-";

    fn settings_path(home: &Path) -> PathBuf {
        home.join(".claude/settings.json")
    }

    fn write_settings(home: &Path, body: &str) -> PathBuf {
        let path = settings_path(home);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, body).expect("write");
        path
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    fn json_at(path: &Path) -> Value {
        serde_json::from_str(&read(path)).expect("valid json")
    }

    /// The block cctui used to merge in, next to a hook the user wrote.
    fn legacy_and_user_hooks() -> String {
        serde_json::json!({
            "theme": "dark",
            "hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command":
                    "curl -sf -X POST 'https://s.example/api/v1/hooks/session-start'"}]}],
                "PostToolUse": [
                    {"hooks": [{"type": "command", "command": LEGACY_CURL}]},
                    {"matcher": "Edit|Write",
                     "hooks": [{"type": "command", "command": "~/bin/fmt.sh"}]},
                ],
                "Stop": [{"hooks": [{"type": "command", "command":
                    "curl -sf -X POST 'https://s.example/api/v1/hooks/stop'"}]}],
                "PreToolUse": [{"hooks": [{"type": "command", "command":
                    "curl -sf -X POST 'https://s.example/api/v1/check'"}]}],
            }
        })
        .to_string()
    }

    #[test]
    fn schema_marker_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        assert_eq!(read_schema_marker(), 0);
        write_schema_marker(7).unwrap();
        assert_eq!(read_schema_marker(), 7);
    }

    #[test]
    fn the_dead_hooks_go_and_the_users_own_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let path = write_settings(tmp.path(), &legacy_and_user_hooks());

        migrate_legacy_hooks().unwrap();

        let v = json_at(&path);
        assert_eq!(v["theme"], "dark", "nothing outside hooks is touched");
        assert!(v["hooks"]["SessionStart"].is_null());
        assert!(v["hooks"]["Stop"].is_null());
        assert!(v["hooks"]["PreToolUse"].is_null());
        let post = v["hooks"]["PostToolUse"].as_array().expect("the user hook stays");
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["matcher"], "Edit|Write");
        assert_eq!(post[0]["hooks"][0]["command"], "~/bin/fmt.sh");
        assert!(!read(&path).contains("/api/v1/"), "no dead endpoint is left");
    }

    #[test]
    fn a_file_of_nothing_but_dead_hooks_keeps_no_empty_scaffolding() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let path = write_settings(
            tmp.path(),
            &serde_json::json!({
                "theme": "dark",
                "hooks": {
                    "PostToolUse": [{"hooks": [{"type": "command", "command": LEGACY_CURL}]}],
                }
            })
            .to_string(),
        );

        migrate_legacy_hooks().unwrap();

        let body = read(&path);
        assert!(!body.contains("hooks"), "no empty hooks scaffolding survives: {body}");
        assert_eq!(json_at(&path)["theme"], "dark");
    }

    #[test]
    fn a_second_run_changes_nothing_byte_for_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let path = write_settings(tmp.path(), &legacy_and_user_hooks());

        migrate_legacy_hooks().unwrap();
        let first = read(&path);
        migrate_legacy_hooks().unwrap();
        assert_eq!(first, read(&path));
    }

    #[test]
    fn a_file_with_no_dead_hooks_is_not_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        // Compact and oddly ordered on purpose: a rewrite would reformat it.
        let original = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"fmt"}]}],"Other":[]},"theme":"dark"}"#;
        let path = write_settings(tmp.path(), original);

        migrate_legacy_hooks().unwrap();

        assert_eq!(read(&path), original, "an untouched file keeps its own formatting");
    }

    #[test]
    fn a_missing_settings_file_is_not_created() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        migrate_legacy_hooks().unwrap();
        assert!(!settings_path(tmp.path()).exists());
        assert!(!tmp.path().join(".claude").exists());
    }

    #[test]
    fn a_settings_file_that_is_not_json_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let path = write_settings(tmp.path(), "{ not json at all");
        migrate_legacy_hooks().unwrap();
        assert_eq!(read(&path), "{ not json at all");
    }

    #[test]
    fn a_group_keeps_its_live_hooks_when_only_one_of_them_is_dead() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let path = write_settings(
            tmp.path(),
            &serde_json::json!({
                "hooks": {"Stop": [{"hooks": [
                    {"type": "command", "command": LEGACY_CURL},
                    {"type": "command", "command": "notify-send done"},
                ]}]}
            })
            .to_string(),
        );

        migrate_legacy_hooks().unwrap();

        let hooks = json_at(&path)["hooks"]["Stop"][0]["hooks"].clone();
        assert_eq!(hooks.as_array().expect("the group stays").len(), 1);
        assert_eq!(hooks[0]["command"], "notify-send done");
    }
}
