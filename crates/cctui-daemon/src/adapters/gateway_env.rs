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
    let (Some(server), Some(mk)) = (server, machine_key) else {
        return Ok(hint.clone());
    };
    match server.gateway_env(mk, local_id).await {
        Ok(resp) => launch_env_decision(adapter, local_id, &resp, hint, required_keys),
        Err(e) => {
            tracing::warn!(
                %local_id,
                adapter,
                "gateway-env pull failed; falling back to pushed env: {e}"
            );
            Ok(hint.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cctui_proto::api::GatewayEnvResponse;

    use super::{
        CLAUDE_GATEWAY_KEYS, FIREWORKS_GATEWAY_KEYS, OPENAI_GATEWAY_KEYS, launch_env_decision,
        resolve_env,
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

    #[tokio::test]
    async fn resolve_env_falls_back_to_hint_without_server() {
        let hint = env_of(&[("FOO", "bar")]);
        let got = resolve_env("codex", None, None, "s1", &hint, OPENAI_GATEWAY_KEYS).await.unwrap();
        assert_eq!(got, hint);
    }
}
