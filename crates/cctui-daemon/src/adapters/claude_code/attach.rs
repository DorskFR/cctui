//! Persistent headless `attach` to `claude daemon` workers (CCT-209).
//!
//! Dispatching a fleet session boots a worker PTY, but the interactive Claude
//! worker uses DEC focus-tracking (mode 1004) and stays *parked/unfocused* —
//! it never pulls the seeded prompt through its input loop — until a client
//! sends `op:"attach"`. The `attach` handler is what runs `seedFocus(true)`
//! (writes focus-in `ESC[I` into the worker PTY), `noteActivity()` (resets the
//! 60s idle-retire timer), and registers an attacher (`attachers.size > 0`
//! blocks idle teardown + stall-respawn). Until then the worker sits in
//! "limbo" and our `reply` text lands in an undriven PTY.
//!
//! The real `claude agents` TUI sends `attach` when a user opens a session —
//! which is why a stuck session "unblocks" the moment it's opened on the
//! machine. We reproduce that headlessly: for every live, user-visible session
//! we hold an `attach` connection open (discarding the raw PTY byte stream the
//! server emits after the ack). Holding the socket open is sufficient to keep
//! the attacher registered; no heartbeat/ACK is required. When the connection
//! drops (respawn, kick, settle) we reconnect with backoff until the driver
//! tells us the session is gone.
//!
//! Wire protocol (claude 2.1.x control socket):
//!   request : `{"proto":1,"op":"attach","short":"<8hex>","cols":N,"rows":M,
//!               "attachId":"<unique>","caps":{"terminal":null,"mux":null,"ssh":false}}`
//!   ack     : first newline-delimited JSON line — `{"ok":true,"op":"attach",…}`
//!             on success, or `{"ok":false,"code":"…"}` on failure.
//!   stream  : after a success ack the socket carries RAW PTY bytes (no JSON
//!             envelope); we read and discard them. A server FIN means we were
//!             detached / the session settled.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

use super::discovery::Discovery;

/// PTY geometry reported on attach. The worker only cares that *some* attacher
/// exists; the dispatch path uses the same 120x40, so we match it.
const ATTACH_COLS: u32 = 120;
const ATTACH_ROWS: u32 = 40;

/// Reconnect backoff bounds. Reset to `MIN` on every successful attach.
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Owns one persistent-attach task per live session `short`, reconciled
/// against the `list` snapshot on every poll tick.
pub(super) struct AttachManager {
    discovery: Discovery,
    /// Parent token: every per-session task is cancelled when this fires.
    shutdown: CancellationToken,
    /// `short` → child cancellation token for that session's attach task.
    tasks: HashMap<String, CancellationToken>,
}

impl AttachManager {
    pub(super) fn new(discovery: Discovery, shutdown: CancellationToken) -> Self {
        Self { discovery, shutdown, tasks: HashMap::new() }
    }

    /// Spawn attach tasks for newly-seen shorts and cancel tasks for shorts
    /// that have left the live roster. Idempotent — safe to call every poll.
    pub(super) fn reconcile<'a, I>(&mut self, live: I)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let live: std::collections::HashSet<&str> = live.into_iter().collect();

        // Drop tasks whose session is gone (cancellation closes the socket,
        // which detaches us server-side — focus-out is then emitted only if we
        // were the last attacher).
        self.tasks.retain(|short, cancel| {
            let keep = live.contains(short.as_str());
            if !keep {
                cancel.cancel();
            }
            keep
        });

        // Start tasks for sessions we aren't yet holding open.
        for short in live {
            if self.tasks.contains_key(short) {
                continue;
            }
            let cancel = self.shutdown.child_token();
            let task = AttachTask {
                discovery: self.discovery.clone(),
                short: short.to_owned(),
                cancel: cancel.clone(),
            };
            tokio::spawn(task.run());
            self.tasks.insert(short.to_owned(), cancel);
        }
    }

    /// Cancel every attach task — used on roster flush when the daemon goes
    /// away, so we don't keep dialing a dead socket.
    pub(super) fn cancel_all(&mut self) {
        for (_, cancel) in self.tasks.drain() {
            cancel.cancel();
        }
    }
}

struct AttachTask {
    discovery: Discovery,
    short: String,
    cancel: CancellationToken,
}

impl AttachTask {
    async fn run(self) {
        let mut backoff = BACKOFF_MIN;
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            match self.attach_once().await {
                AttachOutcome::Detached => {
                    // Clean detach (settle / kick / respawn): reconnect quickly
                    // in case the worker came back under the same short.
                    backoff = BACKOFF_MIN;
                }
                AttachOutcome::Gone => {
                    // ENOJOB / EUNVERIFIED: the worker is unrecoverable under
                    // this short. The next poll will drop us from the roster
                    // and cancel this task; back off hard meanwhile so we don't
                    // hammer the socket.
                    backoff = BACKOFF_MAX;
                }
                AttachOutcome::Retry => {
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
        }
    }

    /// One connect → attach → hold-until-EOF cycle. Returns how to proceed.
    async fn attach_once(&self) -> AttachOutcome {
        let Some(sock) = self.discovery.locate_live().await else {
            return AttachOutcome::Retry;
        };
        match self.dial_and_hold(&sock).await {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::debug!(short = %self.short, %err, "attach cycle failed");
                AttachOutcome::Retry
            }
        }
    }

