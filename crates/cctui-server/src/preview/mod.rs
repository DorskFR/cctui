//! Dev-server previews: a session's daemon registers a loopback port, the
//! server serves it on a dedicated host (`CCTUI_PREVIEW_HOST`, e.g.
//! `cctui-pv-{id}.example`) and tunnels every request down the daemon WS.
//! Owner-only: the browser gets a host-only cookie by redeeming a
//! short-lived ticket minted through the session API.

pub mod csrf;
#[cfg(test)]
mod e2e;
pub mod forward;
pub mod handler;
pub mod routes;
pub mod store;
pub mod ticket;

use std::sync::Arc;

use bytes::Bytes;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp, PreviewChunk, PreviewHeader};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::state::AppState;

pub const MAX_PER_SESSION: usize = 8;
pub const MIN_PORT: u16 = 1024;
/// Response body chunks buffered per stream before the daemon link is
/// paused (each chunk ≤ `PREVIEW_CHUNK_BYTES`).
const BODY_BACKLOG: usize = 32;

/// `CCTUI_PREVIEW_HOST` pattern split around its `{id}` placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewHost {
    prefix: String,
    suffix: String,
}

impl PreviewHost {
    pub fn parse(pattern: &str) -> anyhow::Result<Self> {
        let pattern = pattern.trim().to_ascii_lowercase();
        let Some((prefix, suffix)) = pattern.split_once("{id}") else {
            anyhow::bail!("CCTUI_PREVIEW_HOST must contain one `{{id}}` placeholder");
        };
        if suffix.contains("{id}") {
            anyhow::bail!("CCTUI_PREVIEW_HOST must contain `{{id}}` exactly once");
        }
        if suffix.is_empty() {
            anyhow::bail!("CCTUI_PREVIEW_HOST needs a suffix after `{{id}}` (a domain)");
        }
        if prefix.contains('/') || suffix.contains('/') || pattern.contains(':') {
            anyhow::bail!("CCTUI_PREVIEW_HOST is a bare host pattern, without scheme or port");
        }
        Ok(Self { prefix: prefix.to_owned(), suffix: suffix.to_owned() })
    }

    /// The preview id embedded in a `Host` header value, ignoring any port.
    #[must_use]
    pub fn id_from_host(&self, host: &str) -> Option<String> {
        let host = host.trim().to_ascii_lowercase();
        let host = host.rsplit_once(':').map_or(host.as_str(), |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) { h } else { host.as_str() }
        });
        let id = host.strip_prefix(&self.prefix)?.strip_suffix(&self.suffix)?;
        (!id.is_empty() && is_preview_id(id)).then(|| id.to_owned())
    }

    #[must_use]
    pub fn host_for(&self, id: &str) -> String {
        format!("{}{id}{}", self.prefix, self.suffix)
    }
}

#[must_use]
pub fn is_preview_id(id: &str) -> bool {
    id.len() >= 16 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// 24 chars of `[a-z2-7]` from 120 bits of OS randomness.
#[must_use]
pub fn new_preview_id() -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    let mut out = String::with_capacity(24);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 && out.len() < 24 {
            bits -= 5;
            out.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
        if out.len() == 24 {
            break;
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub id: String,
    pub session_id: String,
    pub user_id: Uuid,
    pub machine_id: Uuid,
    pub port: u16,
    pub opened_at: DateTime<Utc>,
}

/// Response head relayed from the daemon.
#[derive(Debug, Clone)]
pub struct Head {
    pub status: u16,
    pub headers: Vec<PreviewHeader>,
}

/// One relayed body piece (HTTP body bytes, or one WebSocket message).
#[derive(Debug, Clone)]
pub struct Inbound {
    pub data: Bytes,
    pub text: bool,
    pub end: bool,
}

pub type OpenedStream =
    (String, oneshot::Receiver<Result<Head, String>>, mpsc::Receiver<Result<Inbound, String>>);

struct Stream {
    preview_id: String,
    head: std::sync::Mutex<Option<oneshot::Sender<Result<Head, String>>>>,
    body: mpsc::Sender<Result<Inbound, String>>,
}

#[derive(Debug)]
pub enum OpenError {
    Disabled,
    Limit,
    Port,
    Taken,
    Db,
}

impl OpenError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => "previews are not enabled on this server".to_owned(),
            Self::Limit => format!("session already has {MAX_PER_SESSION} open previews"),
            Self::Port => format!("previews only tunnel ports >= {MIN_PORT}"),
            Self::Taken => "that session and port belong to another user".to_owned(),
            Self::Db => "could not register the preview".to_owned(),
        }
    }
}

