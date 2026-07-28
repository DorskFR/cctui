//! Per-session opencode configuration.
//!
//! Generated at spawn from the dispatch payload + gateway env. Nothing here may be baked into the worker image: the
//! model comes from the spawn spec and the provider credential/base URL from
//! `FIREWORKS_API_KEY` / `FIREWORKS_BASE_URL`, which the gateway mints per
//! session. The api key is referenced as `{env:FIREWORKS_API_KEY}` so it is
//! resolved from the child's env and never written to disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub const PROVIDER_ID: &str = "fireworks-ai";
pub const API_KEY_ENV: &str = "FIREWORKS_API_KEY";
pub const BASE_URL_ENV: &str = "FIREWORKS_BASE_URL";
pub const DEFAULT_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
pub const REVIEWER_AGENT: &str = "cctui-reviewer";

const REVIEWER_PROMPT: &str = "You are an automated code reviewer running without a human in the \
loop. Inspect the diff and the repository and report security, correctness, performance and \
maintainability findings, each with file and line. You cannot modify anything, run arbitrary \
commands, or reach the network: only reading the repository and `git diff`/`git log`/`git show` \
are available. Never ask a question — state your findings and stop.";

/// A `provider/model` pair as the spawn spec carries it (e.g.
/// `fireworks-ai/accounts/fireworks/models/kimi-k3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelRef {
    #[must_use]
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }
        Some(match spec.split_once('/') {
            Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
                Self { provider_id: provider.to_owned(), model_id: model.to_owned() }
            }
            _ => Self { provider_id: PROVIDER_ID.to_owned(), model_id: spec.to_owned() },
        })
    }

    #[must_use]
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// Build the `opencode.json` the session runs under.
///
/// `thinking` is disabled: Fireworks serves Kimi without a separate reasoning
/// channel, and leaving it on makes tool-call turns 400 on a missing
/// `reasoning_content`.
#[must_use]
pub fn session_config(model: Option<&ModelRef>, env: &BTreeMap<String, String>) -> Value {
    let base_url = env.get(BASE_URL_ENV).map_or(DEFAULT_BASE_URL, String::as_str);
    let mut cfg = json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            PROVIDER_ID: {
                "npm": "@ai-sdk/openai-compatible",
                "name": "Fireworks AI",
                "options": {
                    "baseURL": base_url,
                    "apiKey": format!("{{env:{API_KEY_ENV}}}"),
                },
            }
        },
        "permission": { "doom_loop": "deny" },
        "agent": { REVIEWER_AGENT: reviewer_agent() },
        "autoupdate": false,
        "share": "disabled",
    });

    if let Some(model) = model {
        cfg["model"] = json!(model.qualified());
        if model.provider_id == PROVIDER_ID {
            cfg["provider"][PROVIDER_ID]["models"] = json!({
                model.model_id.clone(): { "options": { "thinking": { "type": "disabled" } } }
            });
        }
    }
    cfg
}

/// The locked-down reviewer profile: no edits, no network, and bash reduced to
/// read-only git inspection. `doom_loop: deny` hard-stops the repeat-tool-call
/// loops Kimi is prone to; `steps` bounds the turn.
#[must_use]
pub fn reviewer_agent() -> Value {
    json!({
        "description": "Read-only automated code review (cctui)",
        "mode": "primary",
        "prompt": REVIEWER_PROMPT,
        "steps": 120,
        "permission": {
            "edit": "deny",
            "webfetch": "deny",
            "websearch": "deny",
            "task": "deny",
            "question": "deny",
            "doom_loop": "deny",
            "external_directory": "deny",
            "read": "allow",
            "glob": "allow",
            "grep": "allow",
            "bash": {
                "*": "deny",
                "git diff*": "allow",
                "git log*": "allow",
                "git show*": "allow",
                "git status*": "allow",
            },
        },
    })
}

/// Layout of the session's ephemeral opencode HOME.
#[derive(Debug, Clone)]
pub struct SessionHome {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub config_file: PathBuf,
}

impl SessionHome {
    #[must_use]
    pub fn under(root: &Path, key: &str) -> Self {
        let home = root.join(sanitize_key(key));
        let config_home = home.join(".config");
        let data_home = home.join(".local/share");
        let config_file = config_home.join("opencode/opencode.json");
        Self { home, config_home, data_home, config_file }
    }

    /// Env vars pointing opencode at this home. Kept separate from the gateway
    /// env so the caller can layer them without letting the spawn payload
    /// override where the config lives.
    #[must_use]
    pub fn env(&self) -> Vec<(String, String)> {
        vec![
            ("HOME".to_owned(), self.home.display().to_string()),
            ("XDG_CONFIG_HOME".to_owned(), self.config_home.display().to_string()),
            ("XDG_DATA_HOME".to_owned(), self.data_home.display().to_string()),
            ("XDG_CACHE_HOME".to_owned(), self.home.join(".cache").display().to_string()),
            ("XDG_STATE_HOME".to_owned(), self.home.join(".local/state").display().to_string()),
        ]
    }

