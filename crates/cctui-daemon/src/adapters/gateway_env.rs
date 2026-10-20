//! Shared launch-time gateway-env resolution for every adapter.

use std::collections::BTreeMap;

use cctui_proto::api::GatewayEnvResponse;

use crate::client::ServerClient;

/// Anthropic (claude) gateway routing keys.
pub const CLAUDE_GATEWAY_KEYS: &[&str] = &["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"];
/// `OpenAI` (codex) gateway routing keys.
pub const OPENAI_GATEWAY_KEYS: &[&str] = &["OPENAI_BASE_URL", "OPENAI_API_KEY"];
/// Fireworks (opencode) gateway routing keys.
pub const FIREWORKS_GATEWAY_KEYS: &[&str] = &["FIREWORKS_BASE_URL", "FIREWORKS_API_KEY"];
/// Google (gemini) gateway routing keys.
pub const GOOGLE_GATEWAY_KEYS: &[&str] = &["GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY"];

/// How one ACP agent reads the routing keys its family's gateway env mints.
///
/// The server mints one key pair per family (`OPENAI_BASE_URL` +
/// `OPENAI_API_KEY` for the `OpenAI` family). An agent that spells them
/// differently gets a copy under its own names, plus the fixed env that
/// selects the provider on its side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayAliases {
    pub agent: &'static str,
    /// The family's routing keys, all required for a launch.
    pub required: &'static [&'static str],
    /// `(family key, agent key)` copies.
    pub aliases: &'static [(&'static str, &'static str)],
    /// Env the binding needs beyond the routing keys, inserted only when not
    /// already set.
    pub fixed: &'static [(&'static str, &'static str)],
}

/// qwen-code reads the `OpenAI` SDK names as they are; `OPENAI_API_KEY` in the
/// env is what selects its `openai` auth type.
pub const QWEN_GATEWAY: GatewayAliases =
    GatewayAliases { agent: "qwen", required: OPENAI_GATEWAY_KEYS, aliases: &[], fixed: &[] };

/// goose's openai provider is picked by `GOOSE_PROVIDER` and addressed as a
/// host plus a base path; the gateway base already carries the family prefix,
/// so the path is the bare chat-completions route.
pub const GOOSE_GATEWAY: GatewayAliases = GatewayAliases {
    agent: "goose",
    required: OPENAI_GATEWAY_KEYS,
    aliases: &[("OPENAI_BASE_URL", "OPENAI_HOST")],
    fixed: &[("GOOSE_PROVIDER", "openai"), ("OPENAI_BASE_PATH", "chat/completions")],
};

/// gemini-cli reads the family's keys as minted.
///
/// `GOOGLE_GEMINI_BASE_URL` in the env is what selects its `gateway` auth
/// type, and `GEMINI_API_KEY` is the key that type sends as `x-goog-api-key`
/// (gemini-cli 0.64, `packages/core/src/core/contentGenerator.ts`).
pub const GEMINI_GATEWAY: GatewayAliases =
    GatewayAliases { agent: "gemini", required: GOOGLE_GATEWAY_KEYS, aliases: &[], fixed: &[] };

/// Every ACP agent bound through a gateway family.
pub const GATEWAY_ALIASES: &[GatewayAliases] = &[GEMINI_GATEWAY, QWEN_GATEWAY, GOOSE_GATEWAY];

#[must_use]
pub fn gateway_aliases(agent: &str) -> Option<&'static GatewayAliases> {
    GATEWAY_ALIASES.iter().find(|a| a.agent == agent)
}

/// The routing keys an account-bound launch of `agent` must hold; empty for
/// an agent with no gateway binding, which never fails closed on a partial env.
#[must_use]
pub fn required_gateway_keys(agent: &str) -> &'static [&'static str] {
    gateway_aliases(agent).map_or(&[], |a| a.required)
}

