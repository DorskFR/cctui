//! `OpenCode` adapter: drives `opencode serve` over its HTTP API + SSE bus.

pub mod client;
pub mod config;
pub mod events;
pub mod normalize;
pub mod session;

use cctui_proto::adapter::{AdapterCommand, AdapterEvent};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::adapter_runtime::{Adapter, AdapterCtx, AdapterFactory};
use crate::client::ServerClient;
use session::{LiveRegistry, OpenCodeConfig, OpenCodeSession, SessionCommand, SpawnParams};

pub const ADAPTER_ID: &str = "opencode";

/// Dispatch-payload env key naming the opencode agent profile to run under.
pub const AGENT_ENV: &str = "CCTUI_OPENCODE_AGENT";

/// FAIL-CLOSED: an account-bound session whose gateway env came back empty must
/// not launch — opencode would fall back to whatever `FIREWORKS_API_KEY` the pod
/// happens to carry (or none) instead of the account's minted token.
fn launch_env_decision(
    local_id: &str,
    resp: &cctui_proto::api::GatewayEnvResponse,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    match resp {
        r if r.account_bound && r.env.is_empty() => anyhow::bail!(
            "refusing to launch opencode {local_id}: session is account-bound but the server \
             returned no gateway env (account missing/unmintable)"
        ),
        r if r.account_bound => {
            let mut merged = hint.clone();
            merged.extend(r.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            Ok(merged)
        }
        _ => Ok(hint.clone()),
    }
}

async fn resolve_launch_env(
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let (Some(server), Some(mk)) = (server, machine_key) else {
        return Ok(hint.clone());
    };
    match server.gateway_env(mk, local_id).await {
        Ok(resp) => launch_env_decision(local_id, &resp, hint),
        Err(err) => {
            tracing::warn!(%local_id, "opencode gateway-env pull failed; using pushed env: {err}");
            Ok(hint.clone())
        }
    }
}

pub struct OpenCodeAdapter;

#[async_trait::async_trait]
impl Adapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()> {
        let cfg = OpenCodeConfig::from_value(&ctx.config);
        match session::probe_version(&cfg.bin).await {
            Ok(version) => {
                tracing::info!(%version, pinned = client::OPENCODE_PINNED_VERSION, "opencode adapter ready");
            }
            Err(err) => tracing::error!(
                %err,
                bin = %cfg.bin,
                "opencode binary unavailable — spawns will fail until it is installed"
            ),
        }
        command_pump(cfg, ctx).await;
        Ok(())
    }
}

