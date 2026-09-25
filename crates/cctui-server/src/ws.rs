use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::Arc;

use cctui_proto::ws::{AgentEvent, ServerEvent, TuiCommand};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc};

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;

/// Authorize a WS command against a session for the connected principal,
/// mirroring the HTTP ownership gate (`spawn.rs`/`admin.rs`): admins always
/// pass; everyone else must own the session (resolved via
/// `machine_uuid -> machines.user_id`). A session whose owner can't be resolved
/// is denied for non-admins rather than leaked. Returns `true` when permitted.
async fn ws_owns_session(state: &AppState, ctx: &AuthContext, session_id: &str) -> bool {
    if ctx.is_admin() {
        return true;
    }
    // Reuse the exact same ownership query as the HTTP `Resource(Session)` guard
    // (`machine_uuid -> machines.user_id`) so the two transports never drift.
    // A DB error or unknown session resolves to "not owned".
    let owner = crate::authz::session_owner(session_id, &state.pool).await.unwrap_or_else(|e| {
        tracing::error!(%session_id, "db error (ws session authz): {e}");
        None
    });
    owner == Some(ctx.user_id)
}

// --- TUI WebSocket ---

/// A JSON-encoded [`ServerEvent`] queued on one socket's outbound channel.
type Frame = Arc<str>;
type FrameTx = mpsc::Sender<Frame>;

/// Encode and queue a socket-local event. `false` once the socket is gone.
async fn send_event(tx: &FrameTx, event: &ServerEvent) -> bool {
    match serde_json::to_string(event) {
        Ok(json) => tx.send(json.into()).await.is_ok(),
        Err(err) => {
            tracing::warn!(%err, "failed to serialize ServerEvent");
            true
        }
    }
}

pub async fn tui_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    // CSWSH defense-in-depth: reject a cross-origin browser upgrade before auth.
    if !origin_permitted(&state.config, &headers) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Browser WS upgrades are same-origin GETs that carry the `HttpOnly` auth
    // cookie automatically, so the token stays out of the query string (where it
    // would leak into access logs). `bearer_or_cookie` also accepts an
    // `Authorization` header for non-browser clients.
    let token = crate::auth::bearer_or_cookie(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let auth_ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;

    // The TUI/webui socket is for human identities (a user or admin token), not
    // a machine key. Gate on the `read` scope and the absence of a machine id.
    if auth_ctx.machine_id.is_some() || !auth_ctx.has(Scope::Read) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(ws.on_upgrade(move |socket| handle_tui_ws(socket, state, auth_ctx)))
}

/// A present `Origin` must be in the allowlist; an absent one is a non-browser
/// client (TUI/daemon) and is allowed. An unparseable `Origin` is rejected.
fn origin_permitted(config: &crate::config::Config, headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::ORIGIN)
        .is_none_or(|value| value.to_str().is_ok_and(|o| config.origin_allowed(o)))
}

/// Mirrors the daemon socket's cadence (`routes/daemon.rs`) and stays well
/// under the gateway's idle timeout.
const TUI_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(20);

