/// Refresh proactively once the access token is within this window of expiry.
pub const REFRESH_SKEW_SECS: i64 = 60;

pub const SESSION_TOKEN_TTL_HOURS_DEFAULT: i64 = 12;

pub fn ttl_hours_from(var: Option<String>) -> i64 {
    var.and_then(|v| v.parse::<i64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(SESSION_TOKEN_TTL_HOURS_DEFAULT)
}

pub fn session_token_ttl() -> chrono::Duration {
    chrono::Duration::hours(ttl_hours_from(std::env::var("CCTUI_SESSION_TOKEN_TTL_HOURS").ok()))
}

/// Anthropic Claude-Code OAuth token endpoint + client id. These are not stable
/// public APIs (caveat accepted in the ticket); overridable via env so we can
/// track upstream changes without a redeploy of code.
pub fn anthropic_token_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://console.anthropic.com/v1/oauth/token".into())
}
pub fn anthropic_client_id() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".into())
}
/// claude.ai authorize endpoint for the manual code-paste OAuth login.
/// Overridable so we can track upstream without a redeploy.
pub fn anthropic_authorize_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://claude.ai/oauth/authorize".into())
}
/// Redirect URI used for the manual code-paste flow — claude.ai displays the
/// `code#state` pair instead of redirecting. Must match what the token exchange
/// sends back.
pub fn anthropic_oauth_redirect_uri() -> String {
    "https://console.anthropic.com/oauth/code/callback".into()
}
pub fn anthropic_upstream() -> String {
    std::env::var("CCTUI_ANTHROPIC_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into())
}
/// OpenAI/Codex OAuth token endpoint. Codex's public client exchanges +
/// refreshes here with **form-encoded** bodies (unlike Anthropic's JSON).
/// Overridable via env to track upstream changes without a code redeploy.
pub fn openai_token_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/token".into())
}
/// Codex's public OAuth client id. Defaults to the well-known `codex` client
/// (`app_EMoamEEZ73f0CkXaXp7hrann`); overridable via env.
pub fn openai_client_id() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "app_EMoamEEZ73f0CkXaXp7hrann".into())
}
/// auth.openai.com authorize endpoint for the "Sign in with `ChatGPT`" login.
/// Overridable so we can track upstream without a redeploy.
pub fn openai_authorize_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/authorize".into())
}
/// Fixed redirect URI baked into Codex's public client — we can't point it at
/// our own host. The browser redirect to localhost:1455 fails to load; the
/// user copies the full URL from the address bar and pastes it back.
pub fn openai_oauth_redirect_uri() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:1455/auth/callback".into())
}
pub fn openai_upstream() -> String {
    // Codex ChatGPT-backed accounts talk to the chatgpt backend, NOT
    // api.openai.com (matches what the codex CLI + CLIProxyAPI do).
    std::env::var("CCTUI_OPENAI_UPSTREAM")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex".into())
}

/// Fireworks' OpenAI-compatible inference base. A provider row's `base_url`
/// still wins when set; this is the default upstream for the family.
pub fn fireworks_upstream() -> String {
    std::env::var("CCTUI_FIREWORKS_UPSTREAM")
        .unwrap_or_else(|_| "https://api.fireworks.ai/inference/v1".into())
}

/// Whether the response body must be teed, which forces the upstream call to be
/// made without `accept-encoding`. reqwest is built without decompression
/// features, so a gzip/zstd body reaches the tee as opaque bytes: Langfuse gets a
/// trace with no usage, and Fireworks gets no metered usage at all.
pub const fn tees_response(langfuse: bool, fireworks: bool) -> bool {
    langfuse || fireworks
}

/// Per-provider request-shaping settings for the `fireworks` family, resolved
/// over [`fireworks_default_settings`]. Applied by the gateway on the way
/// upstream so no worker needs to know them (and none can bypass them).
pub struct FireworksSettings {
    /// Injected as the request body's `context_length_exceeded_behavior`
    /// (Fireworks defaults to `truncate`, which silently loses prompt).
    /// `None` (settings key `null`) injects nothing.
    pub context_length_exceeded_behavior: Option<String>,
    /// Pin a conversation's requests to one replica so its prompt prefix stays
    /// cache-warm: the session id goes out as `user` + `x-session-affinity`.
    pub session_affinity: bool,
    /// Extra body keys merged in, none overriding what the client sent.
    pub extra_body: serde_json::Map<String, serde_json::Value>,
    /// Name of cctui's own API key as it appears in Fireworks' billing console.
    /// A Fireworks account is shared across keys and tenants, so without this
    /// there is no way to tell cctui's spend from anyone else's — unset disables
    /// billing reconciliation rather than importing the whole account's usage.
    pub billing_api_key_name: Option<String>,
}