    #[allow(clippy::cognitive_complexity)]
    async fn dial_and_hold(&self, sock: &Path) -> anyhow::Result<AttachOutcome> {
        let stream = UnixStream::connect(sock).await?;
        let (read_half, mut write_half) = stream.into_split();

        let attach_id = uuid::Uuid::new_v4().simple().to_string();
        let req = attach_request(&self.short, &attach_id);
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        // Read the single newline-delimited JSON ack.
        let mut reader = BufReader::new(read_half);
        let mut ack = String::new();
        let n = reader.read_line(&mut ack).await?;
        if n == 0 {
            // EOF before any ack — treat as a transient drop.
            return Ok(AttachOutcome::Retry);
        }
        let ack: Value = serde_json::from_str(ack.trim())?;
        if ack.get("ok") == Some(&Value::Bool(false)) {
            let code = ack.get("code").and_then(Value::as_str).unwrap_or("?");
            tracing::debug!(short = %self.short, %code, "attach rejected");
            return Ok(match code {
                // Worker is dead / unverifiable under this short — stop dialing.
                "ENOJOB" | "EUNVERIFIED" => AttachOutcome::Gone,
                // Respawning / starting — retry with backoff.
                _ => AttachOutcome::Retry,
            });
        }

        tracing::debug!(short = %self.short, "attached (holding open to keep worker awake)");

        // Attached. Drain and discard the raw PTY byte stream until the server
        // closes the connection (detach/settle) or we're cancelled. We never
        // write to the socket — any bytes written would be injected as keys.
        let mut buf = [0_u8; 8192];
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => {
                    // Dropping the socket detaches us server-side.
                    return Ok(AttachOutcome::Detached);
                }
                read = reader.read(&mut buf) => {
                    match read {
                        Ok(0) => return Ok(AttachOutcome::Detached), // server FIN
                        Ok(_) => {} // discard PTY bytes
                        Err(err) => {
                            tracing::debug!(short = %self.short, %err, "attach stream read error");
                            return Ok(AttachOutcome::Retry);
                        }
                    }
                }
            }
        }
    }
}

/// Build the `attach` request the daemon validates (zod
/// `discriminatedUnion("op")`): `proto` must be the integer `1`, `short` an
/// 8-char hex id, `cols`/`rows` ints in `1..=10000`. `caps` is optional but
/// when present `terminal`/`mux`/`ssh` are its required members.
fn attach_request(short: &str, attach_id: &str) -> Value {
    let mut req = json!({
        "proto": 1,
        "op": "attach",
        "short": short,
        "cols": ATTACH_COLS,
        "rows": ATTACH_ROWS,
        "attachId": attach_id,
        "caps": { "terminal": Value::Null, "mux": Value::Null, "ssh": false },
    });
    // Claude Code ≥2.1.168 gates `attach` behind the daemon control key
    // (CCT-264); echo it back when present, or the daemon rejects with EAUTH.
    if let (Some(obj), Some(key)) = (req.as_object_mut(), super::socket::control_key()) {
        obj.insert("auth".to_owned(), Value::String(key));
    }
    req
}

/// What an attach cycle tells the reconnect loop to do next.
enum AttachOutcome {
    /// Clean detach (server FIN / cancel) — reconnect promptly.
    Detached,
    /// Worker unrecoverable under this short — back off hard, await roster drop.
    Gone,
    /// Transient failure — exponential backoff.
    Retry,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format the daemon's zod schema accepts — guards against a
    /// silent drift that would make every attach 400 with `EPROTO`/`EUNKNOWN`.
    #[test]
    fn attach_request_matches_schema() {
        let req = attach_request("a1b2c3d4", "deadbeef");
        assert_eq!(req["proto"], json!(1)); // integer, not "1"
        assert_eq!(req["op"], json!("attach"));
        assert_eq!(req["short"], json!("a1b2c3d4"));
        assert!(req["cols"].as_u64().is_some_and(|c| (1..=10_000).contains(&c)));
        assert!(req["rows"].as_u64().is_some_and(|r| (1..=10_000).contains(&r)));
        assert_eq!(req["attachId"], json!("deadbeef"));
        // caps: terminal/mux/ssh are the required members.
        assert_eq!(req["caps"]["terminal"], Value::Null);
        assert_eq!(req["caps"]["mux"], Value::Null);
        assert_eq!(req["caps"]["ssh"], json!(false));
    }

    /// Reconcile spawns one task per live short and cancels tasks whose session
    /// has left the roster. Discovery points at an empty base, so the spawned
    /// tasks just spin on `locate_live() == None` (harmless) — we only assert
    /// the bookkeeping here.
    #[tokio::test]
    async fn reconcile_tracks_live_shorts() {
        let dir = tempfile::tempdir().unwrap();
        let discovery = Discovery::with_base(dir.path().to_path_buf());
        let shutdown = CancellationToken::new();
        let mut mgr = AttachManager::new(discovery, shutdown.clone());

        mgr.reconcile(["aaaaaaaa", "bbbbbbbb"]);
        assert_eq!(mgr.tasks.len(), 2);
        let token_a = mgr.tasks["aaaaaaaa"].clone();

        // Drop one, keep one, add one.
        mgr.reconcile(["bbbbbbbb", "cccccccc"]);
        assert_eq!(mgr.tasks.len(), 2);
        assert!(token_a.is_cancelled(), "dropped short's task must be cancelled");
        assert!(mgr.tasks.contains_key("bbbbbbbb"));
        assert!(mgr.tasks.contains_key("cccccccc"));

        mgr.cancel_all();
        assert!(mgr.tasks.is_empty());
        shutdown.cancel();
    }
}