/// Besides forwarding events, the outbound pump sends a periodic `Ping`: a
/// browser that slept leaves a half-open socket that never errors on read, so
/// only a write failure retires this task and releases its relays. The paired
/// [`ServerEvent::Heartbeat`] carries the same tick to the browser, which cannot
/// observe a `Ping`; it is written per-socket here rather than broadcast, so it
/// reaches every client whether or not any daemon is online.
fn spawn_send_task(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Frame>,
) {
    tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(TUI_KEEPALIVE);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;
        loop {
            tokio::select! {
                frame = rx.recv() => {
                    let Some(frame) = frame else { break };
                    if sink.send(Message::Text((&*frame).into())).await.is_err() {
                        break;
                    }
                }
                _ = keepalive.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                    let beat = serde_json::to_string(&ServerEvent::Heartbeat {})
                        .expect("Heartbeat serializes");
                    if sink.send(Message::Text(beat.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn spawn_relay_task(
    mut receiver: broadcast::Receiver<AgentEvent>,
    session_id: String,
    event_tx: FrameTx,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let event = match receiver.recv().await {
                Ok(data) => ServerEvent::Stream { session_id: session_id.clone(), data },
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(session_id = %session_id, skipped = n, "TUI receiver lagged");
                    ServerEvent::Resync { session_id: Some(session_id.clone()) }
                }
            };
            if !send_event(&event_tx, &event).await {
                break;
            }
        }
    })
}

/// Dispatch a client-typed reply to the session's daemon and, when the client
/// opted in with a `client_msg_id`, ack the outcome back to this socket so the
/// UI can show a precise delivery state (sending → delivered / failed) instead
/// of optimistically assuming a sent frame was delivered.
async fn handle_message(
    state: &AppState,
    event_tx: &FrameTx,
    session_id: String,
    content: String,
    client_msg_id: Option<String>,
    ask_picks: Option<Vec<Vec<usize>>>,
    turn_id: Option<uuid::Uuid>,
) {
    // NoDaemon / NoAdapter are expected for sessions whose daemon is momentarily
    // offline — that is exactly the case the ack lets the client recover from.
    // Carry re-minted gateway env on the reply so a reply-driven cold-resume of
    // a hibernated worker revives it with a fresh valid token rather than empty
    // env. Ignored when the worker is already alive.
    let env = crate::routes::gateway::resume_env_for_session(state, &session_id).await;
    // A successful dispatch only means the frame was queued toward a daemon; the
    // adapter's `CommandResult` under this id is the delivery proof.
    let command_id = uuid::Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.clone()),
        None,
    );
    let dispatch = crate::bus::dispatch(
        state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: session_id.clone(),
            text: content,
            ask_picks,
            env,
            command_id: Some(command_id),
            turn_id,
        },
    )
    .await;
    let err_reason = dispatch.as_ref().err().map(|err| {
        use crate::bus::BusError;
        match err {
            BusError::NoDaemon(_) | BusError::NoAdapter | BusError::NotFound => {
                tracing::debug!(%session_id, ?err, "daemon dispatch skipped");
            }
            _ => tracing::warn!(%session_id, %err, "daemon dispatch failed"),
        }
        err.to_string()
    });
    if let Some(client_msg_id) = client_msg_id {
        let ack = ServerEvent::MessageAck {
            session_id,
            client_msg_id,
            ok: err_reason.is_none(),
            error: err_reason,
            command_id: Some(command_id),
        };
        send_event(event_tx, &ack).await;
    }
}

async fn handle_subscribe(
    session_id: String,
    state: &AppState,
    event_tx: &FrameTx,
    sub_handles: &mut std::collections::HashMap<String, tokio::task::JoinHandle<()>>,
) {
    let receiver = state.bus.subscribe_session(&session_id);

    // Re-surface any prompt the session is blocked on: the client resubscribes
    // on every focus change and dedups the replay by request_id / overwrite.
    replay_pending(&state.permission_store, &session_id, event_tx).await;

    if let Some(receiver) = receiver {
        let handle = spawn_relay_task(receiver, session_id.clone(), event_tx.clone());
        // Abort any prior relay task for this session on this socket before
        // replacing it. The client re-subscribes on every tab focus/visibility
        // change; without this, each resubscribe leaked an extra
        // relay task that re-delivered every event, duplicating chat messages.
        if let Some(old) = sub_handles.insert(session_id, handle) {
            old.abort();
        }
    } else {
        // Historical/terminated sessions won't be in the registry — this is expected
        tracing::debug!(session_id = %session_id, "tui_ws: session not in registry (historical)");
    }
}

/// Copies the session's open prompts out of the store, then sends them with
/// the guard released: a full per-socket channel must not hold the store.
async fn replay_pending(
    store: &crate::routes::permissions::SharedPermissionStore,
    session_id: &str,
    event_tx: &FrameTx,
) {
    let prompts = {
        let store = store.read().await;
        let mut prompts: Vec<ServerEvent> = store
            .list_pending()
            .into_iter()
            .filter(|p| p.session_id == session_id)
            .map(|p| ServerEvent::PermissionRequest {
                session_id: p.session_id,
                request_id: p.request_id,
                tool_name: p.tool_name,
                description: p.description,
                input_preview: p.input_preview,
            })
            .collect();
        if let Some(ask) = store.pending_ask(session_id) {
            prompts.push(ServerEvent::AskQuestion {
                session_id: ask.session_id,
                question: ask.question,
                questions: ask.questions,
                preamble: ask.preamble,
            });
        }
        if let Some(plan) = store.pending_plan(session_id) {
            prompts.push(ServerEvent::PlanRequest {
                session_id: plan.session_id,
                plan: plan.plan,
                preamble: plan.preamble,
            });
        }
        prompts
    };
    for event in &prompts {
        if !send_event(event_tx, event).await {
            return;
        }
    }
}

