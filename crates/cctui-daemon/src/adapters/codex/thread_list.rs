//! Codex `thread/list` inventory.
//!
//! The legacy [`super::log_tail`] scrapes `~/.codex/sessions/**/*.jsonl`
//! heuristically — it only learns a session's working dir if a line happens
//! to carry one, never sees a human-readable preview/name, and guesses
//! tool-vs-message from ad-hoc field probing.
//!
//! `codex app-server` exposes a first-class, state-DB-backed inventory via the
//! `thread/list` JSON-RPC method: each `Thread` carries `id`/`sessionId`
//! (the rollout `UUIDv7`, identical to the log-tail's `local_id` and the
//! app-server driver's thread id), plus `preview`, `name`, `cwd`, `source`
//! (`cli|vscode|exec|appServer|subAgent…`) and a `status` object. Polling it
//! surfaces **every** codex session on the machine — CLI, VS Code, `codex
//! exec`, app-server — with real metadata, which is exactly the 1:1-with-claude
//! inventory parity the ticket asks for.
//!
//! This module owns a short-lived stdio `codex app-server` per poll: spawn,
//! `initialize` → `thread/list`, read the one response, exit. That keeps the
//! blast radius tiny (no long-lived singleton, no control socket) and sidesteps
//! the experimental daemon/remote-control preconditions that are flagged as
//! risky in — those (managed standalone install + multiplexed driver)
//! are deliberately left to a follow-up. The poll shares the app-server
//! driver's [`SessionRegistry`] so cctui-owned threads (which the driver
//! already streams live) are not re-emitted here, and the log-tail's `owned`
//! set is likewise extended to cover everything this inventory has surfaced.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use cctui_proto::adapter::{AdapterEvent, SessionMeta};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::app_server::{AppServerConfig, SessionRecord, SessionRegistry};

/// Set of thread ids the inventory has surfaced (id → last seen
/// `status.type`). This is the inventory's own dedup state; since it is
/// no longer shared with the log-tail (which keeps tailing the real rollout
/// JSONL so discovered CLI sessions get a populated conversation).
pub type SeenIds = Arc<Mutex<HashMap<String, Option<String>>>>;

/// One inventory entry parsed from a `thread/list` `data[]` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadEntry {
    /// Rollout `UUIDv7` — the cctui `local_id`.
    pub id: String,
    pub preview: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    /// `cli` / `vscode` / `exec` / `appServer` / `subAgent` …
    pub source: Option<String>,
    /// `status.type`: `active` / `idle` / `notLoaded` / `systemError`.
    pub status: Option<String>,
    pub parent_id: Option<String>,
    /// Thread `updatedAt` (unix seconds).
    pub updated_at: Option<i64>,
}

/// Canonical `UUIDv7` form (lowercase, hyphenated) for a codex thread id, so
/// identity matches across list discovery, resume, and the session registry.
/// Non-UUID ids fall back to a trimmed lowercase form.
#[must_use]
pub fn canonical_id(raw: &str) -> String {
    Uuid::parse_str(raw.trim())
        .map_or_else(|_| raw.trim().to_lowercase(), |u| u.hyphenated().to_string())
}

/// Reduce a `SessionSource` (bare string, `{custom}`, or `{subAgent}` object)
/// to a stable source label. Newer codex builds report subagent/custom sources
/// as objects, which the string-only parser used to drop.
#[must_use]
pub fn parse_source(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => map.get("custom").and_then(Value::as_str).map_or_else(
            || {
                if map.contains_key("subAgent") {
                    Some("subAgent".to_owned())
                } else {
                    map.keys().next().cloned()
                }
            },
            |c| Some(c.to_owned()),
        ),
        _ => None,
    }
}

/// Extract the parent thread id of a codex subagent thread, preferring the
/// top-level `parentThreadId` and falling back to the structured
/// `source.subAgent.thread_spawn.parent_thread_id`. Canonicalized so it matches
/// the parent's `local_id`. `None` for plain cli/vscode/exec/appServer threads.
#[must_use]
pub fn parse_parent(v: &Value) -> Option<String> {
    let raw = v.get("parentThreadId").and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(
        || {
            v.pointer("/source/subAgent/thread_spawn/parent_thread_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        },
    )?;
    Some(canonical_id(raw))
}

/// Parse a single `thread/list` `data[]` element. Returns `None` for entries
/// without a usable thread id (which cannot be addressed as a session).
#[must_use]
pub fn parse_thread(v: &Value) -> Option<ThreadEntry> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let id = v
        .get("sessionId")
        .or_else(|| v.get("id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(canonical_id)?;
    Some(ThreadEntry {
        id,
        preview: s("preview").filter(|p| !p.is_empty()),
        name: s("name").filter(|n| !n.is_empty()),
        cwd: s("cwd"),
        source: v.get("source").and_then(parse_source),
        status: v.pointer("/status/type").and_then(Value::as_str).map(str::to_owned),
        parent_id: parse_parent(v),
        updated_at: v.get("updatedAt").and_then(Value::as_i64),
    })
}

/// Parse the `result` of a `thread/list` response into entries.
#[must_use]
pub fn parse_thread_list(result: &Value) -> Vec<ThreadEntry> {
    result
        .get("data")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_thread).collect())
        .unwrap_or_default()
}

fn initialize_req() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"clientInfo": {"name": "cctui", "version": env!("CARGO_PKG_VERSION")}},
    })
}

