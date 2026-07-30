use std::env;

/// One external dispatcher registration, parsed from `CCTUI_HTTP_DISPATCHERS`
/// (a JSON array of these). `token`, when set, is sent as a bearer secret.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HttpDispatcherConfig {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
}

/// Parse `CCTUI_DISPATCHERS` (a JSON array of `{ "kind":..,.. }`)
/// into the http registrations the server still honors. Only `kind:"http"`
/// entries are kept; any other kind (the retired in-process `kube`/`docker`
/// dispatchers) is skipped with a warning rather than failing the
/// parse — prod still ships a stale `kind:"kube"` entry, and a hard error here
/// would crash-loop it. Returns `[]` on malformed input.
fn parse_dispatchers(raw: &str) -> Vec<HttpDispatcherConfig> {
    let entries: Vec<serde_json::Value> = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("CCTUI_DISPATCHERS is not a JSON array, ignoring: {e}");
            return Vec::new();
        }
    };
    entries
        .into_iter()
        .filter_map(|v| {
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("http");
            if kind != "http" {
                let id = v.get("id").and_then(|i| i.as_str()).unwrap_or("?");
                tracing::warn!(id, kind, "CCTUI_DISPATCHERS entry skipped: in-process kube/docker dispatchers are unsupported; use an enrolled dispatcher");
                return None;
            }
            match serde_json::from_value::<HttpDispatcherConfig>(v) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!("CCTUI_DISPATCHERS http entry skipped (bad shape): {e}");
                    None
                }
            }
        })
        .collect()
}

/// One operator-declared, self-hosted Claude model, parsed from
/// `CCTUI_CLAUDE_LITELLM_MODELS` (a JSON array of these). `model` is the code
/// passed to `claude --model` (and the name a litellm endpoint routes on);
/// `label` is the free-form display name shown in the spawn picker.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LiteLlmModel {
    pub model: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub external_url: String,
    /// Browser origins allowed to make credentialed (cookie) cross-origin
    /// requests and to open the user WebSocket. Defaults to the server's own
    /// `external_url` plus the local Vite dev origins; extend via
    /// `CCTUI_ALLOWED_ORIGINS` (comma-separated). A credentialed CORS response
    /// must never use `*`, so this is an explicit list.
    pub allowed_origins: Vec<String>,
    /// How long a session may sit without activity before the reaper
    /// demotes it from `Active` to `Inactive`. The old
    /// `CCTUI_HEARTBEAT_TIMEOUT` env var is still accepted for
    /// back-compat with existing deployments.
    pub inactive_after_secs: u64,
    /// How long a session may sit without a heartbeat before the reaper
    /// auto-archives it (hides it from the default list). Configured in
    /// hours via `CCTUI_SESSION_ARCHIVE_TTL_HOURS`; stored as seconds.
    /// `0` disables auto-archiving.
    pub archive_after_secs: u64,
    /// Optional GitHub PAT (read access to the releases repo). When set, the
    /// daemon-binary manifest points clients at this server's proxy endpoint
    /// and the server streams the release asset itself (so a private releases
    /// repo stays private and clients never need a token). When unset, the
    /// manifest falls back to the raw GitHub release URLs and the proxy
    /// 302-redirects there — letting selfupdate degrade gracefully (it fails
    /// for a private repo, which is the intended no-op until a token is set).
    pub github_token: Option<String>,
    /// External dispatcher registrations, parsed from `CCTUI_HTTP_DISPATCHERS`.
    pub http_dispatchers: Vec<HttpDispatcherConfig>,
    /// Http dispatcher registrations parsed from `CCTUI_DISPATCHERS`,
    /// filtered to `kind:"http"` only (retired the in-process variants).
    /// Merged with `http_dispatchers` at startup.
    pub dispatchers: Vec<HttpDispatcherConfig>,
    /// How long an `ephemeral` (dispatch/worker) machine may go without being
    /// seen before the reaper soft-deletes it — covers pods that die before
    /// self-deenroll. Configured in hours via
    /// `CCTUI_EPHEMERAL_MACHINE_TTL_HOURS`; stored as seconds. `0` disables the
    /// sweep. Persistent machines are never reaped.
    pub ephemeral_machine_ttl_secs: u64,
    /// ntfy access token (`CCTUI_NTFY_TOKEN`, provisioned from vault). Its
    /// presence is the on/off switch for dispatch push notifications: when
    /// unset, `ntfy::notify` is a no-op.
    pub ntfy_token: Option<String>,
    /// ntfy topic URL to POST notifications to (`CCTUI_NTFY_URL`, a full topic
    /// URL, e.g. `https://ntfy.example.com/cctui-dispatch`). No default: when
    /// unset, push notifications stay off.
    pub ntfy_url: Option<String>,
    /// Base URL of a self-hosted, Anthropic-compatible endpoint (e.g. a `LiteLLM`
    /// proxy in front of Ollama) dedicated to Claude Code. When set together
    /// with `claude_litellm_models`, the listed models become selectable in the
    /// spawn picker and a session launched under one of them gets
    /// `ANTHROPIC_BASE_URL` (and the token below) injected so `claude` routes
    /// here instead of the real Anthropic API. From `CCTUI_CLAUDE_LITELLM_ENDPOINT`.
    pub claude_litellm_endpoint: Option<String>,
    /// Bearer token injected as `ANTHROPIC_AUTH_TOKEN` for the custom endpoint
    /// (`CCTUI_CLAUDE_LITELLM_TOKEN`). Optional: an open proxy accepts any value,
    /// so when unset we inject a dummy. Never exposed to clients.
    pub claude_litellm_token: Option<String>,
    /// Operator-declared self-hosted Claude models (`CCTUI_CLAUDE_LITELLM_MODELS`,
    /// JSON array of `{model,label}`). Only surfaced to clients when
    /// `claude_litellm_endpoint` is also set — see [`Config::claude_litellm_visible_models`].
    pub claude_litellm_models: Vec<LiteLlmModel>,
}