#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn run_tui_socket(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    state: AppState,
    ctx: AuthContext,
    event_tx: FrameTx,
) {
    // Relay tasks keyed by session id, so a resubscribe replaces (not stacks)
    // the per-session relay and an unsubscribe can tear it down.
    let mut sub_handles: std::collections::HashMap<String, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();
    // Sessions whose live terminal THIS socket is watching. Tracked
    // per-socket so a disconnect decrements the shared watcher refcount and the
    // daemon stops streaming to a browser that vanished without unwatching.
    let mut pty_watches: std::collections::HashSet<String> = std::collections::HashSet::new();

    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        let cmd: TuiCommand = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(%err, "failed to parse TuiCommand");
                continue;
            }
        };

        match cmd {
            TuiCommand::Subscribe { session_id } => {
                // Only stream a session this principal owns (admin bypasses).
                // Pending-ask/permission replay lives inside handle_subscribe,
                // so gating here keeps the whole replay path owner-scoped too.
                if !ws_owns_session(&state, &ctx, &session_id).await {
                    tracing::debug!(session_id = %session_id, user_id = %ctx.user_id, "tui_ws: subscribe denied (not owner)");
                    continue;
                }
                handle_subscribe(session_id, &state, &event_tx, &mut sub_handles).await;
            }
            TuiCommand::Unsubscribe { session_id } => {
                // Tear down this session's relay task so it stops delivering
                // events to this socket.
                if let Some(handle) = sub_handles.remove(&session_id) {
                    handle.abort();
                }
            }
            TuiCommand::WatchTerminal { session_id, watch } => {
                if !ws_owns_session(&state, &ctx, &session_id).await {
                    tracing::debug!(session_id = %session_id, user_id = %ctx.user_id, "tui_ws: watch-terminal denied (not owner)");
                    continue;
                }
                handle_watch_terminal(&state, &session_id, watch, &mut pty_watches).await;
            }
            TuiCommand::Message { session_id, content, client_msg_id, ask_picks, turn_id } => {
                if !ws_owns_session(&state, &ctx, &session_id).await {
                    tracing::debug!(session_id = %session_id, user_id = %ctx.user_id, "tui_ws: message denied (not owner)");
                    // Ack the failure when the client opted in, so it doesn't
                    // hang waiting on a delivery state for a denied send.
                    if let Some(client_msg_id) = client_msg_id {
                        let ack = ServerEvent::MessageAck {
                            session_id,
                            client_msg_id,
                            ok: false,
                            error: Some("forbidden".into()),
                            command_id: None,
                        };
                        send_event(&event_tx, &ack).await;
                    }
                    continue;
                }
                handle_message(
                    &state,
                    &event_tx,
                    session_id,
                    content,
                    client_msg_id,
                    ask_picks,
                    turn_id,
                )
                .await;
            }
            TuiCommand::PermissionResponse { session_id, request_id, behavior } => {
                if !ws_owns_session(&state, &ctx, &session_id).await {
                    tracing::debug!(session_id = %session_id, user_id = %ctx.user_id, "tui_ws: permission-response denied (not owner)");
                    continue;
                }
                tracing::info!(
                    session_id = %session_id,
                    request_id = %request_id,
                    behavior = %behavior,
                    "TUI permission response received"
                );
                let allow = {
                    let b = behavior.to_ascii_lowercase();
                    b.starts_with("allow") || b == "accept" || b == "approved"
                };
                let Some(resolved_session_id) =
                    permission_target(&state.permission_store, session_id, &request_id, behavior)
                        .await
                else {
                    tracing::warn!(%request_id, user_id = %ctx.user_id, "tui_ws: permission-response for another session's request");
                    continue;
                };
                // Push the decision down to the adapter so blocking agents
                // (e.g. the codex app-server, which holds the turn open until
                // it gets a reply) are unblocked.
                let dispatch = crate::bus::dispatch(
                    &state,
                    &resolved_session_id,
                    cctui_proto::adapter::AdapterCommand::PermissionResponse {
                        local_id: resolved_session_id.clone(),
                        request_id: request_id.clone(),
                        allow,
                    },
                )
                .await;
                if let Err(err) = dispatch {
                    use crate::bus::BusError;
                    match err {
                        BusError::NoDaemon(_) | BusError::NoAdapter | BusError::NotFound => {
                            tracing::debug!(%resolved_session_id, ?err, "permission dispatch skipped");
                        }
                        _ => {
                            tracing::warn!(%resolved_session_id, %err, "permission dispatch failed");
                        }
                    }
                }
                state.bus.publish_server(ServerEvent::PermissionResolved {
                    session_id: resolved_session_id,
                    request_id,
                });
            }
        }
    }

    for (_, handle) in sub_handles {
        handle.abort();
    }
    // Decrement every terminal this socket still watched so a browser that
    // closed the tab (or dropped) releases the daemon PTY stream.
    for session_id in pty_watches {
        if state.bus.pty_watch_dec(&session_id) {
            set_daemon_pty_watch(&state, &session_id, false).await;
        }
    }
}