impl GatewayAliases {
    /// Copy the family's keys under the agent's names and add the fixed env.
    /// A hint that already names a key keeps its value.
    pub fn apply(&self, env: &mut BTreeMap<String, String>) {
        for (family_key, agent_key) in self.aliases {
            if let Some(value) = env.get(*family_key).cloned() {
                env.entry((*agent_key).to_owned()).or_insert(value);
            }
        }
        for (key, value) in self.fixed {
            env.entry((*key).to_owned()).or_insert_with(|| (*value).to_owned());
        }
    }
}

/// Decide a worker's launch env from the server's `GatewayEnvResponse`.
///
/// Fail-closed for an account-bound session: refuse the launch when the resolved
/// env is empty *or* lacks any of `required_keys` (a half-routed worker 401s).
/// Otherwise gateway env is merged OVER `hint` (its routing keys win, other hint
/// entries survive). An unbound session keeps `hint` and never fails closed.
pub fn launch_env_decision(
    adapter: &str,
    local_id: &str,
    resp: &GatewayEnvResponse,
    hint: &BTreeMap<String, String>,
    required_keys: &[&str],
) -> anyhow::Result<BTreeMap<String, String>> {
    if !resp.account_bound {
        return Ok(hint.clone());
    }
    if resp.env.is_empty() {
        tracing::error!(
            %local_id,
            adapter,
            "🔴 GATEWAY-ENV MISSING: account-bound session but the server returned no gateway env \
             (account missing/unmintable). Refusing to launch; check the session's account \
             binding and the server's token mint."
        );
        anyhow::bail!(
            "refusing to launch {adapter} {local_id}: session is account-bound but the server \
             returned no gateway env (account missing/unmintable) — launching would route to the \
             default upstream and 401"
        );
    }
    let mut merged = hint.clone();
    merged.extend(resp.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    let missing: Vec<&str> =
        required_keys.iter().copied().filter(|k| !merged.contains_key(*k)).collect();
    if !missing.is_empty() {
        let missing = missing.join(" + ");
        tracing::error!(
            %local_id,
            adapter,
            %missing,
            "🔴 GATEWAY-ENV PARTIAL: account-bound session resolved a gateway env missing routing \
             keys — the worker would route unauthenticated and 401. Refusing to launch."
        );
        anyhow::bail!(
            "refusing to launch {adapter} {local_id}: account-bound session resolved a gateway \
             env missing {missing} — launching would route unauthenticated and 401"
        );
    }
    if let Some(aliases) = gateway_aliases(adapter) {
        aliases.apply(&mut merged);
    }
    Ok(merged)
}

/// Pull the launch-time gateway env for `local_id` from the server's durable
/// `sessions.account_id` binding and apply [`launch_env_decision`] over `hint`.
///
/// Degrades to `hint` when no server is configured (tests / legacy) or the pull
/// is unavailable (older server / transient network) — a rollout or blip falls
/// back to the pushed env rather than blocking the launch. The fail-closed
/// refusal fires only when the authoritative pull SUCCEEDS and reports the
/// binding.
pub async fn resolve_env(
    adapter: &str,
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    hint: &BTreeMap<String, String>,
    required_keys: &[&str],
) -> anyhow::Result<BTreeMap<String, String>> {
    resolve_launch(adapter, server, machine_key, local_id, hint, required_keys).await.map(|l| l.env)
}

/// `settings`, `spawn_capability` and `plugins` are empty/`None` whenever the
/// env came from `hint` rather than a successful pull.
#[derive(Debug, Default, Clone)]
pub struct LaunchEnv {
    pub env: BTreeMap<String, String>,
    pub settings: Option<serde_json::Value>,
    pub spawn_capability: Option<cctui_proto::api::SpawnCapability>,
    /// Runtime plugins the session owner enabled, each with the skill files to
    /// mirror and the env to export.
    pub plugins: Vec<cctui_proto::api::SessionPlugin>,
    /// Memory notes and the prompt template the server resolved for this
    /// session, already scoped. Rendered into the neutral launch preamble.
    pub context: Vec<cctui_proto::api::SessionContextItem>,
}

pub async fn resolve_launch(
    adapter: &str,
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    hint: &BTreeMap<String, String>,
    required_keys: &[&str],
) -> anyhow::Result<LaunchEnv> {
    let (Some(server), Some(mk)) = (server, machine_key) else {
        return Ok(LaunchEnv { env: hint.clone(), ..LaunchEnv::default() });
    };
    match server.gateway_env(mk, local_id).await {
        Ok(resp) => {
            let settings = resp.settings.clone();
            let spawn_capability = resp.spawn_capability.clone();
            let plugins = resp.plugins.clone();
            let context = resp.context.clone();
            let env = launch_env_decision(adapter, local_id, &resp, hint, required_keys)?;
            Ok(LaunchEnv { env, settings, spawn_capability, plugins, context })
        }
        Err(e) => {
            tracing::warn!(
                %local_id,
                adapter,
                "gateway-env pull failed; falling back to pushed env: {e}"
            );
            Ok(LaunchEnv { env: hint.clone(), ..LaunchEnv::default() })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cctui_proto::api::GatewayEnvResponse;

    use super::{
        CLAUDE_GATEWAY_KEYS, FIREWORKS_GATEWAY_KEYS, GATEWAY_ALIASES, GOOGLE_GATEWAY_KEYS,
        OPENAI_GATEWAY_KEYS, launch_env_decision, required_gateway_keys, resolve_env,
    };

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn bound(env: &[(&str, &str)]) -> GatewayEnvResponse {
        GatewayEnvResponse { account_bound: true, env: env_of(env), ..Default::default() }
    }

    #[test]
    fn merges_gateway_env_over_hint_when_account_bound() {
        let resp = bound(&[("ANTHROPIC_BASE_URL", "gw"), ("ANTHROPIC_AUTH_TOKEN", "tok")]);
        let hint = env_of(&[("FOO", "bar"), ("ANTHROPIC_BASE_URL", "stale")]);
        let got = launch_env_decision("claude", "s1", &resp, &hint, CLAUDE_GATEWAY_KEYS).unwrap();
        assert_eq!(got.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(got.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("gw"));
        assert_eq!(got.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str), Some("tok"));
    }

    #[test]
    fn fails_closed_when_account_bound_but_env_empty() {
        let resp = GatewayEnvResponse { account_bound: true, ..Default::default() };
        let err = launch_env_decision(
            "claude",
            "s1",
            &resp,
            &env_of(&[("HINT", "1")]),
            CLAUDE_GATEWAY_KEYS,
        )
        .unwrap_err();
        assert!(err.to_string().contains("account-bound"), "got: {err}");
    }

    #[test]
    fn keeps_hint_when_not_account_bound() {
        let resp = GatewayEnvResponse { account_bound: false, ..Default::default() };
        let hint = env_of(&[("FOO", "bar")]);
        assert_eq!(
            launch_env_decision("codex", "s1", &resp, &hint, OPENAI_GATEWAY_KEYS).unwrap(),
            hint
        );
    }

    #[test]
    fn codex_fails_closed_on_partial_env() {
        let resp = bound(&[("OPENAI_BASE_URL", "gw")]);
        let err = launch_env_decision("codex", "s1", &resp, &BTreeMap::new(), OPENAI_GATEWAY_KEYS)
            .unwrap_err();
        assert!(err.to_string().contains("account-bound"), "got: {err}");
        assert!(err.to_string().contains("OPENAI_API_KEY"), "got: {err}");
    }

    #[test]
    fn opencode_fails_closed_on_partial_env() {
        let resp = bound(&[("FIREWORKS_API_KEY", "tok")]);
        let err =
            launch_env_decision("opencode", "s1", &resp, &BTreeMap::new(), FIREWORKS_GATEWAY_KEYS)
                .unwrap_err();
        assert!(err.to_string().contains("account-bound"), "got: {err}");
        assert!(err.to_string().contains("FIREWORKS_BASE_URL"), "got: {err}");
    }

    #[test]
    fn claude_fails_closed_on_partial_env() {
        let resp = bound(&[("ANTHROPIC_BASE_URL", "gw")]);
        let err = launch_env_decision("claude", "s1", &resp, &BTreeMap::new(), CLAUDE_GATEWAY_KEYS)
            .unwrap_err();
        assert!(err.to_string().contains("ANTHROPIC_AUTH_TOKEN"), "got: {err}");
    }

    /// An account-bound qwen launch carries the `OpenAI` SDK names as minted;
    /// a goose launch gets its provider selector, host and base path derived
    /// from the same pair. Either refuses an empty or half-minted env.
    #[test]
    fn qwen_takes_the_openai_pair_as_minted() {
        let resp =
            bound(&[("OPENAI_BASE_URL", "https://gw/gateway/openai"), ("OPENAI_API_KEY", "tok")]);
        let hint = env_of(&[("QWEN_LOG", "1")]);
        let got =
            launch_env_decision("qwen", "s1", &resp, &hint, required_gateway_keys("qwen")).unwrap();
        assert_eq!(
            got.get("OPENAI_BASE_URL").map(String::as_str),
            Some("https://gw/gateway/openai")
        );
        assert_eq!(got.get("OPENAI_API_KEY").map(String::as_str), Some("tok"));
        assert_eq!(got.get("QWEN_LOG").map(String::as_str), Some("1"));
        assert!(!got.contains_key("GOOSE_PROVIDER"));
    }

    #[test]
    fn goose_gets_its_provider_host_and_base_path_from_the_openai_pair() {
        let resp =
            bound(&[("OPENAI_BASE_URL", "https://gw/gateway/openai"), ("OPENAI_API_KEY", "tok")]);
        let got = launch_env_decision(
            "goose",
            "s1",
            &resp,
            &BTreeMap::new(),
            required_gateway_keys("goose"),
        )
        .unwrap();
        assert_eq!(got.get("GOOSE_PROVIDER").map(String::as_str), Some("openai"));
        assert_eq!(got.get("OPENAI_HOST").map(String::as_str), Some("https://gw/gateway/openai"));
        assert_eq!(got.get("OPENAI_BASE_PATH").map(String::as_str), Some("chat/completions"));
        assert_eq!(got.get("OPENAI_API_KEY").map(String::as_str), Some("tok"));
    }

    #[test]
    fn an_unbound_acp_launch_gets_no_gateway_aliases() {
        let resp = GatewayEnvResponse { account_bound: false, ..Default::default() };
        let hint = env_of(&[("OPENAI_BASE_URL", "https://api.openai.com/v1")]);
        let got = launch_env_decision("goose", "s1", &resp, &hint, required_gateway_keys("goose"))
            .unwrap();
        assert_eq!(got, hint, "an agent on its own login keeps its own env untouched");
    }

    #[test]
    fn a_bound_acp_launch_fails_closed_on_an_empty_or_partial_env() {
        for agent in ["qwen", "goose"] {
            let empty = GatewayEnvResponse { account_bound: true, ..Default::default() };
            let err = launch_env_decision(
                agent,
                "s1",
                &empty,
                &env_of(&[("OPENAI_API_KEY", "stale")]),
                required_gateway_keys(agent),
            )
            .unwrap_err();
            assert!(err.to_string().contains("account-bound"), "{agent}: {err}");
            assert!(err.to_string().contains(agent), "{agent}: {err}");

            let partial = bound(&[("OPENAI_BASE_URL", "gw")]);
            let err = launch_env_decision(
                agent,
                "s1",
                &partial,
                &BTreeMap::new(),
                required_gateway_keys(agent),
            )
            .unwrap_err();
            assert!(err.to_string().contains("OPENAI_API_KEY"), "{agent}: {err}");
        }
        assert!(required_gateway_keys("fake-acp").is_empty());
    }

    /// An account-bound gemini launch carries the Google pair as minted: the
    /// base URL alone selects gemini-cli's `gateway` auth type, so no selector
    /// env is added, and either key missing refuses the launch.
    #[test]
    fn gemini_takes_the_google_pair_as_minted() {
        let resp = bound(&[
            ("GOOGLE_GEMINI_BASE_URL", "https://gw/gateway/google"),
            ("GEMINI_API_KEY", "tok"),
        ]);
        let hint = env_of(&[("GEMINI_API_KEY", "own-key"), ("GEMINI_MODEL", "gemini-2.5-pro")]);
        let got =
            launch_env_decision("gemini", "s1", &resp, &hint, required_gateway_keys("gemini"))
                .unwrap();
        assert_eq!(
            got.get("GOOGLE_GEMINI_BASE_URL").map(String::as_str),
            Some("https://gw/gateway/google")
        );
        assert_eq!(got.get("GEMINI_API_KEY").map(String::as_str), Some("tok"));
        assert_eq!(got.get("GEMINI_MODEL").map(String::as_str), Some("gemini-2.5-pro"));
        assert_eq!(got.len(), 3, "nothing beyond the pair and the hint: {got:?}");
        assert_eq!(required_gateway_keys("gemini"), GOOGLE_GATEWAY_KEYS);

        let partial = bound(&[("GEMINI_API_KEY", "tok")]);
        let err = launch_env_decision(
            "gemini",
            "s1",
            &partial,
            &BTreeMap::new(),
            required_gateway_keys("gemini"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("GOOGLE_GEMINI_BASE_URL"), "got: {err}");
        let unbound = GatewayEnvResponse { account_bound: false, ..Default::default() };
        assert_eq!(
            launch_env_decision("gemini", "s1", &unbound, &hint, required_gateway_keys("gemini"))
                .unwrap(),
            hint
        );
    }

    #[test]
    fn every_alias_row_requires_the_family_pair_it_copies_from() {
        for row in GATEWAY_ALIASES {
            assert!(!row.required.is_empty(), "{}", row.agent);
            for (family_key, agent_key) in row.aliases {
                assert!(row.required.contains(family_key), "{}: {family_key}", row.agent);
                assert_ne!(family_key, agent_key, "{}", row.agent);
            }
            assert_eq!(required_gateway_keys(row.agent), row.required);
        }
    }

    #[tokio::test]
    async fn resolve_env_falls_back_to_hint_without_server() {
        let hint = env_of(&[("FOO", "bar")]);
        let got = resolve_env("codex", None, None, "s1", &hint, OPENAI_GATEWAY_KEYS).await.unwrap();
        assert_eq!(got, hint);
    }

    /// The pull is the only source of a session's enabled plugins.
    #[tokio::test]
    async fn the_pull_hands_the_enabled_plugins_to_every_adapter() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let _ = sock.read(&mut [0u8; 4096]).unwrap();
            let body = serde_json::to_string(&GatewayEnvResponse {
                account_bound: true,
                env: env_of(&[("OPENAI_BASE_URL", "gw"), ("OPENAI_API_KEY", "tok")]),
                plugins: vec![cctui_proto::api::SessionPlugin {
                    id: "yubisashi".into(),
                    version: "0.5.3".into(),
                    skills_hash: "abcdef".into(),
                    files: vec!["yubisashi/SKILL.md".into()],
                    env: BTreeMap::new(),
                }],
                ..Default::default()
            })
            .unwrap();
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });

        let server = crate::client::ServerClient::new(format!("http://{addr}"));
        let launch = super::resolve_launch(
            "codex",
            Some(&server),
            Some(&"machine-key".to_owned()),
            "s1",
            &BTreeMap::new(),
            OPENAI_GATEWAY_KEYS,
        )
        .await
        .expect("a fully routed env");
        assert_eq!(launch.plugins.len(), 1);
        assert_eq!(launch.plugins[0].id, "yubisashi");
        assert_eq!(launch.env.get("OPENAI_API_KEY").map(String::as_str), Some("tok"));
    }
}