fn add_origin(out: &mut Vec<String>, origin: &str) {
    let origin = origin.trim().trim_end_matches('/');
    if !origin.is_empty() && !out.iter().any(|e| e == origin) {
        out.push(origin.to_owned());
    }
}

/// Build the allowed-origin list: the server's own public URL and the local
/// Vite dev origins are always present so existing deploys keep working, then
/// any comma-separated `CCTUI_ALLOWED_ORIGINS` entries are added.
fn parse_allowed_origins(raw: Option<&str>, external_url: &str) -> Vec<String> {
    let mut out = Vec::new();
    add_origin(&mut out, external_url);
    add_origin(&mut out, "http://localhost:5173");
    add_origin(&mut out, "http://127.0.0.1:5173");
    if let Some(raw) = raw {
        for entry in raw.split(',') {
            add_origin(&mut out, entry);
        }
    }
    out
}

impl Config {
    /// Whether `origin` (an `Origin` header value) is in the CORS/WS allowlist.
    #[must_use]
    pub fn origin_allowed(&self, origin: &str) -> bool {
        let origin = origin.trim_end_matches('/');
        self.allowed_origins.iter().any(|o| o == origin)
    }

    pub fn from_env() -> Self {
        let external_url =
            env::var("CCTUI_EXTERNAL_URL").unwrap_or_else(|_| "http://localhost:8700".into());
        let allowed_origins =
            parse_allowed_origins(env::var("CCTUI_ALLOWED_ORIGINS").ok().as_deref(), &external_url);
        Self {
            host: env::var("CCTUI_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("CCTUI_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8700),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            external_url,
            allowed_origins,
            inactive_after_secs: env::var("CCTUI_INACTIVE_AFTER")
                .or_else(|_| env::var("CCTUI_HEARTBEAT_TIMEOUT"))
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            archive_after_secs: env::var("CCTUI_SESSION_ARCHIVE_TTL_HOURS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(24 * 60 * 60, |hours| hours * 60 * 60),
            github_token: env::var("CCTUI_GITHUB_TOKEN")
                .or_else(|_| env::var("GH_TOKEN"))
                .ok()
                .filter(|s| !s.trim().is_empty()),
            http_dispatchers: env::var("CCTUI_HTTP_DISPATCHERS")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    serde_json::from_str(&s).expect("CCTUI_HTTP_DISPATCHERS must be a JSON array")
                })
                .unwrap_or_default(),
            dispatchers: env::var("CCTUI_DISPATCHERS")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| parse_dispatchers(&s))
                .unwrap_or_default(),
            ephemeral_machine_ttl_secs: env::var("CCTUI_EPHEMERAL_MACHINE_TTL_HOURS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(2 * 60 * 60, |hours| hours * 60 * 60),
            ntfy_token: env::var("CCTUI_NTFY_TOKEN").ok().filter(|s| !s.trim().is_empty()),
            ntfy_url: env::var("CCTUI_NTFY_URL").ok().filter(|s| !s.trim().is_empty()),
            claude_litellm_endpoint: env::var("CCTUI_CLAUDE_LITELLM_ENDPOINT")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            claude_litellm_token: env::var("CCTUI_CLAUDE_LITELLM_TOKEN")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            claude_litellm_models: env::var("CCTUI_CLAUDE_LITELLM_MODELS")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    serde_json::from_str(&s)
                        .expect("CCTUI_CLAUDE_LITELLM_MODELS must be a JSON array of {model,label}")
                })
                .unwrap_or_default(),
        }
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// The custom Claude models to surface to clients. Non-empty **only when
    /// both** the endpoint and the model list are configured — that conjunction
    /// is the feature gate. When either is missing the feature stays dark and
    /// the spawn picker shows only the native families.
    pub fn claude_litellm_visible_models(&self) -> &[LiteLlmModel] {
        if self.claude_litellm_endpoint.is_some() && !self.claude_litellm_models.is_empty() {
            &self.claude_litellm_models
        } else {
            &[]
        }
    }

    pub fn admin_tokens() -> Vec<String> {
        env::var("CCTUI_ADMIN_TOKENS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect()
    }
}

