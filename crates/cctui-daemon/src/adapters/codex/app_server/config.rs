use serde_json::Value;

use super::requests::normalize_service_tier;
use crate::adapters::codex::model_list;

/// Configuration for spawning the `codex app-server` subprocess.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppServerConfig {
    /// Binary to invoke (default `"codex"`).
    pub bin: String,
    /// Approval policy passed via `-c approval_policy=...`. `"on-request"`
    /// (the default) lets Codex ask for approval so the relay has something
    /// to forward; `"never"` disables prompts. Codex 0.153 refuses to start on
    /// `"untrusted"`.
    pub approval_policy: String,
    /// Sandbox mode passed via `-c sandbox_mode=...`. `"read-only"`
    /// and `"workspace-write"` wrap commands in bubblewrap; on a host whose
    /// kernel forbids unprivileged user namespaces those fail to launch, so a
    /// per-host default of `"danger-full-access"` (no sandbox) is required
    /// there. Overridable per-spawn via the full-access toggle.
    pub sandbox_mode: String,
    /// Reasoning effort passed via `-c model_reasoning_effort=...`
    /// (codex: `minimal`/`low`/`medium`/`high`). `None` keeps the codex
    /// default. Set per-spawn from the spawn request.
    pub reasoning_effort: Option<String>,
    /// Model passed via `-c model="…"`. `None` keeps the codex
    /// default. Set per-spawn from the spawn request.
    pub model: Option<String>,
    /// Per-session service tier, `"default"` or `"fast"`. NOT a
    /// `config_overrides()` key: it is per-thread, and lives here only because
    /// this struct is the durable per-session cache (persisted in
    /// [`SessionRecord::cfg`]) that lets `thread/{resume,fork}` re-supply it.
    /// `None` keeps codex's own (expensive `priority`) default.
    pub service_tier: Option<String>,
    /// Whether to refresh the codex model catalog on session start
    /// by issuing `model/list` over this session's authenticated app-server
    /// connection. `false` (`model_catalog = false`) disables the refresh.
    pub model_catalog: bool,
}

impl Default for AppServerConfig {
    fn default() -> Self {
        Self {
            bin: "codex".to_string(),
            approval_policy: "on-request".to_string(),
            sandbox_mode: "workspace-write".to_string(),
            reasoning_effort: None,
            model: None,
            service_tier: None,
            model_catalog: true,
        }
    }
}

impl AppServerConfig {
    /// The `-c key="value"` overrides passed to `codex app-server` for a spawn.
    /// This is the COMPLETE set of config knobs cctui sets. They are
    /// PROCESS-level — they apply to every thread this app-server serves — so
    /// nothing per-session belongs here.
    ///
    /// Fast mode (`service_tier = "fast"`) is deliberately absent. It is a
    /// speed/price tier — 1.5x speed and increased usage on the SAME model at
    /// the SAME quality, not a quality downgrade — and it is per-thread, so it
    /// rides [`ThreadConfig`] on `thread/{start,resume,fork}` instead.
    ///
    /// Omitting it here is NOT the safe branch: codex's own default tier is
    /// `priority` (every gpt-5.x entry in `models_cache.json` carries
    /// `"default_service_tier": "priority"`), so an app-server with no opinion
    /// runs the EXPENSIVE tier. The server resolves a concrete tier per session;
    /// the daemon must supply it per thread.
    #[must_use]
    pub fn config_overrides(&self) -> Vec<(String, String)> {
        let mut args = vec![
            ("approval_policy".to_owned(), self.approval_policy.clone()),
            ("sandbox_mode".to_owned(), self.sandbox_mode.clone()),
        ];
        if let Some(effort) = self.reasoning_effort.as_deref() {
            args.push(("model_reasoning_effort".to_owned(), effort.to_owned()));
        }
        if let Some(model) = self.model.as_deref() {
            args.push(("model".to_owned(), model.to_owned()));
        }
        args
    }