/// The session a client's permission decision is forwarded to, or `None` when
/// `request_id` is pending for a session other than the (authorized)
/// `session_id` the client named.
async fn permission_target(
    store: &crate::routes::permissions::SharedPermissionStore,
    session_id: String,
    request_id: &str,
    behavior: String,
) -> Option<String> {
    use crate::routes::permissions::SessionDecision;
    match store.write().await.record_session_decision(&session_id, request_id, behavior) {
        SessionDecision::Foreign => None,
        SessionDecision::Recorded | SessionDecision::Unknown => Some(session_id),
    }
}

/// Toggle this socket's live-terminal watch of `session_id`. Ref-count
/// per session is on the bus; only the 0↔1 edge tells the daemon to start/stop
/// its viewer PTY attach. Idempotent per socket via `pty_watches`.
async fn handle_watch_terminal(
    state: &AppState,
    session_id: &str,
    watch: bool,
    pty_watches: &mut std::collections::HashSet<String>,
) {
    if watch {
        if !pty_watches.insert(session_id.to_owned()) {
            return;
        }
        if state.bus.pty_watch_inc(session_id) {
            set_daemon_pty_watch(state, session_id, true).await;
        }
    } else {
        if !pty_watches.remove(session_id) {
            return;
        }
        if state.bus.pty_watch_dec(session_id) {
            set_daemon_pty_watch(state, session_id, false).await;
        }
    }
}

/// Tell the session's daemon to start/stop relaying its PTY. Best-effort: a
/// session whose daemon is momentarily offline just gets no stream (the browser
/// re-sends `watch` on reconnect), so `NoDaemon`/`NotFound` are logged at debug.
async fn set_daemon_pty_watch(state: &AppState, session_id: &str, watch: bool) {
    let dispatch = crate::bus::dispatch(
        state,
        session_id,
        cctui_proto::adapter::AdapterCommand::WatchPty { local_id: session_id.to_owned(), watch },
    )
    .await;
    if let Err(err) = dispatch {
        tracing::debug!(%session_id, watch, %err, "watch-terminal daemon dispatch skipped");
    }
}

/// The resource whose owner may receive an event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Owned {
    Session(String),
    Machine(uuid::Uuid),
    Account(uuid::Uuid),
    Dispatcher(uuid::Uuid),
}

/// Who a broadcast event reaches besides admins.
#[derive(Debug, PartialEq, Eq)]
enum Audience {
    Everyone,
    OwnerOf(Owned),
    AdminsOnly,
}