    pub fn write_config(&self, config: &Value) -> Result<()> {
        let dir = self.config_file.parent().context("config file has no parent")?;
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(&self.data_home)?;
        std::fs::write(&self.config_file, serde_json::to_vec_pretty(config)?)
            .with_context(|| format!("write {}", self.config_file.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.config_file, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

fn sanitize_key(key: &str) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if cleaned.is_empty() { "session".to_owned() } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn model_ref_splits_provider_and_model() {
        let m = ModelRef::parse("fireworks-ai/accounts/fireworks/models/kimi-k3").unwrap();
        assert_eq!(m.provider_id, "fireworks-ai");
        assert_eq!(m.model_id, "accounts/fireworks/models/kimi-k3");
        assert_eq!(m.qualified(), "fireworks-ai/accounts/fireworks/models/kimi-k3");
    }

    #[test]
    fn bare_model_defaults_to_the_fireworks_provider() {
        let m = ModelRef::parse("kimi-k3").unwrap();
        assert_eq!(m.provider_id, PROVIDER_ID);
        assert_eq!(m.model_id, "kimi-k3");
        assert!(ModelRef::parse("  ").is_none());
    }

    #[test]
    fn config_takes_base_url_from_the_gateway_env_and_never_the_key() {
        let model = ModelRef::parse("fireworks-ai/accounts/fireworks/models/kimi-k3").unwrap();
        let env = env_of(&[
            (BASE_URL_ENV, "https://cctui.example/gateway/fireworks"),
            (API_KEY_ENV, "sk-secret-value"),
        ]);
        let cfg = session_config(Some(&model), &env);
        assert_eq!(
            cfg["provider"][PROVIDER_ID]["options"]["baseURL"],
            "https://cctui.example/gateway/fireworks"
        );
        assert_eq!(cfg["provider"][PROVIDER_ID]["options"]["apiKey"], "{env:FIREWORKS_API_KEY}");
        assert_eq!(cfg["model"], "fireworks-ai/accounts/fireworks/models/kimi-k3");
        let rendered = cfg.to_string();
        assert!(!rendered.contains("sk-secret-value"), "the api key must never be written out");
    }

    #[test]
    fn config_falls_back_to_the_public_base_url() {
        let cfg = session_config(None, &BTreeMap::new());
        assert_eq!(cfg["provider"][PROVIDER_ID]["options"]["baseURL"], DEFAULT_BASE_URL);
        assert!(cfg.get("model").is_none());
    }

    #[test]
    fn thinking_is_disabled_for_the_selected_fireworks_model() {
        let model = ModelRef::parse("fireworks-ai/accounts/fireworks/models/kimi-k3").unwrap();
        let cfg = session_config(Some(&model), &BTreeMap::new());
        assert_eq!(
            cfg["provider"][PROVIDER_ID]["models"]["accounts/fireworks/models/kimi-k3"]["options"]
                ["thinking"]["type"],
            "disabled"
        );
    }

    #[test]
    fn reviewer_agent_is_locked_down() {
        let a = reviewer_agent();
        assert_eq!(a["mode"], "primary");
        assert_eq!(a["permission"]["edit"], "deny");
        assert_eq!(a["permission"]["webfetch"], "deny");
        assert_eq!(a["permission"]["websearch"], "deny");
        assert_eq!(a["permission"]["doom_loop"], "deny");
        assert_eq!(a["permission"]["bash"]["*"], "deny");
        assert_eq!(a["permission"]["bash"]["git diff*"], "allow");
        assert_eq!(a["permission"]["bash"]["git log*"], "allow");
        assert_eq!(a["permission"]["bash"]["git show*"], "allow");
        assert!(a["steps"].as_u64().unwrap() > 0);
    }

    #[test]
    fn config_ships_the_reviewer_agent_and_disables_autoupdate() {
        let cfg = session_config(None, &BTreeMap::new());
        assert_eq!(cfg["agent"][REVIEWER_AGENT]["permission"]["edit"], "deny");
        assert_eq!(cfg["autoupdate"], false);
        assert_eq!(cfg["permission"]["doom_loop"], "deny");
    }

    #[test]
    fn session_home_writes_the_config_under_xdg_config_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = SessionHome::under(tmp.path(), "ses/../weird id");
        home.write_config(&session_config(None, &BTreeMap::new())).unwrap();
        assert!(home.config_file.ends_with("opencode/opencode.json"));
        assert!(home.config_file.starts_with(&home.config_home));
        assert!(home.data_home.is_dir());
        let back: Value =
            serde_json::from_slice(&std::fs::read(&home.config_file).unwrap()).unwrap();
        assert_eq!(back["provider"][PROVIDER_ID]["npm"], "@ai-sdk/openai-compatible");

        let env: BTreeMap<String, String> = home.env().into_iter().collect();
        assert_eq!(env["HOME"], home.home.display().to_string());
        assert_eq!(env["XDG_CONFIG_HOME"], home.config_home.display().to_string());
        assert!(!home.home.display().to_string().contains(".."));
    }
}
