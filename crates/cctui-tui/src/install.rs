//! Port of the config-writing half of `scripts/install.sh`.
//!
//! Keeps `~/.claude.json` (MCP server entry) and `~/.claude/settings.json`
//! (Claude Code hooks) in sync with the schema the current binary expects.
//! Idempotent: running it twice on an already-configured host is a no-op.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// Bump whenever the JSON emitted by `apply_settings` changes meaningfully
/// (new hook, changed endpoint, altered prelude…). Self-update compares this
/// against the integer stored in `~/.cctui/settings_schema` to decide whether
/// to re-merge the user's settings files.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

const SCHEMA_MARKER_FILENAME: &str = "settings_schema";

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

fn auth_prelude(fallback_token: &str) -> String {
    format!(
        "KEY=\"${{CCTUI_AGENT_TOKEN:-$(jq -r .machine_key \
\"${{XDG_CONFIG_HOME:-$HOME/.config}}/cctui/machine.json\" 2>/dev/null)}}\"; \
[ -z \"$KEY\" ] && KEY=\"{fallback_token}\"; "
    )
}

fn curl_cmd(server_url: &str, path: &str, enrich: Option<&str>, fallback_token: &str) -> String {
    let pipe = enrich.map_or_else(|| "cat".to_string(), |jq_args| format!("jq -c {jq_args}"));
    format!(
        "{prelude}{pipe} | curl -sf -X POST {server_url}{path} \
-H 'Content-Type: application/json' \
-H \"Authorization: Bearer $KEY\" -d @-",
        prelude = auth_prelude(fallback_token),
    )
}

fn build_hooks(server_url: &str, fallback_token: &str) -> Value {
    let session_start = curl_cmd(
        server_url,
        "/api/v1/hooks/session-start",
        Some(
            "--arg ppid \"$PPID\" --arg mid \"$(hostname)\" \
'. + {ppid: ($ppid | tonumber), machine_id: $mid}'",
        ),
        fallback_token,
    );
    let check = curl_cmd(server_url, "/api/v1/check", None, fallback_token);
    let post_tool = curl_cmd(server_url, "/api/v1/hooks/post-tool-use", None, fallback_token);
    let stop = curl_cmd(server_url, "/api/v1/hooks/stop", None, fallback_token);

    json!({
        "SessionStart": [{"hooks": [{"type": "command", "command": session_start}]}],
        "PreToolUse":   [{"hooks": [{"type": "command", "command": check}]}],
        "PostToolUse":  [{"hooks": [{"type": "command", "command": post_tool}]}],
        "Stop":         [{"hooks": [{"type": "command", "command": stop}]}],
    })
}

fn load_json_or_empty(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Merge the cctui MCP entry + hook block into the user's Claude Code config.
///
/// `fallback_token` is embedded verbatim into the hook command strings as the
/// last-resort auth token if neither `$CCTUI_AGENT_TOKEN` nor `machine.json`
/// is readable at hook time. Pass the current `machine_key`.
pub fn apply_settings(server_url: &str, fallback_token: &str, bin_path: &Path) -> Result<()> {
    let home = home_dir().context("could not resolve $HOME")?;
    let claude_json = home.join(".claude.json");
    let settings_json = home.join(".claude/settings.json");

    // ~/.claude.json — MCP servers entry
    let mut cfg = load_json_or_empty(&claude_json);
    let obj = cfg.as_object_mut().context(".claude.json is not a JSON object")?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .context("mcpServers is not an object")?;
    servers.insert(
        "cctui".to_string(),
        json!({
            "command": bin_path.to_string_lossy(),
            "args": ["channel"],
            "env": { "CCTUI_URL": server_url },
        }),
    );
    write_json_pretty(&claude_json, &cfg)?;

    // ~/.claude/settings.json — hooks
    let mut settings = load_json_or_empty(&settings_json);
    let sobj = settings.as_object_mut().context("settings.json is not a JSON object")?;
    let hooks_val = sobj.entry("hooks").or_insert_with(|| Value::Object(serde_json::Map::new()));
    let new_hooks = build_hooks(server_url, fallback_token);
    if let (Some(existing), Some(new_map)) = (hooks_val.as_object_mut(), new_hooks.as_object()) {
        for (k, v) in new_map {
            existing.insert(k.clone(), v.clone());
        }
    } else {
        *hooks_val = new_hooks;
    }
    write_json_pretty(&settings_json, &settings)?;

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

    #[test]
    fn schema_marker_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        assert_eq!(read_schema_marker(), 0);
        write_schema_marker(7).unwrap();
        assert_eq!(read_schema_marker(), 7);
    }

    #[test]
    fn apply_settings_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let bin = tmp.path().join("cctui");
        std::fs::write(&bin, b"").unwrap();

        apply_settings("https://server.example", "cctui_m_test", &bin).unwrap();
        let first = std::fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        let first_claude = std::fs::read_to_string(tmp.path().join(".claude.json")).unwrap();

        apply_settings("https://server.example", "cctui_m_test", &bin).unwrap();
        let second = std::fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        let second_claude = std::fs::read_to_string(tmp.path().join(".claude.json")).unwrap();

        assert_eq!(first, second);
        assert_eq!(first_claude, second_claude);
        assert!(first.contains("session-start"));
        assert!(first_claude.contains("\"cctui\""));
    }

    #[test]
    fn apply_settings_preserves_unrelated_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = HomeGuard::set(tmp.path());
        let settings_path = tmp.path().join(".claude/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(&settings_path, r#"{"theme":"dark","hooks":{"Other":[]}}"#).unwrap();

        let bin = tmp.path().join("cctui");
        std::fs::write(&bin, b"").unwrap();
        apply_settings("https://s", "tok", &bin).unwrap();

        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert!(v["hooks"]["Other"].is_array());
        assert!(v["hooks"]["SessionStart"].is_array());
    }
}