#[cfg(test)]
impl Config {
    /// Minimal `Config` for tests that only exercise the origin allowlist.
    #[must_use]
    pub fn for_test(allowed_origins: Vec<String>) -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 8700,
            database_url: String::new(),
            external_url: String::new(),
            allowed_origins,
            inactive_after_secs: 0,
            archive_after_secs: 0,
            github_token: None,
            http_dispatchers: vec![],
            dispatchers: vec![],
            ephemeral_machine_ttl_secs: 0,
            ntfy_token: None,
            ntfy_url: None,
            claude_litellm_endpoint: None,
            claude_litellm_token: None,
            claude_litellm_models: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_origins_default_to_same_origin_plus_dev() {
        let origins = parse_allowed_origins(None, "https://cctui.example.com/");
        assert_eq!(
            origins,
            vec![
                "https://cctui.example.com".to_owned(),
                "http://localhost:5173".to_owned(),
                "http://127.0.0.1:5173".to_owned(),
            ]
        );
    }

    #[test]
    fn allowed_origins_appends_and_dedups_env_entries() {
        let origins = parse_allowed_origins(
            Some("https://extra.example.com/, http://localhost:5173 ,"),
            "https://cctui.example.com",
        );
        assert_eq!(
            origins,
            vec![
                "https://cctui.example.com".to_owned(),
                "http://localhost:5173".to_owned(),
                "http://127.0.0.1:5173".to_owned(),
                "https://extra.example.com".to_owned(),
            ]
        );
    }

    #[test]
    fn origin_allowed_matches_ignoring_trailing_slash() {
        let cfg = Config::for_test(vec!["https://cctui.example.com".to_owned()]);
        assert!(cfg.origin_allowed("https://cctui.example.com"));
        assert!(cfg.origin_allowed("https://cctui.example.com/"));
        assert!(!cfg.origin_allowed("https://evil.example.com"));
    }

    /// the in-process `kube`/`docker` dispatchers are gone, but prod
    /// still ships a stale `kind:"kube"` entry in `CCTUI_DISPATCHERS`. The parse
    /// must skip non-http kinds without panicking and keep the `http`
    /// escape-hatch entries.
    #[test]
    fn cctui_dispatchers_skips_kube_docker_keeps_http() {
        let raw = r#"[
            {"kind":"kube","id":"claude-worker","namespace":"ai","source_cronjob":"claude-worker-base","cctui_url":"http://x:8700"},
            {"kind":"docker","id":"docker-worker","image":"worker:latest"},
            {"kind":"http","id":"ext","url":"http://x:9000"}
        ]"#;
        let parsed = parse_dispatchers(raw);
        assert_eq!(parsed.len(), 1, "only the http entry survives");
        assert_eq!(parsed[0].id, "ext");
        assert_eq!(parsed[0].url, "http://x:9000");
    }

    /// A `CCTUI_DISPATCHERS` entry with no `kind` defaults to http (back-compat
    /// with plain `{id,url}` shapes); malformed JSON yields an empty list rather
    /// than a panic.
    #[test]
    fn cctui_dispatchers_defaults_kind_http_and_tolerates_garbage() {
        assert_eq!(parse_dispatchers(r#"[{"id":"a","url":"http://y"}]"#).len(), 1);
        assert!(parse_dispatchers("not json").is_empty());
    }

    /// `CCTUI_CLAUDE_LITELLM_MODELS` is a JSON array of `{model,label}`; the
    /// label is free-form (spaces, parens) so JSON — not a delimited string —
    /// is the format. Assert it parses.
    #[test]
    fn claude_litellm_models_parse() {
        let raw = r#"[{"model":"qwen3-coder","label":"Qwen3-Coder (local)"}]"#;
        let parsed: Vec<LiteLlmModel> =
            serde_json::from_str(raw).expect("CCTUI_CLAUDE_LITELLM_MODELS must parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].model, "qwen3-coder");
        assert_eq!(parsed[0].label, "Qwen3-Coder (local)");
    }

    /// The shim is gated on BOTH the endpoint and the model list: with
    /// only one set, nothing is surfaced (`claude_litellm_visible_models` empty),
    /// so the back-compat shim is a no-op.
    #[test]
    fn claude_litellm_gating() {
        let base = Config {
            host: "0.0.0.0".into(),
            port: 8700,
            database_url: String::new(),
            external_url: String::new(),
            allowed_origins: vec![],
            inactive_after_secs: 0,
            archive_after_secs: 0,
            github_token: None,
            http_dispatchers: vec![],
            dispatchers: vec![],
            ephemeral_machine_ttl_secs: 0,
            ntfy_token: None,
            ntfy_url: None,
            claude_litellm_endpoint: None,
            claude_litellm_token: None,
            claude_litellm_models: vec![LiteLlmModel {
                model: "qwen3-coder".into(),
                label: "Qwen3-Coder".into(),
            }],
        };

        // Models set but endpoint missing → dark.
        assert!(base.claude_litellm_visible_models().is_empty());

        // Both set → surfaced (the shim then synthesizes managed accounts).
        let cfg = Config {
            claude_litellm_endpoint: Some("https://litellm.example/".into()),
            claude_litellm_token: Some("tok".into()),
            ..base
        };
        assert_eq!(cfg.claude_litellm_visible_models().len(), 1);
    }
}