/// Exhaustive on purpose: a new variant must declare who may see it.
fn audience(event: &ServerEvent) -> Audience {
    use Audience::{AdminsOnly, Everyone, OwnerOf};
    let session = |id: &str| OwnerOf(Owned::Session(id.to_owned()));
    match event {
        ServerEvent::Stream { session_id, .. }
        | ServerEvent::Status { session_id, .. }
        | ServerEvent::SessionDeregistered { session_id }
        | ServerEvent::PermissionRequest { session_id, .. }
        | ServerEvent::PermissionResolved { session_id, .. }
        | ServerEvent::AskQuestion { session_id, .. }
        | ServerEvent::AskResolved { session_id }
        | ServerEvent::PlanRequest { session_id, .. }
        | ServerEvent::PlanResolved { session_id }
        | ServerEvent::PtyChunk { session_id, .. }
        | ServerEvent::SessionEnded { session_id, .. }
        | ServerEvent::MessageAck { session_id, .. }
        | ServerEvent::SoftLimitReached { session_id, .. }
        | ServerEvent::SoftLimitCleared { session_id } => session(session_id),
        ServerEvent::SessionRegistered { session: s } => session(&s.id),
        ServerEvent::CommandResult { session_id, .. } => {
            session_id.as_deref().map_or(AdminsOnly, session)
        }
        ServerEvent::ArchiveManifest { machine_id, .. }
        | ServerEvent::ArchiveUploaded { machine_id, .. }
        | ServerEvent::MachineLiveness { machine_id, .. }
        | ServerEvent::MachineResources { machine_id, .. } => OwnerOf(Owned::Machine(*machine_id)),
        ServerEvent::AccountUsage { account_id, .. } => OwnerOf(Owned::Account(*account_id)),
        ServerEvent::DispatcherLiveness { dispatcher_id, .. } => {
            OwnerOf(Owned::Dispatcher(*dispatcher_id))
        }
        ServerEvent::GithubEvent { .. } => AdminsOnly,
        ServerEvent::Heartbeat {} | ServerEvent::Resync { .. } => Everyone,
    }
}

/// Resolves the owning user of a resource; `None` when unknown.
trait OwnerLookup {
    async fn owner(&self, owned: &Owned) -> Option<uuid::Uuid>;
}

impl OwnerLookup for sqlx::PgPool {
    async fn owner(&self, owned: &Owned) -> Option<uuid::Uuid> {
        let found = match owned {
            Owned::Session(id) => crate::authz::session_owner(id, self).await,
            Owned::Machine(id) => {
                sqlx::query_scalar("SELECT user_id FROM machines WHERE id = $1")
                    .bind(id)
                    .fetch_optional(self)
                    .await
            }
            Owned::Account(id) => {
                sqlx::query_scalar("SELECT user_id FROM accounts WHERE id = $1")
                    .bind(id)
                    .fetch_optional(self)
                    .await
            }
            Owned::Dispatcher(id) => {
                sqlx::query_scalar(
                    "SELECT user_id FROM dispatchers WHERE id = $1 AND deleted_at IS NULL",
                )
                .bind(id)
                .fetch_optional(self)
                .await
            }
        };
        found.unwrap_or_else(|e| {
            tracing::error!(?owned, "db error (ws event authz): {e}");
            None
        })
    }
}

const OWNER_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const UNKNOWN_OWNER_TTL: std::time::Duration = std::time::Duration::from_secs(5);
const OWNER_CACHE_MAX: usize = 4096;

/// Per-socket memo of resource owners, so a busy session costs one lookup per
/// socket per TTL instead of one per event.
#[derive(Default)]
struct OwnerCache {
    entries: std::collections::HashMap<Owned, (Option<uuid::Uuid>, tokio::time::Instant)>,
}

impl OwnerCache {
    async fn owner(&mut self, owned: &Owned, lookup: &impl OwnerLookup) -> Option<uuid::Uuid> {
        let now = tokio::time::Instant::now();
        if let Some(&(owner, expires)) = self.entries.get(owned)
            && expires > now
        {
            return owner;
        }
        let owner = lookup.owner(owned).await;
        if self.entries.len() >= OWNER_CACHE_MAX {
            self.entries.retain(|_, (_, expires)| *expires > now);
            if self.entries.len() >= OWNER_CACHE_MAX {
                self.entries.clear();
            }
        }
        let ttl = if owner.is_some() { OWNER_TTL } else { UNKNOWN_OWNER_TTL };
        self.entries.insert(owned.clone(), (owner, now + ttl));
        owner
    }

    fn forget(&mut self, owned: &Owned) {
        self.entries.remove(owned);
    }
}