/// Every codex `ThreadSourceKind`. Omitting `sourceKinds` makes the app-server
/// default to interactive sources (`cli`/`vscode`), so cctui-owned `appServer`
/// threads never come back and the rediscovery branch misses them after a
/// daemon restart. Request all kinds explicitly.
const SOURCE_KINDS: &[&str] = &[
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

/// Upper bound on pages followed in one poll — a guard against a server that
/// keeps handing back a non-null cursor.
const MAX_PAGES: usize = 100;

fn thread_list_req(id: i64, limit: u32, cursor: Option<&str>) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("limit".to_owned(), json!(limit));
    params.insert("sourceKinds".to_owned(), json!(SOURCE_KINDS));
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), json!(cursor));
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "thread/list",
        "params": Value::Object(params),
    })
}

#[must_use]
fn next_cursor(result: &Value) -> Option<String> {
    result.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned)
}

/// String `SubAgentSource`s (`review`/`compact`/`memory_consolidation`) carry
/// no parent id on the wire, so such threads can never nest — skip them.
#[must_use]
pub fn is_orphan_subagent(entry: &ThreadEntry) -> bool {
    entry.parent_id.is_none() && entry.source.as_deref().is_some_and(|s| s.starts_with("subAgent"))
}

/// Build the `SessionStarted` meta for an inventory thread. The preview seeds
/// a first user `Message` separately; here we record cwd + source so the list
/// row renders like a claude-observed session.
fn started_meta(entry: &ThreadEntry) -> SessionMeta {
    let parent_local_id = entry.parent_id.as_ref().map(|parent| {
        crate::dispatch_codex::dispatch_session_for(parent).unwrap_or_else(|| parent.clone())
    });
    let mut extra = json!({
        "source": format!("codex-thread-list:{}", entry.source.as_deref().unwrap_or("unknown")),
        "observed_at": entry.updated_at,
    });
    if entry.source.as_deref().is_some_and(|s| s.starts_with("subAgent")) {
        extra["subagent"] = json!(true);
    }
    SessionMeta { working_dir: entry.cwd.clone(), parent_local_id, extra }
}

#[derive(Debug, Clone)]
pub struct ThreadListConfig {
    pub app: AppServerConfig,
    pub poll_interval: Duration,
    pub page_size: u32,
}

impl ThreadListConfig {
    pub fn from_value(v: &Value) -> Self {
        let mut cfg = Self {
            app: AppServerConfig::from_value(v),
            poll_interval: Duration::from_secs(15),
            page_size: 100,
        };
        if let Some(ms) = v.get("inventory_poll_ms").and_then(Value::as_u64) {
            cfg.poll_interval = Duration::from_millis(ms);
        }
        if let Some(n) = v.get("inventory_page_size").and_then(Value::as_u64) {
            cfg.page_size = u32::try_from(n).unwrap_or(100);
        }
        cfg
    }

    /// `false` disables the inventory poll entirely (`inventory = false` in the
    /// adapter config). Enabled by default.
    pub fn enabled(v: &Value) -> bool {
        v.get("inventory").and_then(Value::as_bool).unwrap_or(true)
    }
}

/// Polls `codex app-server`'s `thread/list` and emits `SessionStarted` (+ a
/// seed `Message` from the preview, + a coarse `Status`) for every machine
/// session, deduping against the live app-server-owned registry and its own
/// already-emitted set.
pub struct ThreadListInventory {
    cfg: ThreadListConfig,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
    /// Threads cctui itself drives via the app-server — skip them so the live
    /// driver stays the single source of their events.
    owned: SessionRegistry,
    /// Threads this poll has already surfaced (id → last seen status), so we
    /// emit `SessionStarted` once and only re-emit `Status` on change. Shared
    /// with the log-tail so it skips these files.
    seen: SeenIds,
}