#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
async fn command_pump(cfg: OpenCodeConfig, ctx: AdapterCtx) {
    let AdapterCtx { events, mut commands, shutdown, server, machine_key, .. } = ctx;
    let live: LiveRegistry = LiveRegistry::default();

    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    AdapterCommand::Spawn { spec, command_id, session_id } => {
                        let Some(working_dir) = spec.working_dir.clone() else {
                            fail(&events, command_id, "working_dir required").await;
                            continue;
                        };
                        let key = session_id
                            .or(command_id)
                            .map_or_else(String::new, |id| id.to_string());
                        let env = match resolve_launch_env(
                            server.as_ref(),
                            machine_key.as_ref(),
                            &key,
                            &spec.env,
                        )
                        .await
                        {
                            Ok(env) => env,
                            Err(err) => {
                                fail(&events, command_id, &err.to_string()).await;
                                continue;
                            }
                        };
                        let attachments = match crate::adapters::uploads::stage_bootstrap(
                            &key,
                            &spec.bootstrap,
                        ) {
                            Ok(paths) => paths,
                            Err(err) => {
                                fail(
                                    &events,
                                    command_id,
                                    &format!("attachment staging failed: {err}"),
                                )
                                .await;
                                continue;
                            }
                        };
                        let params = SpawnParams {
                            cfg: cfg.clone(),
                            key,
                            cwd: working_dir,
                            env,
                            prompt: spec.prompt.clone(),
                            name: spec.name.clone(),
                            model: spec.model.clone(),
                            agent: agent_of(&spec, &cfg),
                            attachments,
                            command_id,
                            parent_local_id: spec.parent_local_id.clone(),
                        };
                        let session = OpenCodeSession::new(
                            params,
                            events.clone(),
                            live.clone(),
                            shutdown.clone(),
                        );
                        tokio::spawn(session.run());
                    }
                    AdapterCommand::Fork { parent_local_id, spec, command_id, .. } => {
                        let delivered = route(
                            &live,
                            &parent_local_id,
                            SessionCommand::Fork {
                                parent: parent_local_id.clone(),
                                prompt: spec.prompt.clone(),
                                name: spec.name.clone(),
                                command_id,
                            },
                        )
                        .await;
                        if !delivered {
                            fail(
                                &events,
                                command_id,
                                "opencode fork requires the parent session to be live on this \
                                 daemon",
                            )
                            .await;
                        }
                    }
                    AdapterCommand::SendMessage { local_id, text }
                    | AdapterCommand::Reply { local_id, text, .. } => {
                        route(
                            &live,
                            &local_id,
                            SessionCommand::Prompt { session_id: local_id.clone(), text },
                        )
                        .await;
                    }
                    AdapterCommand::Kill { local_id, .. } | AdapterCommand::Remove { local_id } => {
                        if !route(
                            &live,
                            &local_id,
                            SessionCommand::Kill { session_id: local_id.clone() },
                        )
                        .await
                        {
                            let _ = events
                                .send(AdapterEvent::SessionEnded {
                                    local_id,
                                    reason: cctui_proto::adapter::EndReason::Killed,
                                })
                                .await;
                        }
                    }
                    AdapterCommand::Interrupt { local_id, command_id } => {
                        let delivered = route(
                            &live,
                            &local_id,
                            SessionCommand::Kill { session_id: local_id.clone() },
                        )
                        .await;
                        if let Some(command_id) = command_id {
                            let _ = events
                                .send(AdapterEvent::CommandResult {
                                    command_id,
                                    ok: delivered,
                                    error: (!delivered)
                                        .then(|| "no live opencode session".to_owned()),
                                })
                                .await;
                        }
                    }
                    AdapterCommand::PermissionResponse { local_id, request_id, allow } => {
                        route(
                            &live,
                            &local_id,
                            SessionCommand::Permission {
                                session_id: local_id.clone(),
                                request_id,
                                allow,
                            },
                        )
                        .await;
                    }
                    AdapterCommand::Diagnose { local_id, request_id } => {
                        let report = diagnose(&live, &local_id, server.as_ref(), machine_key.as_ref())
                            .await;
                        let _ = events
                            .send(AdapterEvent::Diagnose {
                                local_id,
                                request_id,
                                report: Box::new(report),
                            })
                            .await;
                    }
                    AdapterCommand::ResumeMarks { .. } | AdapterCommand::Resume { .. } => {}
                    _ => tracing::warn!("opencode: unhandled AdapterCommand variant"),
                }
            }
        }
    }
}

/// Which opencode agent profile the spawn runs under: named by the dispatch
/// payload (`CCTUI_OPENCODE_AGENT`), else the adapter default. Both resolve
/// against the agent definitions the daemon writes into the session config, so
/// `cctui-reviewer` is selectable without anything being baked into the image.
fn agent_of(spec: &cctui_proto::adapter::SessionSpec, cfg: &OpenCodeConfig) -> Option<String> {
    spec.env
        .get(AGENT_ENV)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| cfg.default_agent.clone())
}

async fn route(live: &LiveRegistry, local_id: &str, cmd: SessionCommand) -> bool {
    let Some(tx) = live.lock().await.get(local_id).cloned() else {
        tracing::warn!(%local_id, "opencode: no live session for command");
        return false;
    };
    tx.send(cmd).await.is_ok()
}

async fn fail(events: &mpsc::Sender<AdapterEvent>, command_id: Option<Uuid>, error: &str) {
    tracing::error!(%error, "opencode command failed");
    if let Some(command_id) = command_id {
        let _ = events
            .send(AdapterEvent::CommandResult {
                command_id,
                ok: false,
                error: Some(error.to_owned()),
            })
            .await;
    }
}