/// Decides whether a server-wide event may reach one socket's principal.
trait EventFilter {
    async fn allows(&mut self, event: &ServerEvent) -> bool;
}

/// Admins see everything; everyone else only events whose resource they own.
/// An event without a resolvable owner is denied.
struct OwnerFilter<L> {
    is_admin: bool,
    user_id: uuid::Uuid,
    lookup: L,
    cache: OwnerCache,
}

impl<L: OwnerLookup> EventFilter for OwnerFilter<L> {
    async fn allows(&mut self, event: &ServerEvent) -> bool {
        if self.is_admin {
            return true;
        }
        match audience(event) {
            Audience::Everyone => true,
            Audience::AdminsOnly => false,
            Audience::OwnerOf(owned) => {
                // A (re)registration may have moved the session to another machine.
                if matches!(event, ServerEvent::SessionRegistered { .. }) {
                    self.cache.forget(&owned);
                }
                let owner = self.cache.owner(&owned, &self.lookup).await;
                if matches!(event, ServerEvent::SessionDeregistered { .. }) {
                    self.cache.forget(&owned);
                }
                owner == Some(self.user_id)
            }
        }
    }
}

/// Forward every permitted frame to the socket as-is. Events lost to lag are
/// unrecoverable, so the client is told to refetch instead.
async fn relay_server_frames(
    mut receiver: broadcast::Receiver<crate::bus::ServerFrame>,
    mut filter: impl EventFilter,
    event_tx: FrameTx,
) {
    loop {
        match receiver.recv().await {
            Ok(frame) => {
                if filter.allows(&frame.event).await && event_tx.send(frame.json).await.is_err() {
                    break;
                }
            }
            Err(RecvError::Closed) => break,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "TUI server-event relay lagged");
                if !send_event(&event_tx, &ServerEvent::Resync { session_id: None }).await {
                    break;
                }
            }
        }
    }
}