impl ThreadListInventory {
    pub const fn new(
        cfg: ThreadListConfig,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: CancellationToken,
        owned: SessionRegistry,
        seen: SeenIds,
    ) -> Self {
        Self { cfg, events, shutdown, owned, seen }
    }

    pub async fn run(self) {
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = tick.tick() => {
                    match self.poll_once().await {
                        Ok(entries) => self.reconcile(entries).await,
                        Err(err) => {
                            // Probe failures (codex missing, sandbox/userns,
                            // auth) are expected on some hosts — the log-tail
                            // fallback still runs. Log at debug, keep ticking.
                            tracing::debug!(%err, "codex thread/list inventory poll failed");
                        }
                    }
                }
            }
        }
    }

    /// Spawn a short-lived stdio app-server, run initialize → thread/list, and
    /// return the parsed entries. The process is reaped before returning.
    async fn poll_once(&self) -> anyhow::Result<Vec<ThreadEntry>> {
        poll_threads(&self.cfg.app, self.cfg.page_size).await
    }

    async fn reconcile(&self, entries: Vec<ThreadEntry>) {
        let owned: std::collections::HashSet<String> =
            self.owned.lock().await.keys().cloned().collect();
        for entry in entries {
            // cctui drives this thread live — let the driver own its events.
            if owned.contains(&entry.id) {
                continue;
            }
            if is_orphan_subagent(&entry) {
                continue;
            }
            let prev = self.seen.lock().await.get(&entry.id).cloned();
            match prev {
                None => self.emit_new(&entry).await,
                Some(prev) if prev != entry.status => {
                    self.emit_status(&entry).await;
                }
                Some(_) => continue,
            }
            self.seen.lock().await.insert(entry.id.clone(), entry.status.clone());
        }
    }

    async fn emit_new(&self, entry: &ThreadEntry) {
        let _ = self
            .events
            .send(AdapterEvent::SessionStarted {
                local_id: entry.id.clone(),
                meta: started_meta(entry),
            })
            .await;
        if let Some(name) = entry.name.clone() {
            let _ = self.events.send(status_name(&entry.id, name)).await;
        }
        if let Some(preview) = entry.preview.clone() {
            // Emit the preview as a codex-native `userMessage` so it survives
            // the server's `normalize::for_client("codex","message",…)` (which
            // keys off the codex `type` discriminant and drops payloads without
            // one). A claude-style `{role,text}` payload would render on the
            // list card but vanish in the conversation drawer.
            let _ = self
                .events
                .send(AdapterEvent::Message {
                    local_id: entry.id.clone(),
                    payload: json!({
                        "type": "userMessage",
                        "content": [{"type": "text", "text": preview}],
                    }),
                })
                .await;
        }
        self.emit_status(entry).await;
    }

    async fn emit_status(&self, entry: &ThreadEntry) {
        if let Some(evt) = status_event(&entry.id, entry.status.as_deref()) {
            let _ = self.events.send(evt).await;
        }
    }
}