/// Defaults for a new `fireworks` provider row. Stored as data on the row at
/// create so the accounts UI can edit every knob.
pub fn fireworks_default_settings() -> serde_json::Value {
    serde_json::json!({
        "context_length_exceeded_behavior": "error",
        "session_affinity": true,
        "extra_body": {},
        "billing_api_key_name": null,
    })
}

impl FireworksSettings {
    /// Resolve a stored `provider_settings` blob over the defaults; an absent or
    /// malformed blob yields the defaults.
    pub fn resolve(stored: Option<&serde_json::Value>) -> Self {
        let mut merged = fireworks_default_settings();
        if let Some(overlay) = stored.filter(|v| v.is_object()) {
            cctui_proto::util::deep_merge(&mut merged, overlay.clone());
        }
        Self {
            context_length_exceeded_behavior: merged
                .get("context_length_exceeded_behavior")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned),
            session_affinity: merged
                .get("session_affinity")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            extra_body: merged
                .get("extra_body")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default(),
            billing_api_key_name: merged
                .get("billing_api_key_name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        }
    }

    /// Apply the settings to a JSON request body. Every injection is
    /// "only if absent" — an explicit client value always wins.
    pub fn apply_body(&self, body: &mut serde_json::Value, session_id: Option<&str>) {
        let Some(obj) = body.as_object_mut() else { return };
        if let Some(behavior) = self.context_length_exceeded_behavior.as_ref() {
            obj.entry("context_length_exceeded_behavior")
                .or_insert_with(|| serde_json::Value::String(behavior.clone()));
        }
        for (k, v) in &self.extra_body {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if self.session_affinity
            && let Some(sid) = session_id
        {
            obj.entry("user").or_insert_with(|| serde_json::Value::String(sid.to_owned()));
        }
    }
}

/// Why the gateway refused to forward a request path.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum GatewayPathError {
    /// A `.`/`..` segment, raw or percent-encoded: it escapes the base path of
    /// an upstream whose base carries one (`…/backend-api/codex`).
    DotSegment,
    /// An empty segment (`//`), which some upstreams collapse and others treat
    /// as an absolute reset of the path.
    EmptySegment,
    /// Not a path this provider's harness calls.
    NotAllowed,
    /// The path is allowed, but not with this method.
    MethodNotAllowed,
}

impl std::fmt::Display for GatewayPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DotSegment => "contains a relative path segment",
            Self::EmptySegment => "contains an empty path segment",
            Self::NotAllowed => "is not an API path this gateway forwards",
            Self::MethodNotAllowed => "is not forwarded with this method",
        })
    }
}

/// One allowlist entry: the methods, the path, and whether subpaths match.
struct PathRule {
    methods: &'static [&'static str],
    path: &'static str,
    subpaths: bool,
}

const fn rule(methods: &'static [&'static str], path: &'static str, subpaths: bool) -> PathRule {
    PathRule { methods, path, subpaths }
}

/// The inference API surface each harness actually calls through its gateway
/// base url. Anything else is refused rather than forwarded with the account's
/// credential: the upstreams behind these routes also serve account
/// administration, and a worker holds only a session token.
fn rules(family: Family) -> &'static [PathRule] {
    const ANTHROPIC: &[PathRule] = &[
        rule(&["POST"], "/v1/messages", false),
        rule(&["POST"], "/v1/messages/count_tokens", false),
        rule(&["POST"], "/v1/complete", false),
        rule(&["GET"], "/v1/models", true),
    ];
    const OPENAI: &[PathRule] = &[
        rule(&["POST", "GET"], "/responses", true),
        rule(&["POST"], "/chat/completions", false),
        rule(&["POST"], "/completions", false),
        rule(&["GET"], "/models", true),
    ];
    const FIREWORKS: &[PathRule] = &[
        rule(&["POST"], "/chat/completions", false),
        rule(&["POST"], "/completions", false),
        rule(&["POST"], "/embeddings", false),
        rule(&["GET"], "/models", true),
    ];
    match family {
        Family::Anthropic => ANTHROPIC,
        Family::Openai => OPENAI,
        Family::Fireworks => FIREWORKS,
    }
}