    pub fn from_value(v: &Value) -> Self {
        let mut cfg = Self::default();
        if let Some(b) = v.get("codex_bin").and_then(Value::as_str) {
            cfg.bin = b.to_string();
        }
        if let Some(p) = v.get("approval_policy").and_then(Value::as_str) {
            cfg.approval_policy = p.to_string();
        }
        if let Some(s) = v.get("sandbox_mode").and_then(Value::as_str) {
            cfg.sandbox_mode = s.to_string();
        }
        if let Some(e) = v.get("model_reasoning_effort").and_then(Value::as_str) {
            cfg.reasoning_effort = Some(e.to_string());
        }
        if let Some(m) = v.get("model").and_then(Value::as_str) {
            cfg.model = Some(m.to_string());
        }
        cfg.service_tier = normalize_service_tier(v.get("service_tier").and_then(Value::as_str));
        cfg.model_catalog = model_list::catalog_enabled(v);
        cfg
    }
}

/// The `-c` overrides that route codex's model provider through the cctui
/// gateway. Codex does NOT honor `OPENAI_BASE_URL`/`OPENAI_API_KEY`
/// from the environment alone: launched with only those env vars it POSTs to
/// api.openai.com with no Authorization header and 401s. It reads them solely
/// through a `model_providers` entry — `base_url` inlined here, the bearer via
/// `env_key` from the launch env at request time. Mirrors the worker
/// entrypoint's `phase_codex_config`, which fixed the same failure
/// for k8s workers by writing this block into config.toml. Empty only when the
/// base URL is absent (an unbound session keeps codex's default provider).
///
/// The block is emitted on the base URL ALONE, without the credential: codex
/// persists `model_provider = "cctui"` in the rollout, so a relaunch that omits
/// the definition fails config load on resume and bricks the thread
/// permanently, whereas a definition whose `env_key` is unset merely fails the
/// turn and heals on the next credential pull.
#[must_use]
pub fn gateway_provider_overrides(
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let Some(base_url) = env.get("OPENAI_BASE_URL") else {
        return Vec::new();
    };
    vec![
        ("model_provider".to_owned(), "cctui".to_owned()),
        ("model_providers.cctui.name".to_owned(), "cctui-gateway".to_owned()),
        ("model_providers.cctui.base_url".to_owned(), base_url.clone()),
        ("model_providers.cctui.env_key".to_owned(), "OPENAI_API_KEY".to_owned()),
        ("model_providers.cctui.wire_api".to_owned(), "responses".to_owned()),
        // Codex only registers the built-in `image_gen` tool for a provider that
        // `uses_openai_actor_authorization()` — a non-empty static
        // `x-openai-actor-authorization` header with `requires_openai_auth`
        // false. The value is never read upstream: the gateway strips it.
        (
            "model_providers.cctui.http_headers.\"x-openai-actor-authorization\"".to_owned(),
            "cctui-gateway".to_owned(),
        ),
    ]
}

/// Quote a value that is always a TOML string (`config_overrides` and the
/// gateway provider block are string-valued by construction).
fn quoted(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    pairs.into_iter().map(|(k, v)| (k, format!("\"{v}\""))).collect()
}

/// The full, ordered `-c key=value` list for an app-server launch, with values
/// already rendered as TOML literals.
///
/// Per-account settings go FIRST and cctui's managed overrides LAST, and a
/// managed key drops any account entry of the same name outright: the ladder
/// must not depend on codex's own last-wins behaviour for `-c` duplicates, and
/// an account must never be able to move gateway routing, the permission
/// posture, or the session's model.
pub(super) fn launch_overrides(
    cfg: &AppServerConfig,
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let managed: Vec<(String, String)> =
        [quoted(cfg.config_overrides()), quoted(gateway_provider_overrides(env))].concat();
    let account = env
        .get(cctui_proto::codex_config::CONFIG_TOML_ENV)
        .map(|b| cctui_proto::codex_config::overrides_from_block(b))
        .unwrap_or_default();
    let owned: std::collections::BTreeSet<&str> = managed.iter().map(|(k, _)| k.as_str()).collect();
    account
        .into_iter()
        .filter(|(k, _)| !owned.contains(k.as_str()))
        .chain(managed.iter().cloned())
        .collect()
}