/// Spawn a short-lived stdio `codex app-server`, run initialize →
/// `thread/list`, and return the parsed entries. The process is reaped before
/// returning. Shared by the periodic inventory poll and the startup
/// rediscovery.
async fn poll_threads(app: &AppServerConfig, limit: u32) -> anyhow::Result<Vec<ThreadEntry>> {
    let mut cmd = Command::new(&app.bin);
    cmd.arg("app-server")
        // No turn is started, so sandbox mode only matters because codex
        // refuses to boot when it cannot create the bwrap namespace on
        // some kernels — pass the configured (host-default) mode through.
        .arg("-c")
        .arg(format!("sandbox_mode=\"{}\"", app.sandbox_mode))
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().context_stdin()?;
    let stdout = child.stdout.take().context_stdout()?;

    let entries = tokio::time::timeout(Duration::from_secs(30), async {
        let mut lines = BufReader::new(stdout).lines();
        write_line(&mut stdin, &initialize_req()).await?;

        let mut entries = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let req_id = 2 + i64::try_from(page).unwrap_or(i64::MAX);
            write_line(&mut stdin, &thread_list_req(req_id, limit, cursor.as_deref())).await?;
            let result = read_response(&mut lines, req_id).await?;
            entries.extend(parse_thread_list(&result));
            match next_cursor(&result) {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        anyhow::Ok(entries)
    })
    .await;

    // Close stdin and reap regardless of how the read went.
    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;

    entries.map_err(|_| anyhow::anyhow!("thread/list timed out"))?
}

/// Read stdout until the JSON-RPC response with `id` arrives, skipping
/// notifications and unrelated responses. Errors on a JSON-RPC `error` object.
async fn read_response<R: AsyncBufRead + Unpin>(
    lines: &mut Lines<R>,
    id: i64,
) -> anyhow::Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            if let Some(err) = v.get("error") {
                anyhow::bail!("thread/list error: {err}");
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
    anyhow::bail!("thread/list response {id} not received before EOF")
}

/// `appServer`-source threads from a `thread/list` snapshot are the ones cctui
/// itself drove before the daemon restarted. Re-seed the durable
/// [`SessionRegistry`] with a [`SessionRecord`] for each so a later command
/// (send/rename/set-model) revives the thread via `thread/resume` instead of
/// reporting `Missing`. Picks `cfg`/cwd from the resolved entry so the
/// resume relaunches in the right directory under the daemon's current config.
#[must_use]
pub fn owned_records(
    entries: &[ThreadEntry],
    app: &AppServerConfig,
) -> Vec<(String, SessionRecord)> {
    entries
        .iter()
        .filter(|e| e.source.as_deref() == Some("appServer"))
        .map(|e| {
            (
                e.id.clone(),
                SessionRecord {
                    cfg: app.clone(),
                    cwd: e.cwd.clone().unwrap_or_default(),
                    name: e.name.clone(),
                    // Gateway env isn't persisted to codex's on-disk
                    // thread state, so a registry seeded from `thread/list`
                    // after a daemon restart starts env-less. The fresh spawn /
                    // fork launch chokepoint in `mod.rs` pulls + stores it; a
                    // resume reuses the stored value, so a rediscovered thread's
                    // first resume can still be env-less for the narrow
                    // restart-then-resume window. See.
                    env: std::collections::BTreeMap::new(),
                },
            )
        })
        .collect()
}

/// On adapter startup, rediscover cctui-owned (`appServer`-source) codex threads
/// from `thread/list` and seed the durable registry so in-flight sessions stay
/// drivable across a daemon restart / self-update. Best-effort: a
/// probe failure (codex missing, sandbox/userns, auth) just leaves the registry
/// empty, exactly as before this change.
pub async fn rediscover_owned(cfg: &ThreadListConfig, registry: &SessionRegistry) {
    let entries = match poll_threads(&cfg.app, cfg.page_size).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::debug!(%err, "codex: startup thread rediscovery probe failed");
            return;
        }
    };
    let records = owned_records(&entries, &cfg.app);
    if records.is_empty() {
        return;
    }
    let mut count = 0_usize;
    {
        let mut guard = registry.lock().await;
        for (id, record) in records {
            // Don't clobber a record a live session already inserted in the race
            // between rediscovery and a freshly spawned session.
            guard.entry(id).or_insert_with(|| {
                count += 1;
                record
            });
        }
    }
    if count > 0 {
        tracing::info!(count, "codex: rediscovered owned threads, seeded for resume");
    }
}

/// Map a `thread/list` `status.type` to a coarse session `Status`. This is the
/// snapshot equivalent of the driver's `thread/status/changed` mapping; a
/// `notLoaded` thread (the common case for an externally-started, idle session)
/// reports `idle` so the row settles rather than spinning.
fn status_event(local_id: &str, status: Option<&str>) -> Option<AdapterEvent> {
    let (tempo, state) = match status {
        Some("active") => (Some("active"), Some("working")),
        Some("idle" | "notLoaded") => (None, Some("idle")),
        Some("systemError") => (None, Some("failed")),
        _ => return None,
    };
    Some(AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: tempo.map(str::to_owned),
        state: state.map(str::to_owned),
        detail: None,
        activity: None,
        name: None,
        intent: None,
        model: None,
        effort: None,
        children: vec![],
    })
}

fn status_name(local_id: &str, name: String) -> AdapterEvent {
    AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: None,
        state: None,
        detail: None,
        activity: None,
        name: Some(name),
        intent: None,
        model: None,
        effort: None,
        children: vec![],
    }
}

async fn write_line<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(v)?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