async fn handle_tui_ws(socket: WebSocket, state: AppState, ctx: AuthContext) {
    let (sink, stream) = socket.split();
    let (tx, rx) = mpsc::channel::<Frame>(256);

    let filter = OwnerFilter {
        is_admin: ctx.is_admin(),
        user_id: ctx.user_id,
        lookup: state.pool.clone(),
        cache: OwnerCache::default(),
    };
    tokio::spawn(relay_server_frames(state.bus.subscribe_server(), filter, tx.clone()));

    spawn_send_task(sink, rx);
    run_tui_socket(stream, state, ctx, tx).await;

    tracing::debug!("TUI WebSocket disconnected");
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use cctui_proto::models::{Session, SessionStatus};
    use cctui_proto::ws::ServerEvent;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use uuid::Uuid;

    use super::{
        Audience, EventFilter, OWNER_TTL, Owned, OwnerCache, OwnerFilter, OwnerLookup, audience,
        origin_permitted, permission_target, relay_server_frames, replay_pending, spawn_relay_task,
    };
    use crate::bus::ServerFrame;
    use crate::config::Config;
    use crate::routes::permissions::{PendingPermission, PermissionStore};

    fn cfg() -> Config {
        Config::for_test(vec!["https://cctui.example.com".to_owned()])
    }

    #[test]
    fn absent_origin_is_allowed() {
        assert!(origin_permitted(&cfg(), &HeaderMap::new()));
    }

    #[test]
    fn allowlisted_origin_is_allowed() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://cctui.example.com"));
        assert!(origin_permitted(&cfg(), &headers));
    }

    #[test]
    fn foreign_origin_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example.com"));
        assert!(!origin_permitted(&cfg(), &headers));
    }

    fn session(id: &str) -> Session {
        let now = chrono::Utc::now();
        Session {
            id: id.to_owned(),
            parent_id: None,
            account_id: None,
            machine_id: "m1".to_owned(),
            working_dir: "/tmp".to_owned(),
            status: SessionStatus::Active,
            registered_at: now,
            last_heartbeat: now,
            metadata: serde_json::json!({}),
            adapter_id: None,
        }
    }

    #[test]
    fn session_registered_is_owner_scoped() {
        let event = ServerEvent::SessionRegistered { session: session("sess-1") };
        assert_eq!(audience(&event), Audience::OwnerOf(Owned::Session("sess-1".into())));
    }

    #[test]
    fn unowned_command_result_is_admin_only() {
        let event = ServerEvent::CommandResult {
            command_id: "c".into(),
            ok: true,
            error: None,
            session_id: None,
        };
        assert_eq!(audience(&event), Audience::AdminsOnly);
    }

    const ALICE: Uuid = Uuid::from_u128(1);
    const BOB: Uuid = Uuid::from_u128(2);
    const BOB_MACHINE: Uuid = Uuid::from_u128(20);
    const ALICE_MACHINE: Uuid = Uuid::from_u128(10);

    /// Every resource named `*bob*` / `BOB_*` belongs to Bob, the rest to Alice.
    #[derive(Default, Clone)]
    struct FakeOwners {
        calls: Arc<AtomicUsize>,
    }

    impl OwnerLookup for FakeOwners {
        async fn owner(&self, owned: &Owned) -> Option<Uuid> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match owned {
                Owned::Session(id) if id.contains("ghost") => None,
                Owned::Session(id) if id.contains("bob") => Some(BOB),
                Owned::Machine(id) if *id == BOB_MACHINE => Some(BOB),
                _ => Some(ALICE),
            }
        }
    }

    fn filter_for(user_id: Uuid, lookup: FakeOwners) -> OwnerFilter<FakeOwners> {
        OwnerFilter { is_admin: false, user_id, lookup, cache: OwnerCache::default() }
    }

    fn foreign_events() -> Vec<ServerEvent> {
        vec![
            ServerEvent::SoftLimitReached {
                session_id: "sess-bob".into(),
                account_id: Uuid::from_u128(30),
                account_name: "bob-max".into(),
                reason: "cap".into(),
                retry_after_secs: 60,
            },
            ServerEvent::MachineResources {
                machine_id: BOB_MACHINE,
                resources: Default::default(),
            },
            ServerEvent::ArchiveUploaded {
                machine_id: BOB_MACHINE,
                project_dir: "/home/bob/secret".into(),
                session_id: "sess-bob".into(),
                size_bytes: 1,
                sha256: String::new(),
            },
            ServerEvent::MachineLiveness {
                machine_id: BOB_MACHINE,
                liveness: cctui_proto::models::MachineLiveness::Online,
            },
        ]
    }

    #[tokio::test]
    async fn non_admin_does_not_receive_another_users_events() {
        let mut alice = filter_for(ALICE, FakeOwners::default());
        for event in foreign_events() {
            assert!(!alice.allows(&event).await, "leaked to a non-owner: {event:?}");
        }
        let github = ServerEvent::GithubEvent {
            kind: serde_json::from_value(serde_json::json!("pull")).unwrap(),
            payload: serde_json::from_value(serde_json::json!({
                "connector_id": Uuid::nil(),
                "repo": "o/r",
            }))
            .unwrap(),
        };
        assert!(!alice.allows(&github).await);
        let ghost = ServerEvent::SessionDeregistered { session_id: "sess-ghost".into() };
        assert!(!alice.allows(&ghost).await);
    }

    fn bob_stream() -> ServerEvent {
        ServerEvent::PtyChunk { session_id: "sess-bob".into(), data: String::new() }
    }

    #[tokio::test]
    async fn many_events_for_one_foreign_session_cost_one_lookup() {
        let owners = FakeOwners::default();
        let mut alice = filter_for(ALICE, owners.clone());
        for _ in 0..100 {
            assert!(!alice.allows(&bob_stream()).await);
        }
        assert_eq!(owners.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cached_owner_expires_and_registration_refreshes_it() {
        let owners = FakeOwners::default();
        let mut alice = filter_for(ALICE, owners.clone());
        alice.allows(&bob_stream()).await;
        tokio::time::advance(OWNER_TTL + std::time::Duration::from_secs(1)).await;
        alice.allows(&bob_stream()).await;
        assert_eq!(owners.calls.load(Ordering::SeqCst), 2);

        let mut moved = session("sess-bob");
        moved.machine_id = "m2".into();
        alice.allows(&ServerEvent::SessionRegistered { session: moved }).await;
        assert_eq!(owners.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn owner_and_admin_receive_the_events() {
        let mut bob = filter_for(BOB, FakeOwners::default());
        for event in foreign_events() {
            assert!(bob.allows(&event).await, "withheld from its owner: {event:?}");
        }
        let mine = ServerEvent::MachineResources {
            machine_id: ALICE_MACHINE,
            resources: Default::default(),
        };
        assert!(filter_for(ALICE, FakeOwners::default()).allows(&mine).await);

        let mut admin = filter_for(ALICE, FakeOwners::default());
        admin.is_admin = true;
        for event in foreign_events() {
            assert!(admin.allows(&event).await);
        }
    }

    fn pending(session_id: &str, request_id: &str) -> PendingPermission {
        PendingPermission {
            session_id: session_id.into(),
            request_id: request_id.into(),
            tool_name: "Bash".into(),
            description: String::new(),
            input_preview: String::new(),
            received_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn permission_response_for_foreign_request_is_not_dispatched() {
        let store = PermissionStore::shared();
        store.write().await.insert_request(pending("sess-b", "req-b"));

        let target = permission_target(&store, "sess-a".into(), "req-b", "allow".into()).await;
        assert_eq!(target, None);
        let kept = store.read().await.list_pending();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].session_id, "sess-b");
    }

    #[tokio::test]
    async fn permission_response_for_own_request_is_dispatched() {
        let store = PermissionStore::shared();
        store.write().await.insert_request(pending("sess-a", "req-a"));

        let target = permission_target(&store, "sess-a".into(), "req-a", "allow".into()).await;
        assert_eq!(target.as_deref(), Some("sess-a"));
        assert!(store.read().await.list_pending().is_empty());

        let stale = permission_target(&store, "sess-a".into(), "req-a", "allow".into()).await;
        assert_eq!(stale.as_deref(), Some("sess-a"));
    }

    #[tokio::test]
    async fn replay_to_a_full_socket_does_not_hold_the_permission_store() {
        let store = PermissionStore::shared();
        store.write().await.insert_request(pending("sess-a", "req-a"));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        tx.send(r#"{"type":"heartbeat"}"#.into()).await.unwrap();

        let replay = {
            let store = store.clone();
            let tx = tx.clone();
            tokio::spawn(async move { replay_pending(&store, "sess-a", &tx).await })
        };
        tokio::task::yield_now().await;

        let write = tokio::time::timeout(std::time::Duration::from_secs(2), store.write()).await;
        assert!(write.is_ok(), "permission_store.write() blocked behind a stalled replay");
        drop(write);
        assert!(!replay.is_finished());
        replay.abort();
    }

    struct AllowAll;
    impl EventFilter for AllowAll {
        async fn allows(&mut self, _event: &ServerEvent) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn lagged_server_relay_sends_resync() {
        let (bus_tx, bus_rx) = tokio::sync::broadcast::channel(1);
        for id in ["a", "b", "c"] {
            let event = ServerEvent::SessionDeregistered { session_id: id.into() };
            bus_tx.send(ServerFrame::encode(event).unwrap()).unwrap();
        }
        drop(bus_tx);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        relay_server_frames(bus_rx, AllowAll, tx).await;

        assert_eq!(&*rx.recv().await.unwrap(), r#"{"type":"resync"}"#);
        assert_eq!(
            &*rx.recv().await.unwrap(),
            r#"{"type":"session_deregistered","session_id":"c"}"#
        );
    }

    #[tokio::test]
    async fn lagged_session_relay_sends_resync_for_that_session() {
        let (stream_tx, stream_rx) = tokio::sync::broadcast::channel(1);
        for _ in 0..3 {
            let beat = cctui_proto::ws::AgentEvent::Heartbeat {
                tokens_in: 0,
                tokens_out: 0,
                cost_usd: 0.0,
                ts: 0,
                seq: None,
            };
            stream_tx.send(beat).ok();
        }
        drop(stream_tx);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        spawn_relay_task(stream_rx, "sess-1".into(), tx).await.unwrap();

        assert_eq!(&*rx.recv().await.unwrap(), r#"{"type":"resync","session_id":"sess-1"}"#);
    }

    #[test]
    fn session_deregistered_is_owner_scoped() {
        let event = ServerEvent::SessionDeregistered { session_id: "sess-2".to_owned() };
        assert_eq!(audience(&event), Audience::OwnerOf(Owned::Session("sess-2".into())));
    }
}