/// `CCTUI_GATEWAY_EXTRA_PATHS`: break-glass additions as `METHOD:/prefix`
/// entries (`*` matches any method), should an upstream grow a path before this
/// build can be updated.
fn extra_paths() -> &'static [(String, String)] {
    static EXTRA: std::sync::LazyLock<Vec<(String, String)>> = std::sync::LazyLock::new(|| {
        std::env::var("CCTUI_GATEWAY_EXTRA_PATHS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|e| e.trim().split_once(':'))
            .map(|(m, p)| (m.trim().to_ascii_uppercase(), p.trim().to_owned()))
            .filter(|(_, p)| p.starts_with('/'))
            .collect()
    });
    EXTRA.as_slice()
}

/// Percent-decode a path for validation only. Invalid escapes are left as the
/// literal bytes they are, which is also how every upstream reads them.
fn percent_decode(tail: &str) -> String {
    let bytes = tail.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) =
                (bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex))
        {
            out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'_'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reject anything that could resolve to a different upstream path than it
/// reads as: dot segments and empty segments, raw or percent-encoded. A single
/// trailing slash is tolerated.
fn normalized_segments(tail: &str) -> Result<(), GatewayPathError> {
    let body = tail.strip_suffix('/').unwrap_or(tail);
    for seg in body.split('/').skip(1) {
        if seg.is_empty() {
            return Err(GatewayPathError::EmptySegment);
        }
        if seg == "." || seg == ".." {
            return Err(GatewayPathError::DotSegment);
        }
    }
    Ok(())
}

/// Whether the gateway may forward `method tail` to `family`'s upstream.
/// `tail` is the request path with the `/gateway/<family>` prefix already
/// stripped, exactly as it will be appended to the upstream base.
pub fn gateway_path_permitted(
    family: Family,
    method: &str,
    tail: &str,
) -> Result<(), GatewayPathError> {
    let unsafe_bytes = |p: &str| p.bytes().any(|b| b == b'\\' || b.is_ascii_control());
    if !tail.starts_with('/') || unsafe_bytes(tail) {
        return Err(GatewayPathError::NotAllowed);
    }
    normalized_segments(tail)?;
    let decoded = percent_decode(tail);
    if unsafe_bytes(&decoded) {
        return Err(GatewayPathError::NotAllowed);
    }
    normalized_segments(&decoded)?;

    let method = method.to_ascii_uppercase();
    let path = decoded.strip_suffix('/').unwrap_or(&decoded);
    let matches = |rule_path: &str, subpaths: bool| {
        path == rule_path || (subpaths && path.starts_with(&format!("{rule_path}/")))
    };
    if extra_paths().iter().any(|(m, p)| (m.as_str() == "*" || *m == method) && matches(p, true)) {
        return Ok(());
    }
    let mut path_known = false;
    for r in rules(family) {
        if matches(r.path, r.subpaths) {
            if r.methods.contains(&method.as_str()) {
                return Ok(());
            }
            path_known = true;
        }
    }
    Err(if path_known { GatewayPathError::MethodNotAllowed } else { GatewayPathError::NotAllowed })
}

/// The provider *family* of an account: which env vars it drives, and the key
/// `UNIQUE (account_id, family)` enforces one credential per. `fireworks` is its
/// own family — despite the `OpenAI` wire protocol — so a Fireworks key can sit
/// next to a codex credential on one account.
///
/// [`label`](Self::label) is the stored value of the generated `family` column;
/// per-family SQL predicates compare against it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Anthropic,
    Openai,
    Fireworks,
}