/// Whether a session can run on the shared app-server without losing what a
/// private process would have given it: the `CctuiAgent` relay is declared per
/// process, and launch env beyond the gateway credential has no per-thread
/// equivalent.
#[must_use]
pub(super) fn shared_eligible(
    env: &std::collections::BTreeMap<String, String>,
    has_agent_mcp: bool,
) -> bool {
    if has_agent_mcp {
        return false;
    }
    let keyed = env.get("OPENAI_API_KEY").is_some_and(|k| !k.is_empty());
    if env.contains_key("OPENAI_BASE_URL") && !keyed {
        return false;
    }
    env.keys().all(|k| {
        matches!(k.as_str(), "OPENAI_BASE_URL" | "OPENAI_API_KEY")
            || k == cctui_proto::codex_config::CONFIG_TOML_ENV
    })
}

/// The `-c` overrides a stdio child would take on its command line, as a
/// per-thread overlay. The gateway block is left out: [`ThreadConfig`] already
/// supplies it per thread, with the bearer inline.
pub(super) fn shared_overlay(
    cfg: &AppServerConfig,
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let managed = quoted(cfg.config_overrides());
    let account = env
        .get(cctui_proto::codex_config::CONFIG_TOML_ENV)
        .map(|b| cctui_proto::codex_config::overrides_from_block(b))
        .unwrap_or_default();
    let owned: std::collections::BTreeSet<&str> = managed.iter().map(|(k, _)| k.as_str()).collect();
    account
        .into_iter()
        .filter(|(k, _)| !owned.contains(k.as_str()) && !k.starts_with("model_provider"))
        .chain(managed.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gateway_provider_overrides_route_via_gateway_when_env_bound() {
        let env: std::collections::BTreeMap<String, String> = [
            ("OPENAI_BASE_URL".to_owned(), "https://cctui.example/gateway/openai".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "cctui_s_tok".to_owned()),
            ("KEEP".to_owned(), "1".to_owned()),
        ]
        .into_iter()
        .collect();
        let got = gateway_provider_overrides(&env);
        assert_eq!(
            got,
            vec![
                ("model_provider".to_owned(), "cctui".to_owned()),
                ("model_providers.cctui.name".to_owned(), "cctui-gateway".to_owned()),
                (
                    "model_providers.cctui.base_url".to_owned(),
                    "https://cctui.example/gateway/openai".to_owned()
                ),
                ("model_providers.cctui.env_key".to_owned(), "OPENAI_API_KEY".to_owned()),
                ("model_providers.cctui.wire_api".to_owned(), "responses".to_owned()),
                (
                    "model_providers.cctui.http_headers.\"x-openai-actor-authorization\""
                        .to_owned(),
                    "cctui-gateway".to_owned()
                ),
            ]
        );
        assert!(got.iter().map(|(k, v)| format!("{k}=\"{v}\"")).any(|arg| arg
            == "model_providers.cctui.http_headers.\"x-openai-actor-authorization\"=\"cctui-gateway\""));
    }

    #[test]
    fn gateway_provider_overrides_empty_without_a_gateway_base_url() {
        let empty = std::collections::BTreeMap::new();
        assert!(gateway_provider_overrides(&empty).is_empty());
        let key_only: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_API_KEY".to_owned(), "tok".to_owned())).collect();
        assert!(gateway_provider_overrides(&key_only).is_empty());
    }

    /// A resume whose credential re-pull came back empty still has to DEFINE
    /// `model_providers.cctui`: codex reads the provider NAME back out of the
    /// rollout and fails config load (-32600) on a dangling reference.
    #[test]
    fn gateway_provider_is_defined_from_the_base_url_alone() {
        let url_only: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_BASE_URL".to_owned(), "https://x/gateway".to_owned()))
                .collect();
        let got = gateway_provider_overrides(&url_only);
        assert_eq!(
            got.iter()
                .find(|(k, _)| k == "model_providers.cctui.base_url")
                .map(|(_, v)| v.as_str()),
            Some("https://x/gateway")
        );
        assert!(got.contains(&("model_provider".to_owned(), "cctui".to_owned())));
        assert!(
            got.contains(&(
                "model_providers.cctui.env_key".to_owned(),
                "OPENAI_API_KEY".to_owned()
            )),
            "the bearer still comes from the launch env at request time"
        );
    }

    #[test]
    fn config_overrides_from_value() {
        let cfg = AppServerConfig::from_value(&json!({
            "codex_bin": "/opt/codex", "approval_policy": "never",
            "sandbox_mode": "danger-full-access",
        }));
        assert_eq!(cfg.bin, "/opt/codex");
        assert_eq!(cfg.approval_policy, "never");
        assert_eq!(cfg.sandbox_mode, "danger-full-access");
        // Default sandbox_mode is the safe, sandboxed mode.
        assert_eq!(AppServerConfig::default().sandbox_mode, "workspace-write");
    }

    #[test]
    fn model_catalog_toggle_defaults_on_and_reads_config() {
        assert!(AppServerConfig::default().model_catalog);
        assert!(AppServerConfig::from_value(&json!({})).model_catalog);
        assert!(!AppServerConfig::from_value(&json!({"model_catalog": false})).model_catalog);
    }

    #[test]
    fn config_overrides_carry_no_per_session_tier() {
        for cfg in [
            AppServerConfig::default(),
            AppServerConfig {
                reasoning_effort: Some("high".to_owned()),
                model: Some("gpt-5-codex".to_owned()),
                ..AppServerConfig::default()
            },
        ] {
            let overrides = cfg.config_overrides();
            let keys: Vec<&str> = overrides.iter().map(|(k, _)| k.as_str()).collect();
            assert!(
                !keys.iter().any(|k| k.to_lowercase().contains("fast")),
                "no fast-mode knob may be set, got {keys:?}"
            );
            // Only the four known, intentional knobs are ever set.
            for k in &keys {
                assert!(
                    matches!(
                        *k,
                        "approval_policy" | "sandbox_mode" | "model_reasoning_effort" | "model"
                    ),
                    "unexpected codex config knob {k:?}"
                );
            }
        }
    }

    #[test]
    fn config_overrides_default_and_with_quality_knobs() {
        let base = AppServerConfig::default().config_overrides();
        assert_eq!(base.len(), 2);
        assert!(base.contains(&("approval_policy".to_owned(), "on-request".to_owned())));
        assert!(base.contains(&("sandbox_mode".to_owned(), "workspace-write".to_owned())));

        let with = AppServerConfig {
            reasoning_effort: Some("high".to_owned()),
            model: Some("gpt-5-codex".to_owned()),
            ..AppServerConfig::default()
        }
        .config_overrides();
        assert!(with.contains(&("model_reasoning_effort".to_owned(), "high".to_owned())));
        assert!(with.contains(&("model".to_owned(), "gpt-5-codex".to_owned())));
    }

    fn env_with_block(block: &str) -> std::collections::BTreeMap<String, String> {
        let mut env = std::collections::BTreeMap::new();
        env.insert("OPENAI_BASE_URL".to_owned(), "https://gw/openai".to_owned());
        env.insert(cctui_proto::codex_config::CONFIG_TOML_ENV.to_owned(), block.to_owned());
        env
    }

    /// A curated setting stored on `account_providers.settings_json` must reach
    /// the launched process — as a `-c` flag with a correctly typed value.
    #[test]
    fn account_settings_reach_the_launch_command_line() {
        let block = cctui_proto::codex_config::render_block(&json!({
            "hide_agent_reasoning": true,
            "model_verbosity": "low",
            "model_context_window": 272_000,
        }))
        .expect("rendered");
        let got = launch_overrides(&AppServerConfig::default(), &env_with_block(&block));
        let flags: Vec<String> = got.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert!(flags.contains(&"hide_agent_reasoning=true".to_owned()), "{flags:?}");
        assert!(flags.contains(&"model_verbosity=\"low\"".to_owned()), "{flags:?}");
        assert!(flags.contains(&"model_context_window=272000".to_owned()), "{flags:?}");
        // The managed knobs still ride along, still quoted.
        assert!(flags.contains(&"approval_policy=\"on-request\"".to_owned()), "{flags:?}");
        assert!(flags.contains(&"model_provider=\"cctui\"".to_owned()), "{flags:?}");
    }

    /// Anything outside the curated set is dropped rather than forwarded — an
    /// unknown key fails app-server startup, and a managed one would move
    /// gateway routing or the permission posture.
    #[test]
    fn uncurated_and_managed_keys_never_reach_the_command_line() {
        let hostile = "model_provider = \"evil\"\napproval_policy = \"never\"\n\
                       sandbox_mode = \"danger-full-access\"\nservice_tier = \"fast\"\n\
                       disableBundledSkills = true\ntotallyNotAKey = 1";
        let got = launch_overrides(&AppServerConfig::default(), &env_with_block(hostile));
        assert!(!got.iter().any(|(k, _)| k == "service_tier" || k == "disableBundledSkills"));
        assert!(!got.iter().any(|(k, _)| k == "totallyNotAKey"));
        // The keys that collide with managed ones survive only with cctui's values.
        for (key, want) in [
            ("model_provider", "\"cctui\""),
            ("approval_policy", "\"on-request\""),
            ("sandbox_mode", "\"workspace-write\""),
        ] {
            let vals: Vec<&str> =
                got.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect();
            assert_eq!(vals, vec![want], "{key} must be cctui's alone");
        }
    }

    #[test]
    fn launch_overrides_are_unchanged_without_an_account_block() {
        let env = std::collections::BTreeMap::new();
        let got = launch_overrides(&AppServerConfig::default(), &env);
        assert_eq!(
            got,
            vec![
                ("approval_policy".to_owned(), "\"on-request\"".to_owned()),
                ("sandbox_mode".to_owned(), "\"workspace-write\"".to_owned()),
            ]
        );
    }

    #[test]
    fn the_shared_overlay_lets_no_account_key_move_managed_or_gateway_settings() {
        let cfg = AppServerConfig { model: Some("m".to_owned()), ..AppServerConfig::default() };
        let env: std::collections::BTreeMap<String, String> = std::iter::once((
            cctui_proto::codex_config::CONFIG_TOML_ENV.to_owned(),
            "model = \"evil\"\nmodel_provider = \"x\"\nmodel_verbosity = \"low\"\n".to_owned(),
        ))
        .collect();
        let overlay = shared_overlay(&cfg, &env);
        let models: Vec<_> = overlay.iter().filter(|(k, _)| k == "model").collect();
        assert_eq!(models, vec![&("model".to_owned(), "\"m\"".to_owned())]);
        assert!(overlay.iter().all(|(k, _)| k != "model_provider"));
    }

    #[test]
    fn only_sessions_a_shared_app_server_can_fully_serve_are_eligible() {
        let env = |pairs: &[(&str, &str)]| -> std::collections::BTreeMap<String, String> {
            pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
        };
        assert!(shared_eligible(&env(&[]), false));
        assert!(shared_eligible(&env(&[("OPENAI_BASE_URL", "u"), ("OPENAI_API_KEY", "k")]), false));
        assert!(!shared_eligible(&env(&[]), true), "the CctuiAgent relay is per process");
        assert!(
            !shared_eligible(&env(&[("OPENAI_BASE_URL", "u")]), false),
            "an env_key bearer would resolve against the shared daemon's env"
        );
        assert!(!shared_eligible(&env(&[("HTTPS_PROXY", "p")]), false));
    }

    /// The per-thread tier must never leak into the process-level `-c` flags.
    #[test]
    fn config_overrides_stay_free_of_the_tier_even_when_one_is_set() {
        let cfg =
            AppServerConfig { service_tier: Some("fast".to_owned()), ..AppServerConfig::default() };
        assert!(cfg.config_overrides().iter().all(|(k, _)| k != "service_tier"));
    }
}