pub struct Registry {
    host: Option<PreviewHost>,
    external_url: String,
    previews: DashMap<String, Preview>,
    streams: DashMap<String, Arc<Stream>>,
    tickets: ticket::Signer,
}

impl Registry {
    #[must_use]
    pub fn new(host: Option<PreviewHost>, external_url: &str, key: &[u8]) -> Self {
        Self {
            host,
            external_url: external_url.to_owned(),
            previews: DashMap::new(),
            streams: DashMap::new(),
            tickets: ticket::Signer::new(key),
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.host.is_some()
    }

    #[must_use]
    pub const fn host(&self) -> Option<&PreviewHost> {
        self.host.as_ref()
    }

    #[must_use]
    pub const fn tickets(&self) -> &ticket::Signer {
        &self.tickets
    }

    #[must_use]
    pub fn url_for(&self, id: &str) -> String {
        let scheme = if self.external_url.starts_with("http://") { "http" } else { "https" };
        let host = self.host.as_ref().map_or_else(|| id.to_owned(), |h| h.host_for(id));
        format!("{scheme}://{host}")
    }

    /// Register (or return the already open) preview for `session_id:port`.
    /// Register a preview and bind its daemon link to this pod. `requested_id`
    /// is a daemon re-announcing after a reconnect: the existing row is adopted
    /// only when machine, session, user and port all still match.
    pub async fn open(
        &self,
        pool: &sqlx::PgPool,
        session_id: &str,
        user_id: Uuid,
        machine_id: Uuid,
        port: u16,
        requested_id: Option<&str>,
    ) -> Result<Preview, OpenError> {
        if !self.enabled() {
            return Err(OpenError::Disabled);
        }
        if port < MIN_PORT {
            return Err(OpenError::Port);
        }
        if let Some(requested) = requested_id.filter(|id| is_preview_id(id))
            && let Some(preview) =
                store::rebind(pool, requested, session_id, user_id, machine_id, port).await
        {
            self.bind_local(&preview);
            return Ok(preview);
        }
        if let Some(existing) = store::by_port(pool, session_id, port).await {
            if existing.user_id != user_id {
                return Err(OpenError::Taken);
            }
            let preview = store::insert(pool, &Preview { machine_id, ..existing })
                .await
                .map_err(|_| OpenError::Db)?;
            self.bind_local(&preview);
            return Ok(preview);
        }
        if store::check_limit(pool, session_id).await.is_err() {
            return Err(OpenError::Limit);
        }
        let preview = store::insert(
            pool,
            &Preview {
                id: new_preview_id(),
                session_id: session_id.to_owned(),
                user_id,
                machine_id,
                port,
                opened_at: Utc::now(),
            },
        )
        .await
        .map_err(|_| OpenError::Db)?;
        self.bind_local(&preview);
        Ok(preview)
    }

    fn bind_local(&self, preview: &Preview) {
        self.previews.insert(preview.id.clone(), preview.clone());
    }

    /// The preview whose daemon WS this pod terminates. `None` means the
    /// request has to be forwarded to (or is simply unknown by) another pod.
    #[must_use]
    pub fn local(&self, id: &str) -> Option<Preview> {
        self.previews.get(id).map(|p| p.clone())
    }

    /// Any pod's view of a preview, from the shared table.
    pub async fn get(&self, pool: &sqlx::PgPool, id: &str) -> Option<Preview> {
        if !self.enabled() {
            return None;
        }
        store::lookup(pool, id).await
    }

    pub async fn list(&self, pool: &sqlx::PgPool, session_id: &str) -> Vec<Preview> {
        store::list_session(pool, session_id).await
    }

    /// Forget a preview everywhere: the shared row and this pod's streams.
    pub async fn close(&self, pool: &sqlx::PgPool, id: &str) -> Option<Preview> {
        let preview = store::lookup(pool, id).await;
        store::delete(pool, id).await;
        self.forget_local(id);
        preview
    }

    fn forget_local(&self, id: &str) {
        self.previews.remove(id);
        self.streams.retain(|_, s| s.preview_id != id);
    }

    pub async fn close_port(
        &self,
        pool: &sqlx::PgPool,
        session_id: &str,
        port: u16,
    ) -> Option<Preview> {
        let preview = store::by_port(pool, session_id, port).await?;
        self.close(pool, &preview.id).await
    }

    /// The session is over: its previews go for good.
    pub async fn close_session(&self, pool: &sqlx::PgPool, session_id: &str) -> Vec<Preview> {
        let previews = store::list_session(pool, session_id).await;
        store::delete_session(pool, session_id).await;
        for preview in &previews {
            self.forget_local(&preview.id);
        }
        previews
    }

    /// A daemon dropped its WS. The rows survive the grace period so a rolling
    /// restart can re-announce them, but this pod no longer serves them.
    pub async fn detach_session(&self, pool: &sqlx::PgPool, session_id: &str) {
        store::detach_session(pool, session_id).await;
        let ids: Vec<String> = self
            .previews
            .iter()
            .filter(|p| p.session_id == session_id)
            .map(|p| p.id.clone())
            .collect();
        for id in &ids {
            self.forget_local(id);
        }
    }

    pub async fn detach_machine(&self, pool: &sqlx::PgPool, machine_id: Uuid) {
        store::detach_machine(pool, machine_id).await;
        let ids: Vec<String> = self
            .previews
            .iter()
            .filter(|p| p.machine_id == machine_id)
            .map(|p| p.id.clone())
            .collect();
        for id in &ids {
            self.forget_local(id);
        }
    }

    /// Park a new tunnelled stream; the daemon's head and body arrive on the
    /// returned receivers. Bounded: a slow browser pauses the daemon link
    /// once `BODY_BACKLOG` chunks are queued instead of buffering in memory.
    pub fn open_stream(&self, preview_id: &str) -> OpenedStream {
        let stream_id = Uuid::new_v4().simple().to_string();
        let (head_tx, head_rx) = oneshot::channel();
        let (body_tx, body_rx) = mpsc::channel(BODY_BACKLOG);
        self.streams.insert(
            stream_id.clone(),
            Arc::new(Stream {
                preview_id: preview_id.to_owned(),
                head: std::sync::Mutex::new(Some(head_tx)),
                body: body_tx,
            }),
        );
        (stream_id, head_rx, body_rx)
    }

    pub fn drop_stream(&self, stream_id: &str) {
        self.streams.remove(stream_id);
    }

    #[cfg(test)]
    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    fn stream(&self, stream_id: &str) -> Option<Arc<Stream>> {
        self.streams.get(stream_id).map(|s| s.clone())
    }
}

async fn send_down(
    state: &AppState,
    machine_id: Uuid,
    session_id: &str,
    frame: DaemonFrameDown,
) -> bool {
    state.bus.command_daemon_local_for_session(machine_id, session_id, frame).await.is_ok()
}

async fn session_owner_on_machine(
    state: &AppState,
    session_id: &str,
    machine_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT user_id FROM sessions WHERE id = $1 AND machine_id = $2")
            .bind(session_id)
            .bind(machine_id.to_string())
            .fetch_optional(&state.pool)
            .await?;
    Ok(row.map(|(owner,)| owner.unwrap_or(Uuid::nil())))
}

/// Daemon-originated preview frames: registrations and relayed responses.
pub async fn on_frame(state: &AppState, machine_id: Uuid, daemon_user: Uuid, frame: DaemonFrameUp) {
    let registry = &state.preview;
    match frame {
        DaemonFrameUp::PreviewOpen { request_id, session_id, port, preview_id } => {
            let owner = match session_owner_on_machine(state, &session_id, machine_id).await {
                Ok(Some(owner)) => Some(if owner.is_nil() { daemon_user } else { owner }),
                Ok(None) => None,
                Err(e) => {
                    tracing::error!("db error (preview open): {e}");
                    None
                }
            };
            let outcome = match owner {
                Some(owner) => registry
                    .open(&state.pool, &session_id, owner, machine_id, port, preview_id.as_deref())
                    .await
                    .map_err(|e| e.message()),
                None => Err("session is not running on this machine".to_owned()),
            };
            let reply = match outcome {
                Ok(preview) => DaemonFrameDown::PreviewOpened {
                    request_id,
                    ok: true,
                    url: Some(registry.url_for(&preview.id)),
                    preview_id: Some(preview.id),
                    error: None,
                },
                Err(error) => DaemonFrameDown::PreviewOpened {
                    request_id,
                    ok: false,
                    preview_id: None,
                    url: None,
                    error: Some(error),
                },
            };
            send_down(state, machine_id, &session_id, reply).await;
        }
        DaemonFrameUp::PreviewClose { session_id, port } => {
            registry.close_port(&state.pool, &session_id, port).await;
        }
        DaemonFrameUp::PreviewResponse { stream_id, status, headers } => {
            let Some(stream) = registry.stream(&stream_id) else { return };
            let waiter =
                stream.head.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            if let Some(waiter) = waiter {
                let _ = waiter.send(Ok(Head { status, headers }));
            }
        }
        DaemonFrameUp::PreviewChunk(PreviewChunk { stream_id, data, text, end }) => {
            let Some(stream) = registry.stream(&stream_id) else { return };
            let decoded = {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.decode(data)
            };
            let item = decoded
                .map(|data| Inbound { data: Bytes::from(data), text, end })
                .map_err(|e| format!("malformed preview chunk: {e}"));
            let closes = end || item.is_err();
            if stream.body.send(item).await.is_err() {
                registry.drop_stream(&stream_id);
                abort_stream(state, registry, &stream.preview_id, &stream_id).await;
            } else if closes {
                registry.drop_stream(&stream_id);
            }
        }
        DaemonFrameUp::PreviewError { stream_id, error } => {
            let Some(stream) = registry.stream(&stream_id) else { return };
            registry.drop_stream(&stream_id);
            let waiter =
                stream.head.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            match waiter {
                Some(waiter) => {
                    let _ = waiter.send(Err(error));
                }
                None => {
                    let _ = stream.body.send(Err(error)).await;
                }
            }
        }
        _ => {}
    }
}

/// Tell the daemon the browser side of a stream went away.
pub async fn abort_stream(
    state: &AppState,
    registry: &Registry,
    preview_id: &str,
    stream_id: &str,
) {
    registry.drop_stream(stream_id);
    let Some(preview) = registry.local(preview_id) else { return };
    send_down(
        state,
        preview.machine_id,
        &preview.session_id,
        DaemonFrameDown::PreviewAbort { stream_id: stream_id.to_owned() },
    )
    .await;
}

/// Close every preview of `session_id` and let its daemon forget them.
pub async fn close_session(state: &AppState, session_id: &str) {
    for preview in state.preview.close_session(&state.pool, session_id).await {
        send_down(
            state,
            preview.machine_id,
            session_id,
            DaemonFrameDown::PreviewClosed {
                session_id: session_id.to_owned(),
                port: preview.port,
            },
        )
        .await;
    }
}

#[must_use]
pub const fn is_preview_frame(frame: &DaemonFrameUp) -> bool {
    matches!(
        frame,
        DaemonFrameUp::PreviewOpen { .. }
            | DaemonFrameUp::PreviewClose { .. }
            | DaemonFrameUp::PreviewResponse { .. }
            | DaemonFrameUp::PreviewChunk(_)
            | DaemonFrameUp::PreviewError { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::new(
            Some(PreviewHost::parse("cctui-pv-{id}.example.test").unwrap()),
            "https://cctui.example.test",
            b"test-key",
        )
    }

    #[test]
    fn host_pattern_parses_and_extracts_ids() {
        let host = PreviewHost::parse("cctui-pv-{id}.dorsk.dev").unwrap();
        assert_eq!(host.host_for("abcdefghijklmnop"), "cctui-pv-abcdefghijklmnop.dorsk.dev");
        assert_eq!(
            host.id_from_host("CCTUI-PV-abcdefghijklmnop.dorsk.dev:443").as_deref(),
            Some("abcdefghijklmnop")
        );
        assert_eq!(host.id_from_host("cctui.dorsk.dev"), None);
        assert_eq!(host.id_from_host("cctui-pv-.dorsk.dev"), None);
        assert_eq!(host.id_from_host("cctui-pv-short.dorsk.dev"), None);
        assert_eq!(host.id_from_host("cctui-pv-abcdefghijklmnop.dorsk.dev.evil"), None);
        assert_eq!(host.id_from_host("cctui-pv-ABC_DEFGHIJKLMNOP.dorsk.dev"), None);
        for bad in ["nothing", "{id}", "a{id}b{id}c", "https://{id}.x", "{id}.x:80"] {
            assert!(PreviewHost::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn preview_ids_are_long_lowercase_and_unique() {
        let a = new_preview_id();
        let b = new_preview_id();
        assert_eq!(a.len(), 24);
        assert!(is_preview_id(&a));
        assert_ne!(a, b);
    }

    async fn test_user(pool: &sqlx::PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("preview-{id}"))
            .bind(format!("hash-{id}"))
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn registry_lifecycle_open_close_session_machine() {
        let Some(url) = crate::routes::gateway::test_db_url("preview_registry_lifecycle") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let reg = registry();
        let user = test_user(&pool).await;
        let machine = Uuid::new_v4();
        let (s1, s2) = (format!("s1-{}", Uuid::new_v4()), format!("s2-{}", Uuid::new_v4()));

        let p = reg.open(&pool, &s1, user, machine, 5173, None).await.unwrap();
        assert_eq!(reg.url_for(&p.id), format!("https://cctui-pv-{}.example.test", p.id));
        assert_eq!(
            reg.open(&pool, &s1, user, machine, 5173, None).await.unwrap().id,
            p.id,
            "same port re-open keeps the id"
        );
        assert!(matches!(
            reg.open(&pool, &s1, user, machine, 80, None).await,
            Err(OpenError::Port)
        ));
        for port in 0..7u16 {
            reg.open(&pool, &s1, user, machine, 6000 + port, None).await.unwrap();
        }
        assert!(matches!(
            reg.open(&pool, &s1, user, machine, 7000, None).await,
            Err(OpenError::Limit)
        ));
        assert_eq!(reg.list(&pool, &s1).await.len(), 8);

        // A reconnect re-announcing its id adopts the row; a mismatched
        // session/user/machine never does.
        reg.detach_machine(&pool, machine).await;
        assert!(reg.local(&p.id).is_none(), "a detached preview is not served by this pod");
        let rebound = reg.open(&pool, &s1, user, machine, 5173, Some(&p.id)).await.unwrap();
        assert_eq!(rebound.id, p.id, "the existing id is rebound");
        assert!(reg.local(&p.id).is_some(), "and bound back to this pod");
        let other_user = test_user(&pool).await;
        let stolen = reg.open(&pool, &s1, other_user, machine, 5173, Some(&p.id)).await;
        assert!(matches!(stolen, Err(OpenError::Taken)), "another user cannot claim the id");

        let (sid, _, _) = reg.open_stream(&p.id);
        assert_eq!(reg.stream_count(), 1);
        assert_eq!(reg.close_port(&pool, &s1, 5173).await.map(|c| c.id), Some(p.id.clone()));
        assert_eq!(reg.stream_count(), 0, "closing a preview drops its streams");
        assert!(reg.get(&pool, &p.id).await.is_none());
        reg.drop_stream(&sid);

        let other = reg.open(&pool, &s2, user, Uuid::new_v4(), 5173, None).await.unwrap();
        assert_eq!(reg.close_session(&pool, &s1).await.len(), 7);
        assert!(reg.list(&pool, &s1).await.is_empty());
        assert_eq!(reg.get(&pool, &other.id).await.map(|p| p.id), Some(other.id.clone()));

        // A daemon disconnect only detaches: the row survives for the reconnect
        // and the sweeper is what finally drops it.
        reg.detach_machine(&pool, other.machine_id).await;
        assert!(reg.get(&pool, &other.id).await.is_some(), "still resolvable during the grace");
        assert!(reg.local(&other.id).is_none());
        store::delete_session(&pool, &s2).await;

        let off = Registry::new(None, "http://localhost:8700", b"k");
        assert!(matches!(
            off.open(&pool, "s", user, machine, 5173, None).await,
            Err(OpenError::Disabled)
        ));
    }

    #[tokio::test]
    async fn the_sweeper_drops_previews_whose_daemon_never_came_back() {
        let Some(url) = crate::routes::gateway::test_db_url("preview_sweeper") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let reg = registry();
        let user = test_user(&pool).await;
        let session = format!("sweep-{}", Uuid::new_v4());
        let p = reg.open(&pool, &session, user, Uuid::new_v4(), 5173, None).await.unwrap();

        reg.detach_machine(&pool, p.machine_id).await;
        store::sweep(&pool, true).await;
        assert!(reg.get(&pool, &p.id).await.is_some(), "inside the grace period it stays");

        sqlx::query(
            "UPDATE previews SET detached_at = now() - interval '10 minutes' WHERE id = $1",
        )
        .bind(&p.id)
        .execute(&pool)
        .await
        .unwrap();
        store::sweep(&pool, true).await;
        assert!(reg.get(&pool, &p.id).await.is_none(), "past the grace period it is swept");

        // A pod killed without closing its WS leaves detached_at NULL; only the
        // presence-aware sweep can tell that daemon is gone for good.
        let orphan = reg.open(&pool, &session, user, Uuid::new_v4(), 5174, None).await.unwrap();
        sqlx::query("UPDATE previews SET created_at = now() - interval '10 minutes' WHERE id = $1")
            .bind(&orphan.id)
            .execute(&pool)
            .await
            .unwrap();
        store::sweep(&pool, false).await;
        assert!(
            reg.get(&pool, &orphan.id).await.is_some(),
            "without presence tracking an absent row proves nothing"
        );
        store::sweep(&pool, true).await;
        assert!(
            reg.get(&pool, &orphan.id).await.is_none(),
            "with presence tracking the orphan is reaped"
        );
    }

    #[tokio::test]
    async fn a_ticket_nonce_cannot_be_redeemed_on_a_second_pod() {
        let Some(url) = crate::routes::gateway::test_db_url("preview_ticket_replay") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        // Two pods, same signing key and same DB — as in a real deployment.
        let (pod_a, pod_b) = (registry(), registry());
        let user = test_user(&pool).await;
        let session = format!("replay-{}", Uuid::new_v4());
        let p = pod_a.open(&pool, &session, user, Uuid::new_v4(), 5173, None).await.unwrap();

        let ticket = pod_a.tickets().mint_ticket(&p.id, user);
        assert!(
            pod_b.tickets().redeem_ticket(&pool, &ticket, &p.id).await.is_ok(),
            "either pod can redeem a ticket the other minted"
        );
        assert_eq!(
            pod_a.tickets().redeem_ticket(&pool, &ticket, &p.id).await,
            Err(ticket::Reject::Reused),
            "and the nonce is burnt for every pod"
        );
        store::delete_session(&pool, &session).await;
    }

    #[test]
    fn url_scheme_follows_external_url() {
        let reg = Registry::new(
            Some(PreviewHost::parse("pv-{id}.local").unwrap()),
            "http://localhost:8700",
            b"k",
        );
        assert_eq!(reg.url_for("abcdefghijklmnop"), "http://pv-abcdefghijklmnop.local");
    }
}