async fn diagnose(
    live: &LiveRegistry,
    local_id: &str,
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
) -> cctui_proto::diagnose::SessionDiagnose {
    use cctui_proto::diagnose::{DiagnoseFact, EffectiveState, GatewayStatus, SessionDiagnose};

    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let live_present = live.lock().await.contains_key(local_id);
    let verdict = if live_present { "live" } else { "unknown session" };

    SessionDiagnose {
        local_id: local_id.to_owned(),
        short: None,
        generated_at_ms: now_ms,
        adapter: ADAPTER_ID.to_owned(),
        effective_state: DiagnoseFact::fresh(
            EffectiveState {
                verdict: verdict.to_owned(),
                tempo: None,
                state: Some(verdict.to_owned()),
                detail: None,
                activity: None,
            },
            "opencode-adapter",
            now_ms,
        ),
        last_hook_event: na(),
        attach: na(),
        pty_output: na(),
        claude_socket: na(),
        transcript: na(),
        prompts: na(),
        permission_mode: na(),
        dispatch: na(),
        gateway: DiagnoseFact::fresh(
            GatewayStatus { server_configured: server.is_some() && machine_key.is_some() },
            "daemon-config",
            now_ms,
        ),
        codex: None,
    }
}

fn na<T>() -> cctui_proto::diagnose::DiagnoseFact<T> {
    cctui_proto::diagnose::DiagnoseFact::missing(ADAPTER_ID, "claude-only fact")
}

pub struct OpenCodeFactory;

impl AdapterFactory for OpenCodeFactory {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }
    fn build(&self, _config: serde_json::Value) -> Box<dyn Adapter> {
        Box::new(OpenCodeAdapter)
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::GatewayEnvResponse;

    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn gateway_env_wins_over_the_pushed_hint() {
        let hint = env_of(&[("KEEP", "1"), ("FIREWORKS_BASE_URL", "old")]);
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: env_of(&[("FIREWORKS_BASE_URL", "gw"), ("FIREWORKS_API_KEY", "tok")]),
            ..Default::default()
        };
        let got = launch_env_decision("ses_1", &resp, &hint).unwrap();
        assert_eq!(got.get("KEEP").map(String::as_str), Some("1"));
        assert_eq!(got.get("FIREWORKS_BASE_URL").map(String::as_str), Some("gw"));
        assert_eq!(got.get("FIREWORKS_API_KEY").map(String::as_str), Some("tok"));
    }

    #[test]
    fn account_bound_without_env_fails_closed() {
        let resp = GatewayEnvResponse { account_bound: true, ..Default::default() };
        let err = launch_env_decision("ses_1", &resp, &env_of(&[("HINT", "1")])).unwrap_err();
        assert!(err.to_string().contains("account-bound"));
    }

    #[test]
    fn unbound_sessions_keep_the_hint() {
        let resp = GatewayEnvResponse { account_bound: false, ..Default::default() };
        let hint = env_of(&[("HINT", "1")]);
        assert_eq!(launch_env_decision("ses_1", &resp, &hint).unwrap(), hint);
    }

    fn spec_with_env(env: &[(&str, &str)]) -> cctui_proto::adapter::SessionSpec {
        cctui_proto::adapter::SessionSpec {
            adapter_id: ADAPTER_ID.into(),
            working_dir: Some("/repo".to_owned()),
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            env: env_of(env),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        }
    }

    #[test]
    fn dispatch_payload_selects_the_agent_profile() {
        let cfg = OpenCodeConfig::default();
        assert_eq!(
            agent_of(&spec_with_env(&[(AGENT_ENV, config::REVIEWER_AGENT)]), &cfg).as_deref(),
            Some(config::REVIEWER_AGENT)
        );
        assert!(agent_of(&spec_with_env(&[]), &cfg).is_none());
    }

    #[test]
    fn adapter_default_agent_applies_when_the_payload_is_silent() {
        let cfg = OpenCodeConfig {
            default_agent: Some(config::REVIEWER_AGENT.to_owned()),
            ..OpenCodeConfig::default()
        };
        assert_eq!(agent_of(&spec_with_env(&[]), &cfg).as_deref(), Some(config::REVIEWER_AGENT));
        assert_eq!(
            agent_of(&spec_with_env(&[(AGENT_ENV, "build")]), &cfg).as_deref(),
            Some("build")
        );
    }
}