impl Family {
    /// Derive the family from a stored `provider` value. Must agree with the
    /// generated `family` column (migration 078).
    pub fn from_provider(provider: &str) -> Self {
        if provider == "fireworks" {
            Self::Fireworks
        } else if provider.contains("openai") {
            Self::Openai
        } else {
            Self::Anthropic
        }
    }
    /// Parse a family label back (the `family` column / API `family` field).
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::Openai),
            "fireworks" => Some(Self::Fireworks),
            _ => None,
        }
    }
    /// Derive the family from a spawn adapter id (`opencode*` → fireworks,
    /// `codex*` → openai, `claude*` → anthropic). This IS the spawn resolution
    /// key: the adapter names the harness family, and the account identity
    /// carries at most one provider row per family.
    ///
    /// Fail-closed: an adapter id that names no known harness yields `None`
    /// rather than silently binding an Anthropic credential to it.
    pub fn try_from_adapter(adapter_id: &str) -> Option<Self> {
        let id = adapter_id.trim();
        if id.starts_with("opencode") {
            Some(Self::Fireworks)
        } else if id.starts_with("codex") {
            Some(Self::Openai)
        } else if id.starts_with("claude") {
            Some(Self::Anthropic)
        } else {
            None
        }
    }

    /// [`try_from_adapter`](Self::try_from_adapter) with the historical
    /// anthropic fallback. Callers that can reject the request should use
    /// `try_from_adapter` instead.
    pub fn from_adapter(adapter_id: &str) -> Self {
        Self::try_from_adapter(adapter_id).unwrap_or_else(|| {
            tracing::warn!(
                adapter = adapter_id,
                "unknown harness family for adapter; defaulting to anthropic"
            );
            Self::Anthropic
        })
    }
    /// Human label for error messages, and the stored `family` column value.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Fireworks => "fireworks",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Family, GatewayPathError, gateway_path_permitted as permitted};

    #[test]
    fn the_paths_each_harness_calls_are_forwarded() {
        for (family, method, tail) in [
            (Family::Anthropic, "POST", "/v1/messages"),
            (Family::Anthropic, "POST", "/v1/messages/count_tokens"),
            (Family::Anthropic, "GET", "/v1/models"),
            (Family::Anthropic, "GET", "/v1/models/claude-opus-5"),
            (Family::Openai, "POST", "/responses"),
            (Family::Openai, "POST", "/responses/compact"),
            (Family::Openai, "GET", "/responses"),
            (Family::Openai, "GET", "/models"),
            (Family::Openai, "POST", "/chat/completions"),
            (Family::Fireworks, "POST", "/chat/completions"),
            (Family::Fireworks, "GET", "/models"),
        ] {
            permitted(family, method, tail).unwrap_or_else(|e| panic!("{method} {tail}: {e}"));
        }
        permitted(Family::Anthropic, "post", "/v1/messages").unwrap();
        permitted(Family::Anthropic, "POST", "/v1/messages/").unwrap();
    }

    #[test]
    fn dot_segments_cannot_escape_the_upstream_base_path() {
        for tail in [
            "/../v1/organizations",
            "/responses/../../me",
            "/%2e%2e/me",
            "/%2E%2E/me",
            "/responses/%2e%2e%2fme",
            "/./responses",
            "/%2e/responses",
        ] {
            assert_eq!(
                permitted(Family::Openai, "POST", tail),
                Err(GatewayPathError::DotSegment),
                "{tail} must be refused as a dot segment"
            );
        }
    }

    #[test]
    fn empty_segments_are_refused() {
        for tail in ["//me", "/responses//x", "/%2f%2fme"] {
            assert_eq!(
                permitted(Family::Openai, "POST", tail),
                Err(GatewayPathError::EmptySegment),
                "{tail}"
            );
        }
    }

    #[test]
    fn a_known_path_with_the_wrong_method_is_refused() {
        assert_eq!(
            permitted(Family::Anthropic, "DELETE", "/v1/messages"),
            Err(GatewayPathError::MethodNotAllowed)
        );
        assert_eq!(
            permitted(Family::Anthropic, "GET", "/v1/messages"),
            Err(GatewayPathError::MethodNotAllowed)
        );
        assert_eq!(
            permitted(Family::Openai, "DELETE", "/responses/resp_1"),
            Err(GatewayPathError::MethodNotAllowed)
        );
    }

    #[test]
    fn administrative_and_cross_family_paths_are_not_forwarded() {
        for (family, method, tail) in [
            (Family::Openai, "GET", "/me"),
            (Family::Openai, "GET", "/accounts"),
            (Family::Anthropic, "GET", "/api/oauth/usage"),
            (Family::Anthropic, "POST", "/v1/organizations/x/invites"),
            (Family::Anthropic, "POST", "/responses"),
            (Family::Anthropic, "GET", "/v1/files"),
            (Family::Fireworks, "POST", "/v1/messages"),
        ] {
            assert_eq!(
                permitted(family, method, tail),
                Err(GatewayPathError::NotAllowed),
                "{method} {tail}"
            );
        }
    }

    #[test]
    fn a_malformed_or_control_laden_tail_is_refused() {
        for tail in [
            "v1/messages",
            "/v1/\u{0}messages",
            "/v1\\messages",
            "/v1/messages%5c..%5cx",
            "/v1/messages%00",
        ] {
            assert_eq!(
                permitted(Family::Anthropic, "POST", tail),
                Err(GatewayPathError::NotAllowed)
            );
        }
    }

    #[test]
    fn an_unknown_adapter_has_no_family() {
        assert_eq!(Family::try_from_adapter("claude-code"), Some(Family::Anthropic));
        assert_eq!(Family::try_from_adapter("codex-app-server"), Some(Family::Openai));
        assert_eq!(Family::try_from_adapter("opencode"), Some(Family::Fireworks));
        for unknown in ["", "gemini", "aider", "cursor", "anthropic"] {
            assert_eq!(Family::try_from_adapter(unknown), None, "{unknown}");
        }
    }
}
