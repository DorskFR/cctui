//! Machine-authenticated daemon transport for the automation host.
use crate::host::Host;
use axum::Json;
use axum::extract::ws::WebSocket;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use cctui_proto::adapter::{AdapterEvent, SessionMeta};
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use cctui_server_core as core;
use core::targets::daemon::conn_limit::{ConnGuard, DAEMON_CONNS};
use core::targets::daemon::decode::{MAX_TRANSFER_BYTES, STALE_TRANSFER};
use core::targets::daemon::lost::PENDING_DAEMON_LOST;
use core::targets::daemon::ownership::{SessionOwners, admit, claim_announced};
use core::targets::daemon::reconcile;
use core::targets::daemon::transport::{
    DAEMON_LIVENESS_CHECK, DAEMON_READ_TIMEOUT, Inbound, InboundReader, Leaves, announced_session,
    bearer_token, decode_inbound, leaves_of, outbound_pump, session_scope,
};
use futures_util::StreamExt;
use std::collections::HashSet;
use tokio::sync::mpsc;
use uuid::Uuid;

pub async fn auth(
    State(host): State<Host>,
    Json(req): Json<cctui_proto::api::DaemonAuthRequest>,
) -> Result<Json<cctui_proto::api::DaemonAuthResponse>, StatusCode> {
    let ctx = host.auth_config.validate(&req.machine_key).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let machine_id = ctx.machine_id.ok_or(StatusCode::FORBIDDEN)?;
    Ok(Json(cctui_proto::api::DaemonAuthResponse {
        session_token: req.machine_key,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
        machine_id,
        user_id: ctx.user_id,
        capabilities: vec![cctui_proto::capability::TURN_END.into()],
    }))
}
pub async fn ws(
    upgrade: WebSocketUpgrade,
    State(host): State<Host>,
    headers: HeaderMap,
) -> Result<axum::response::Response, StatusCode> {
    let token = bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = host.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let machine_id = ctx.machine_id.ok_or(StatusCode::FORBIDDEN)?;
    let shared = sqlx::query_scalar::<_, String>("SELECT kind FROM machines WHERE id = $1")
        .bind(machine_id)
        .fetch_optional(&host.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some_and(|kind| kind == "dispatch");
    let cap =
        std::env::var("CCTUI_MAX_DAEMON_WS_CONNS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    let guard = if shared {
        Some(ConnGuard::exempt())
    } else {
        DAEMON_CONNS.acquire(ctx.key_id, cap, std::time::Instant::now())
    }
    .ok_or(StatusCode::TOO_MANY_REQUESTS)?;
    Ok(upgrade
        .on_upgrade(move |socket| connection(socket, host, machine_id, ctx.user_id, shared, guard))
        .into_response())
}
struct Connection {
    host: Host,
    machine: Uuid,
    user: Uuid,
    id: Uuid,
    owners: SessionOwners,
    announced: HashSet<String>,
}
async fn connection(
    socket: WebSocket,
    host: Host,
    machine: Uuid,
    user: Uuid,
    shared: bool,
    guard: ConnGuard,
) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = mpsc::channel(64);
    let (priority, priority_rx) = mpsc::channel(16);
    let mut conn = Connection {
        host,
        machine,
        user,
        id: Uuid::new_v4(),
        owners: SessionOwners::new(machine, user),
        announced: HashSet::new(),
    };
    conn.host.bus.register_daemon(machine, conn.id, tx.clone());
    conn.host.bus.register_daemon_priority(conn.id, priority);
    if !shared {
        PENDING_DAEMON_LOST.cancel_machine(machine);
    }
    conn.host.auth_recovery_daemons.remove(&machine);
    let outgoing = tokio::spawn(outbound_pump(sink, rx, priority_rx));
    if let Ok(adapters) = reconcile::load_reconcile(&conn.host, machine).await {
        let secret_scrub = reconcile::load_scrub_config(&conn.host, machine).await;
        let _ = tx.send(DaemonFrameDown::Reconcile { adapters, secret_scrub }).await;
    }
    if let Ok(session_marks) = reconcile::load_resume_marks(&conn.host, machine).await
        && !session_marks.is_empty()
    {
        let _ = tx.send(DaemonFrameDown::ResumeMarks { session_marks, archived: Vec::new() }).await;
    }
    let mut reader =
        InboundReader::new(&mut stream, DAEMON_READ_TIMEOUT, DAEMON_LIVENESS_CHECK).await;
    let mut reassembler = cctui_proto::chunk::Reassembler::new(MAX_TRANSFER_BYTES);
    loop {
        reassembler.evict_older_than(STALE_TRANSFER);
        let inbound = tokio::select! { inbound=reader.next()=>inbound, ()=guard.evicted()=>break };
        if matches!(inbound, Inbound::Done | Inbound::Idle) {
            break;
        }
        let Some(frame) = decode_inbound(inbound) else { continue };
        match leaves_of(frame, &mut reassembler, &tx).await {
            Leaves::Frames(frames) => {
                for frame in frames {
                    if let Some(session) = session_scope(&frame)
                        && !admit(&mut conn.owners, &conn.host.pool, session).await
                    {
                        continue;
                    }
                    let announced = announced_session(&frame).map(str::to_owned);
                    if let Some(session) = &announced {
                        conn.host.bus.bind_session_conn(session, conn.id);
                    }
                    if let Err(error) = conn.process(frame).await {
                        tracing::warn!(%error,%machine,"daemon event rejected");
                    }
                    if let Some(session) = announced
                        && claim_announced(
                            &mut conn.owners,
                            &conn.host.pool,
                            &conn.host.bus,
                            conn.id,
                            &session,
                        )
                        .await
                    {
                        PENDING_DAEMON_LOST.announced(&session);
                        conn.announced.insert(session);
                    }
                }
            }
            Leaves::Dropped => {}
            Leaves::Closed => break,
        }
    }
    let newest = conn.host.bus.unregister_daemon(machine, conn.id, &tx);
    outgoing.abort();
    if newest || shared {
        conn.schedule_lost(shared);
    }
}
impl Connection {
    async fn register(
        &self,
        adapter: &str,
        session: &str,
        meta: SessionMeta,
    ) -> anyhow::Result<()> {
        let guard = self.host.lifecycle.lock().await;
        let spawn_key =
            meta.extra.get("spawn_key").and_then(serde_json::Value::as_str).map(str::to_owned);
        for identity in [Some(session), spawn_key.as_deref()].into_iter().flatten() {
            if let Some(stop) = crate::termination::find(&self.host, identity).await?
                && stop.user_id == self.user
                && stop.machine_id.is_none_or(|id| id == self.machine)
            {
                crate::termination::record(
                    &self.host,
                    &[session.to_owned()],
                    self.user,
                    Some(self.machine),
                    adapter,
                    &stop.end_reason,
                    stop.detail.as_deref().unwrap_or("late registration of stopped work"),
                )
                .await?;
                drop(guard);
                crate::termination::stop_worker(&self.host, self.machine, session, adapter).await;
                return Ok(());
            }
        }
        let trusted_spawn_key = if let Some(spawn) = spawn_key.as_deref()
            && core::store::tokens::rebind_allowed(
                &self.host.pool,
                spawn,
                session,
                self.machine,
                self.user,
            )
            .await?
        {
            core::session_credentials::rebind_spawn_key(&self.host, spawn.into(), session.into())
                .await;
            Some(spawn)
        } else {
            None
        };
        let registered = core::lifecycle::upsert_session(
            &self.host.pool,
            &self.host.bus,
            self.machine,
            self.user,
            adapter,
            session,
            meta.working_dir,
            meta.parent_local_id,
            meta.extra.get("observed_at").and_then(serde_json::Value::as_i64),
            (!meta.extra.is_null()).then_some(meta.extra),
        )
        .await?;
        if registered.is_some() {
            core::machine_launches::registered(
                &self.host.pool,
                trusted_spawn_key.unwrap_or(session),
                session,
                self.machine,
                self.user,
            )
            .await?;
            core::machine_policy::claim_spawner(&self.host.pool, session, trusted_spawn_key).await;
        }
        Ok(())
    }
    async fn process(&self, frame: DaemonFrameUp) -> anyhow::Result<()> {
        match frame {
            DaemonFrameUp::SessionRegistered { adapter_id, local_id } => {
                self.register(&adapter_id, &local_id, SessionMeta::default()).await?;
            }
            DaemonFrameUp::Event { adapter_id, event } => self.event(&adapter_id, event).await?,
            DaemonFrameUp::Heartbeat { mark_acks, auth_recovery, .. } => {
                note_support(&self.host.mark_ack_daemons, self.machine, mark_acks);
                note_support(&self.host.auth_recovery_daemons, self.machine, auth_recovery);
                sqlx::query("UPDATE machines SET last_seen_at=now() WHERE id=$1")
                    .bind(self.machine)
                    .execute(&self.host.pool)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }
    async fn event(&self, adapter: &str, event: AdapterEvent) -> anyhow::Result<()> {
        match event {
            AdapterEvent::SessionStarted { local_id, meta } => {
                self.register(adapter, &local_id, meta).await?;
            }
            AdapterEvent::Message { local_id, payload, .. } => {
                core::results::capture_message(
                    &self.host.pool,
                    self.machine,
                    self.user,
                    &local_id,
                    adapter,
                    payload,
                )
                .await?;
            }
            AdapterEvent::SessionEnded { local_id, reason } => {
                let _guard = self.host.lifecycle.lock().await;
                if crate::termination::stopped(&self.host, &local_id, self.user, self.machine)
                    .await?
                {
                    return Ok(());
                }
                core::lifecycle::persist_session_end(
                    &self.host.pool,
                    &core::lifecycle::NoopSessionEndJournal,
                    self.machine,
                    self.user,
                    &local_id,
                    &reason,
                )
                .await?;
                core::session_credentials::revoke_session_tokens(&self.host, &local_id).await;
            }
            AdapterEvent::Status { local_id, tempo, state, activity, .. } => {
                sqlx::query("UPDATE sessions SET last_heartbeat=now(), tempo=COALESCE($2,tempo),agent_state=COALESCE($3,agent_state),activity=COALESCE($4,activity) WHERE id=$1 AND machine_uuid=$5 AND user_id=$6").bind(local_id).bind(tempo).bind(state).bind(activity).bind(self.machine).bind(self.user).execute(&self.host.pool).await?;
            }
            AdapterEvent::TurnEnd { local_id, .. } => {
                sqlx::query("UPDATE sessions SET agent_state='idle',tempo='idle',activity=NULL WHERE id=$1 AND machine_uuid=$2 AND user_id=$3 AND status='active'").bind(local_id).bind(self.machine).bind(self.user).execute(&self.host.pool).await?;
            }
            event @ AdapterEvent::TokenUsage { .. } => {
                core::daemon_usage::on_token_usage(&self.host.pool, event).await?;
            }
            AdapterEvent::TranscriptMark { local_id, offset } => {
                core::daemon_checkpoints::update_transcript_mark(&self.host, &local_id, offset)
                    .await?;
                core::daemon_checkpoints::ack_transcript_mark(
                    &self.host,
                    self.machine,
                    adapter,
                    &local_id,
                    offset,
                )
                .await;
            }
            AdapterEvent::SessionModel { local_id, model } => {
                if let Some(model) = cctui_proto::adapter::observed_model_id(&model) {
                    sqlx::query("UPDATE sessions SET model=$2 WHERE id=$1 AND machine_uuid=$3 AND user_id=$4").bind(local_id).bind(model).bind(self.machine).bind(self.user).execute(&self.host.pool).await?;
                }
            }
            AdapterEvent::PermissionRequest { local_id, .. }
            | AdapterEvent::AskQuestion { local_id, .. }
            | AdapterEvent::PlanRequest { local_id, .. } => {
                self.stop_for_human_input(adapter, &local_id).await?;
            }
            AdapterEvent::CommandResult { command_id, ok, error } => {
                self.command_result(command_id, ok, error).await?;
            }
            _ => {}
        }
        Ok(())
    }
    async fn command_result(
        &self,
        command_id: uuid::Uuid,
        ok: bool,
        error: Option<String>,
    ) -> anyhow::Result<()> {
        let _guard = self.host.lifecycle.lock().await;
        let known =
            core::machine_launches::find(&self.host.pool, &command_id.to_string(), None).await?;
        if known
            .as_ref()
            .is_some_and(|launch| launch.machine_id != self.machine || launch.user_id != self.user)
        {
            tracing::warn!(%command_id,"refusing foreign machine launch verdict");
            return Ok(());
        }
        let durable = core::machine_launches::verdict(
            &self.host.pool,
            command_id,
            self.machine,
            self.user,
            ok,
            error.as_deref(),
        )
        .await?;
        let pending = self.host.pending_commands.remove(&command_id).map(|(_, c)| c);
        let failed = durable
            .map(|launch| launch.failed_row())
            .or_else(|| if known.is_none() { pending.and_then(|c| c.spawn) } else { None });
        if !ok && let Some(row) = failed {
            let reason = cctui_proto::adapter::EndReason::SpawnFailed {
                detail: error.clone().unwrap_or_else(|| "spawn failed".into()),
            };
            core::lifecycle::persist_failed_spawn(
                &self.host.pool,
                &core::lifecycle::NoopSessionEndJournal,
                &row,
                &reason,
            )
            .await?;
            core::child_policy::release_child(&self.host.pool, &row.session_id).await;
            self.host.spawn_capabilities.remove(&row.session_id);
            self.host.session_usd_budgets.remove(&row.session_id);
            core::session_credentials::revoke_session_tokens(&self.host, &row.session_id).await;
        }
        core::commands::resolve_verdict(
            &self.host.command_waiters,
            command_id,
            core::commands::CommandVerdict { ok, error },
        );
        Ok(())
    }
    async fn stop_for_human_input(&self, adapter: &str, local_id: &str) -> anyhow::Result<()> {
        let guard = self.host.lifecycle.lock().await;
        crate::termination::record(
            &self.host,
            &[local_id.to_owned()],
            self.user,
            Some(self.machine),
            adapter,
            "other",
            "human input required; unattended automation stopped the worker",
        )
        .await?;
        drop(guard);
        crate::termination::stop_worker(&self.host, self.machine, local_id, adapter).await;
        Ok(())
    }
    fn schedule_lost(self, shared: bool) {
        let host = self.host;
        let machine = self.machine;
        let user = self.user;
        PENDING_DAEMON_LOST.schedule(self.id,machine,self.announced.into_iter().collect(),std::time::Duration::from_secs(45),move |sessions|async move {
            if !shared && host.bus.daemon_connected(machine) {return;}
            if let Err(error)=sqlx::query("UPDATE sessions SET status='ended',ended_at=now(),end_reason='daemon_lost' WHERE id=ANY($1) AND machine_uuid=$2 AND user_id=$3 AND status IN ('new','active','inactive') AND end_reason IS NULL").bind(sessions).bind(machine).bind(user).execute(&host.pool).await {
                tracing::error!(%error,%machine,"daemon loss persistence failed");
            }
        });
    }
}
fn note_support(map: &dashmap::DashMap<Uuid, ()>, machine: Uuid, supported: Option<bool>) {
    if supported == Some(true) {
        map.insert(machine, ());
    } else {
        map.remove(&machine);
    }
}

/// Same version contract used by a worker's post-update health probe.
pub async fn version(
    State(host): State<Host>,
    headers: HeaderMap,
) -> Result<Json<cctui_proto::api::update_hook::DaemonVersion>, StatusCode> {
    let token = bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = host.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    ctx.machine_id.ok_or(StatusCode::FORBIDDEN)?;
    Ok(Json(cctui_proto::api::update_hook::DaemonVersion {
        version: env!("CARGO_PKG_VERSION"),
        git_hash: env!("CCTUI_GIT_HASH"),
    }))
}

pub async fn machine_status(
    State(host): State<Host>,
    axum::Extension(ctx): axum::Extension<core::auth::AuthContext>,
    axum::extract::Path(machine): axum::extract::Path<Uuid>,
) -> Result<Json<core::targets::machine_status::MachineStatusResponse>, core::error::AppError> {
    core::machine_policy::require_machine_read(&host.pool, &ctx, machine).await?;
    core::targets::machine_status::machine_status(State(host.core), axum::extract::Path(machine))
        .await
}