/// Tiny helpers so `take()`-of-`None` reads as a clear error rather than an
/// `unwrap`.
trait OptionStdioExt<T> {
    fn context_stdin(self) -> anyhow::Result<T>;
    fn context_stdout(self) -> anyhow::Result<T>;
}
impl<T> OptionStdioExt<T> for Option<T> {
    fn context_stdin(self) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("codex app-server stdin missing"))
    }
    fn context_stdout(self) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("codex app-server stdout missing"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({"data": [
            {
                "id": "019ea66a-cf6e-73b1",
                "sessionId": "019ea66a-cf6e-73b1",
                "preview": "Implement EX-3 end to end please.",
                "name": "EX-3",
                "cwd": "/home/u/proj",
                "source": "vscode",
                "status": {"type": "notLoaded"},
            },
            {
                "id": "no-session-id-but-id",
                "preview": "",
                "cwd": "/tmp",
                "source": "cli",
                "status": {"type": "active"},
            },
            { "preview": "unaddressable", "status": {"type": "idle"} },
        ], "nextCursor": null})
    }

    #[test]
    fn parses_threads_and_skips_idless() {
        let entries = parse_thread_list(&sample());
        assert_eq!(entries.len(), 2);
        let first = &entries[0];
        assert_eq!(first.id, "019ea66a-cf6e-73b1");
        assert_eq!(first.preview.as_deref(), Some("Implement EX-3 end to end please."));
        assert_eq!(first.name.as_deref(), Some("EX-3"));
        assert_eq!(first.cwd.as_deref(), Some("/home/u/proj"));
        assert_eq!(first.source.as_deref(), Some("vscode"));
        assert_eq!(first.status.as_deref(), Some("notLoaded"));
        // Empty preview is dropped; id falls back from `id` when no sessionId.
        assert_eq!(entries[1].id, "no-session-id-but-id");
        assert_eq!(entries[1].preview, None);
    }

    #[test]
    fn missing_data_is_empty() {
        assert!(parse_thread_list(&json!({})).is_empty());
        assert!(parse_thread_list(&Value::Null).is_empty());
    }

    #[test]
    fn request_builders_shape() {
        assert_eq!(initialize_req()["method"], "initialize");
        let lr = thread_list_req(2, 42, None);
        assert_eq!(lr["method"], "thread/list");
        assert_eq!(lr["id"], 2);
        // paginate with `limit`, never the removed `pageSize`.
        assert_eq!(lr["params"]["limit"], 42);
        assert!(lr["params"].get("pageSize").is_none());
        assert!(lr["params"].get("cursor").is_none());
    }

    #[test]
    fn thread_list_req_asks_for_all_source_kinds() {
        // Omitting `sourceKinds` defaults to cli/vscode, hiding cctui's own
        // appServer threads. Every ThreadSourceKind must be requested.
        let lr = thread_list_req(2, 100, None);
        let kinds: Vec<&str> = lr["params"]["sourceKinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for expected in [
            "cli",
            "vscode",
            "exec",
            "appServer",
            "subAgent",
            "subAgentReview",
            "subAgentCompact",
            "subAgentThreadSpawn",
            "subAgentOther",
            "unknown",
        ] {
            assert!(kinds.contains(&expected), "missing source kind {expected}");
        }
    }

    #[test]
    fn thread_list_req_carries_cursor_when_paginating() {
        let lr = thread_list_req(3, 100, Some("CUR-2"));
        assert_eq!(lr["id"], 3);
        assert_eq!(lr["params"]["cursor"], "CUR-2");
    }

    #[test]
    fn next_cursor_follows_until_null() {
        assert_eq!(next_cursor(&json!({"nextCursor": "abc"})).as_deref(), Some("abc"));
        assert_eq!(next_cursor(&json!({"nextCursor": ""})), None);
        assert_eq!(next_cursor(&json!({"nextCursor": Value::Null})), None);
        assert_eq!(next_cursor(&json!({})), None);
    }

    #[tokio::test]
    async fn read_response_paginates_to_exhaustion() {
        // Two `thread/list` pages (ids 2, 3) with a notification and the
        // initialize reply interleaved; the reader must skip non-matching lines
        // and follow the cursor until `nextCursor` is null.
        let stream = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"thread/status/changed","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"result":{"data":[{"id":"019ea66a-cf6e-73b1-8000-000000000001","sessionId":"019ea66a-cf6e-73b1-8000-000000000001"}],"nextCursor":"p2"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":3,"result":{"data":[{"id":"019ea66a-cf6e-73b1-8000-000000000002","sessionId":"019ea66a-cf6e-73b1-8000-000000000002"}],"nextCursor":null}}"#,
            "\n",
        );
        let mut lines = BufReader::new(stream.as_bytes()).lines();

        let mut entries = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let req_id = 2 + i64::try_from(page).unwrap();
            let result = read_response(&mut lines, req_id).await.unwrap();
            entries.extend(parse_thread_list(&result));
            match next_cursor(&result) {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(cursor.as_deref(), Some("p2"));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "019ea66a-cf6e-73b1-8000-000000000001");
        assert_eq!(entries[1].id, "019ea66a-cf6e-73b1-8000-000000000002");
    }

    #[tokio::test]
    async fn read_response_surfaces_jsonrpc_error() {
        let stream =
            concat!(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"boom"}}"#, "\n",);
        let mut lines = BufReader::new(stream.as_bytes()).lines();
        assert!(read_response(&mut lines, 2).await.is_err());
    }

    #[test]
    fn canonical_id_normalizes_uuids() {
        assert_eq!(
            canonical_id("019EA66A-CF6E-73B1-8000-0000000000AB"),
            "019ea66a-cf6e-73b1-8000-0000000000ab"
        );
        // Non-UUID ids stay addressable, just lowercased/trimmed.
        assert_eq!(canonical_id("  Weird-ID  "), "weird-id");
    }

    #[test]
    fn parse_source_handles_string_and_object_variants() {
        assert_eq!(parse_source(&json!("appServer")).as_deref(), Some("appServer"));
        assert_eq!(parse_source(&json!({"custom": "cctui"})).as_deref(), Some("cctui"));
        assert_eq!(
            parse_source(&json!({"subAgent": {"thread_spawn": {"depth": 1}}})).as_deref(),
            Some("subAgent")
        );
        assert_eq!(parse_source(&json!({"subAgent": "review"})).as_deref(), Some("subAgent"));
        assert_eq!(parse_source(&Value::Null), None);
    }

    #[test]
    fn parse_thread_canonicalizes_and_reads_structured_source() {
        let entry = parse_thread(&json!({
            "id": "019EA66A-CF6E-73B1-8000-0000000000AB",
            "sessionId": "019EA66A-CF6E-73B1-8000-0000000000AB",
            "source": {"subAgent": {"thread_spawn": {"depth": 2}}},
            "status": {"type": "active"},
        }))
        .unwrap();
        assert_eq!(entry.id, "019ea66a-cf6e-73b1-8000-0000000000ab");
        assert_eq!(entry.source.as_deref(), Some("subAgent"));
    }

    #[test]
    fn parse_parent_prefers_top_level_field() {
        let entry = parse_thread(&json!({
            "id": "child",
            "sessionId": "child",
            "source": {"subAgent": "review"},
            "parentThreadId": "019EA66A-CF6E-73B1-8000-0000000000AB",
        }))
        .unwrap();
        assert_eq!(entry.parent_id.as_deref(), Some("019ea66a-cf6e-73b1-8000-0000000000ab"));
    }

    #[test]
    fn parse_parent_falls_back_to_structured_source() {
        let entry = parse_thread(&json!({
            "id": "child",
            "sessionId": "child",
            "source": {"subAgent": {"thread_spawn": {
                "depth": 1,
                "parent_thread_id": "019EA66A-CF6E-73B1-8000-000000000ABC",
            }}},
        }))
        .unwrap();
        assert_eq!(entry.parent_id.as_deref(), Some("019ea66a-cf6e-73b1-8000-000000000abc"));
    }

    #[test]
    fn parse_parent_none_for_plain_threads() {
        for src in [json!("cli"), json!("appServer"), json!({"custom": "cctui"})] {
            let entry = parse_thread(&json!({"id": "t", "sessionId": "t", "source": src})).unwrap();
            assert_eq!(entry.parent_id, None);
        }
    }

    #[test]
    fn status_mapping() {
        let active = status_event("t", Some("active")).unwrap();
        let AdapterEvent::Status { tempo, state, .. } = active else { panic!() };
        assert_eq!(tempo.as_deref(), Some("active"));
        assert_eq!(state.as_deref(), Some("working"));

        let not_loaded = status_event("t", Some("notLoaded")).unwrap();
        let AdapterEvent::Status { tempo, state, .. } = not_loaded else { panic!() };
        assert_eq!(tempo, None);
        assert_eq!(state.as_deref(), Some("idle"));

        assert!(status_event("t", Some("weird")).is_none());
        assert!(status_event("t", None).is_none());
    }

    #[test]
    fn started_meta_carries_cwd_and_source() {
        let entry = ThreadEntry {
            id: "x".into(),
            parent_id: None,
            updated_at: None,
            preview: None,
            name: None,
            cwd: Some("/repo".into()),
            source: Some("exec".into()),
            status: None,
        };
        let meta = started_meta(&entry);
        assert_eq!(meta.working_dir.as_deref(), Some("/repo"));
        assert_eq!(meta.extra["source"], "codex-thread-list:exec");
        assert_eq!(meta.parent_local_id, None);
        assert!(meta.extra["observed_at"].is_null());
    }

    #[test]
    fn started_meta_carries_observed_at() {
        let entry = ThreadEntry {
            id: "x".into(),
            parent_id: None,
            updated_at: Some(1_762_000_000),
            preview: None,
            name: None,
            cwd: None,
            source: Some("cli".into()),
            status: None,
        };
        assert_eq!(started_meta(&entry).extra["observed_at"], 1_762_000_000_i64);
    }

    #[test]
    fn orphan_subagents_are_skipped_linked_ones_are_not() {
        let mut entry = ThreadEntry {
            id: "x".into(),
            parent_id: None,
            updated_at: None,
            preview: None,
            name: None,
            cwd: None,
            source: Some("subAgent".into()),
            status: None,
        };
        assert!(is_orphan_subagent(&entry));
        entry.parent_id = Some("p".into());
        assert!(!is_orphan_subagent(&entry));
        entry.parent_id = None;
        entry.source = Some("cli".into());
        assert!(!is_orphan_subagent(&entry));
        entry.source = None;
        assert!(!is_orphan_subagent(&entry));
    }

    #[test]
    fn started_meta_remaps_dispatched_parent_and_flags_subagent() {
        let exec = "019f832c-6301-7053-8000-0000000000d1";
        crate::dispatch_codex::register_dispatch_thread(exec, "DISPATCH-SESS-1");
        let entry = ThreadEntry {
            id: "child".into(),
            parent_id: Some(exec.into()),
            updated_at: None,
            preview: None,
            name: None,
            cwd: Some("/repo".into()),
            source: Some("subAgent".into()),
            status: None,
        };
        let meta = started_meta(&entry);
        assert_eq!(meta.parent_local_id.as_deref(), Some("DISPATCH-SESS-1"));
        assert_eq!(meta.extra["subagent"], json!(true));
    }

    #[test]
    fn started_meta_propagates_parent_link() {
        let entry = ThreadEntry {
            id: "child".into(),
            parent_id: Some("parent".into()),
            updated_at: None,
            preview: None,
            name: None,
            cwd: Some("/repo".into()),
            source: Some("subAgent".into()),
            status: None,
        };
        let meta = started_meta(&entry);
        assert_eq!(meta.parent_local_id.as_deref(), Some("parent"));
    }

    #[test]
    fn config_from_value_defaults_and_overrides() {
        let cfg = ThreadListConfig::from_value(&json!({}));
        assert_eq!(cfg.poll_interval, Duration::from_secs(15));
        assert_eq!(cfg.page_size, 100);
        assert!(ThreadListConfig::enabled(&json!({})));

        let cfg = ThreadListConfig::from_value(&json!({
            "inventory_poll_ms": 5000, "inventory_page_size": 25,
        }));
        assert_eq!(cfg.poll_interval, Duration::from_millis(5000));
        assert_eq!(cfg.page_size, 25);
        assert!(!ThreadListConfig::enabled(&json!({"inventory": false})));
    }

    #[tokio::test]
    async fn reconcile_emits_started_once_then_status_on_change() {
        let (tx, mut rx) = mpsc::channel(64);
        let owned = SessionRegistry::default();
        let inv = ThreadListInventory::new(
            ThreadListConfig::from_value(&json!({})),
            tx,
            CancellationToken::new(),
            owned,
            SeenIds::default(),
        );
        let entry = ThreadEntry {
            id: "t1".into(),
            parent_id: None,
            updated_at: None,
            preview: Some("hello".into()),
            name: Some("nm".into()),
            cwd: Some("/w".into()),
            source: Some("cli".into()),
            status: Some("notLoaded".into()),
        };
        inv.reconcile(vec![entry.clone()]).await;
        // SessionStarted, Status(name), Message(preview), Status(idle).
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { name: Some(_), .. }));
        // the preview is emitted as a codex-native `userMessage` so it
        // survives `normalize::for_client("codex","message",…)` server-side.
        let AdapterEvent::Message { payload, .. } = rx.recv().await.unwrap() else {
            panic!("expected preview Message")
        };
        assert_eq!(payload["type"], "userMessage");
        assert_eq!(payload["content"][0]["type"], "text");
        assert_eq!(payload["content"][0]["text"], "hello");
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));

        // Same status again → nothing new.
        inv.reconcile(vec![entry.clone()]).await;
        assert!(rx.try_recv().is_err());

        // Status change → one Status event, no re-Started.
        let mut active = entry;
        active.status = Some("active".into());
        inv.reconcile(vec![active]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::Status { state: Some(s), .. } if s == "working"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn reconcile_emits_started_with_parent_link_for_subagent() {
        let (tx, mut rx) = mpsc::channel(64);
        let inv = ThreadListInventory::new(
            ThreadListConfig::from_value(&json!({})),
            tx,
            CancellationToken::new(),
            SessionRegistry::default(),
            SeenIds::default(),
        );
        let entry = ThreadEntry {
            id: "child".into(),
            parent_id: Some("parent".into()),
            updated_at: None,
            preview: None,
            name: None,
            cwd: Some("/w".into()),
            source: Some("subAgent".into()),
            status: Some("active".into()),
        };
        inv.reconcile(vec![entry]).await;
        let AdapterEvent::SessionStarted { local_id, meta } = rx.recv().await.unwrap() else {
            panic!("expected SessionStarted")
        };
        assert_eq!(local_id, "child");
        assert_eq!(meta.parent_local_id.as_deref(), Some("parent"));
    }

    #[test]
    fn owned_records_seeds_only_app_server_threads() {
        // startup rediscovery re-seeds the durable registry from the
        // `thread/list` snapshot, but only for cctui-driven (`appServer`-source)
        // threads — CLI/vscode/exec sessions are not ours to resume.
        let entries = vec![
            ThreadEntry {
                id: "mine".into(),
                parent_id: None,
                updated_at: None,
                preview: None,
                name: Some("nm".into()),
                cwd: Some("/repo".into()),
                source: Some("appServer".into()),
                status: Some("idle".into()),
            },
            ThreadEntry {
                id: "cli-one".into(),
                parent_id: None,
                updated_at: None,
                preview: None,
                name: None,
                cwd: Some("/elsewhere".into()),
                source: Some("cli".into()),
                status: Some("active".into()),
            },
        ];
        let recs = owned_records(&entries, &AppServerConfig::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].0, "mine");
        assert_eq!(recs[0].1.cwd, "/repo");
        assert_eq!(recs[0].1.name.as_deref(), Some("nm"));
    }

    #[tokio::test]
    async fn rediscover_seeds_registry_without_clobbering_live() {
        use super::super::app_server::SessionRecord;
        let registry = SessionRegistry::default();
        // A live session already registered this id with a name; rediscovery
        // must not overwrite it.
        registry.lock().await.insert(
            "live".into(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/live".into(),
                name: Some("keep-me".into()),
                env: std::collections::BTreeMap::new(),
            },
        );
        let entries = vec![
            ThreadEntry {
                id: "live".into(),
                parent_id: None,
                updated_at: None,
                preview: None,
                name: Some("from-inventory".into()),
                cwd: Some("/other".into()),
                source: Some("appServer".into()),
                status: None,
            },
            ThreadEntry {
                id: "rediscovered".into(),
                parent_id: None,
                updated_at: None,
                preview: None,
                name: None,
                cwd: Some("/repo".into()),
                source: Some("appServer".into()),
                status: None,
            },
        ];
        // Drive the seeding directly (poll_threads needs a real codex binary).
        let mut guard = registry.lock().await;
        for (id, record) in owned_records(&entries, &AppServerConfig::default()) {
            guard.entry(id).or_insert(record);
        }
        drop(guard);
        let (live_name, live_cwd, rediscovered_cwd) = {
            let guard = registry.lock().await;
            (
                guard.get("live").and_then(|r| r.name.clone()),
                guard.get("live").map(|r| r.cwd.clone()),
                guard.get("rediscovered").map(|r| r.cwd.clone()),
            )
        };
        assert_eq!(live_name.as_deref(), Some("keep-me"));
        assert_eq!(live_cwd.as_deref(), Some("/live"));
        assert_eq!(rediscovered_cwd.as_deref(), Some("/repo"));
    }

    #[tokio::test]
    async fn reconcile_skips_app_server_owned_threads() {
        use super::super::app_server::SessionRecord;
        let (tx, mut rx) = mpsc::channel(8);
        let owned = SessionRegistry::default();
        owned.lock().await.insert(
            "owned1".into(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/w".into(),
                name: None,
                env: std::collections::BTreeMap::new(),
            },
        );
        let inv = ThreadListInventory::new(
            ThreadListConfig::from_value(&json!({})),
            tx,
            CancellationToken::new(),
            owned,
            SeenIds::default(),
        );
        inv.reconcile(vec![ThreadEntry {
            id: "owned1".into(),
            parent_id: None,
            updated_at: None,
            preview: Some("x".into()),
            name: None,
            cwd: None,
            source: Some("appServer".into()),
            status: Some("active".into()),
        }])
        .await;
        assert!(rx.try_recv().is_err(), "owned thread must not be re-emitted");
    }
}
