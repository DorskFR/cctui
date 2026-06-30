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
use std::sync::atomic::{AtomicU64, Ordering};
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

/// How often a held attach actively probes its worker's liveness (CCT-487).
///
/// Holding the socket open is normally sufficient to keep the worker focused
/// and the idle-retire timer reset, but that assumption is unverified: if the
/// worker idle-retires anyway, or the held socket half-opens (peer stops
/// sending and never FINs), the drain loop blocks reading PTY bytes forever
/// over a worker that already slept and cctui never notices (liveness was
/// otherwise purely time-derived server-side). So while we hold, we periodically
/// ask the daemon `has` whether the short we *believe* held is still alive.
const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(20);

/// Maximum quiet period on a held attach's read side before we treat the socket
/// as half-open (CCT-487). A healthy idle worker still gets a liveness probe at
/// `LIVENESS_PROBE_INTERVAL`, so a read that stalls well past it (without the
/// probe having reconnected us) means the peer wedged — map that to a `Retry`
/// reconnect rather than blocking on `read` indefinitely.
const READ_STALL_TIMEOUT: Duration = Duration::from_secs(90);

/// Count of held attaches found dead by the periodic liveness probe (CCT-487).
/// The daemon has no metrics exporter, so this process-local counter is surfaced
/// in the `warn!` it accompanies, making the prod frequency of
/// held-but-dead workers visible in journald without new infra.
static HELD_ATTACH_FOUND_DEAD: AtomicU64 = AtomicU64::new(0);

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
                AttachOutcome::HeldDead => {
                    // Keep-alive FAILED (CCT-487): the liveness probe found the
                    // short we believe held no longer alive (idle-retired despite
                    // the held attacher), or the held socket stalled half-open.
                    // Reconnect promptly — a fresh `attach` re-seeds focus and
                    // re-registers the attacher if the worker is reachable; if it
                    // is truly gone the reject will downgrade us to Gone and the
                    // next poll drops us from the roster (where `resume_worker`
                    // can revive a still-revivable session).
                    backoff = BACKOFF_MIN;
                }
                AttachOutcome::Gone => {
                    // ENOJOB / EUNVERIFIED: the worker is unrecoverable under
                    // this short. The next poll will drop us from the roster
                    // and cancel this task; back off hard meanwhile so we don't
                    // hammer the socket.
                    backoff = BACKOFF_MAX;
                }
                AttachOutcome::Unauthorized => {
                    // EAUTH: the control key is missing/rotated/wrong, so every
                    // attach will fail the same way until it's fixed. Back off
                    // hard like `Gone` so we don't hammer the socket, but unlike
                    // `Gone` the roster won't drop us — keep retrying so a
                    // regenerated key (re-read fresh each cycle) is picked up.
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
            let outcome = classify_reject(code);
            // EAUTH means the control key is missing/rotated/wrong, so
            // keep-alive is globally broken — not a transient blip. Warn loud.
            if outcome == AttachOutcome::Unauthorized {
                tracing::warn!(
                    short = %self.short,
                    %code,
                    "attach unauthorized — daemon control key missing/rotated/wrong; \
                     keep-alive is broken until the key is fixed"
                );
            } else {
                tracing::debug!(short = %self.short, %code, "attach rejected");
            }
            return Ok(outcome);
        }

        tracing::debug!(short = %self.short, "attached (holding open to keep worker awake)");

        // Attached. Drain and discard the raw PTY byte stream until the server
        // closes the connection (detach/settle) or we're cancelled. We never
        // write to the socket — any bytes written would be injected as keys.
        //
        // CCT-487: holding open is no longer trusted blind. A periodic `has`
        // probe verifies the worker we believe held is actually alive, and a
        // read-stall timeout catches a half-open socket whose peer wedged
        // without a FIN — either maps to a `HeldDead` reconnect instead of
        // blocking on `read` forever over a worker that already slept.
        let mut buf = [0_u8; 8192];
        let mut probe = tokio::time::interval(LIVENESS_PROBE_INTERVAL);
        // First tick fires immediately; skip it so we don't probe a worker we
        // just confirmed reachable by attaching.
        probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        probe.tick().await;
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => {
                    // Dropping the socket detaches us server-side.
                    return Ok(AttachOutcome::Detached);
                }
                _ = probe.tick() => {
                    if let Some(outcome) = self.probe_liveness(sock).await {
                        return Ok(outcome);
                    }
                }
                read = tokio::time::timeout(READ_STALL_TIMEOUT, reader.read(&mut buf)) => {
                    match read {
                        Ok(Ok(0)) => return Ok(AttachOutcome::Detached), // server FIN
                        Ok(Ok(_)) => {} // discard PTY bytes
                        Ok(Err(err)) => {
                            tracing::debug!(short = %self.short, %err, "attach stream read error");
                            return Ok(AttachOutcome::Retry);
                        }
                        Err(_elapsed) => {
                            // Half-open: no bytes AND no FIN for far longer than
                            // the probe interval. Verify before bailing so a
                            // genuinely-idle-but-alive worker isn't churned.
                            if let Some(outcome) = self.probe_liveness(sock).await {
                                return Ok(outcome);
                            }
                            tracing::warn!(
                                short = %self.short,
                                timeout_s = READ_STALL_TIMEOUT.as_secs(),
                                "held attach read stalled past timeout but worker still alive — \
                                 reconnecting to clear a possibly half-open socket"
                            );
                            return Ok(AttachOutcome::HeldDead);
                        }
                    }
                }
            }
        }
    }

    /// Liveness probe for a held attach (CCT-487): ask the daemon `has` whether
    /// the short we believe held is still alive. Returns `Some(HeldDead)` when the
    /// worker reports not-alive — keep-alive has failed and the caller should
    /// reconnect/re-dispatch — or `None` when it is alive (or the probe couldn't
    /// be run, e.g. the socket is momentarily gone) so the hold continues.
    async fn probe_liveness(&self, sock: &Path) -> Option<AttachOutcome> {
        let req = json!({"proto": 1, "op": "has", "short": self.short});
        match super::socket::one_shot(sock, &req).await {
            Ok(resp) => {
                if has_reports_alive(&resp) {
                    return None;
                }
                let count = HELD_ATTACH_FOUND_DEAD.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!(
                    short = %self.short,
                    held_attach_found_dead_total = count,
                    "held attach found dead — worker idle-retired despite the keep-alive \
                     attacher; reconnecting to re-seed focus / let the poll revive it"
                );
                Some(AttachOutcome::HeldDead)
            }
            // Probe couldn't run (socket gone / transient). Don't tear down a
            // possibly-healthy hold on a probe blip — the read side or the next
            // probe tick will catch a real death.
            Err(err) => {
                tracing::debug!(short = %self.short, %err, "liveness probe failed; holding");
                None
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

/// Interpret a daemon `has` response (CCT-487): the worker is alive only when
/// the daemon explicitly says so. A missing/non-bool `alive` field, or a
/// `{ok:false}` ack, is treated as NOT alive — the conservative reading for a
/// keep-alive liveness check, so an ambiguous answer triggers a reconnect rather
/// than masking a dead worker.
fn has_reports_alive(resp: &Value) -> bool {
    if resp.get("ok") == Some(&Value::Bool(false)) {
        return false;
    }
    resp.get("alive").and_then(Value::as_bool).unwrap_or(false)
}

/// Map an attach-reject `code` (the daemon's `{ok:false, code}` ack) to the
/// reconnect outcome. EAUTH is auth-class (Claude Code ≥2.1.168 gates `attach`
/// behind the control key, CCT-264): keep-alive is globally broken until the key
/// is fixed, so it must NOT fall into the silent transient-retry bucket (CCT-486).
fn classify_reject(code: &str) -> AttachOutcome {
    match code {
        // Worker is dead / unverifiable under this short — stop dialing.
        "ENOJOB" | "EUNVERIFIED" => AttachOutcome::Gone,
        // Control key missing/rotated/wrong — back off hard but keep retrying so
        // a regenerated key (re-read fresh each cycle) is picked up.
        "EAUTH" => AttachOutcome::Unauthorized,
        // Respawning / starting — retry with backoff.
        _ => AttachOutcome::Retry,
    }
}

/// What an attach cycle tells the reconnect loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachOutcome {
    /// Clean detach (server FIN / cancel) — reconnect promptly.
    Detached,
    /// Keep-alive verification FAILED (CCT-487): the periodic liveness probe
    /// found the held short not-alive (idle-retired despite the attacher), or the
    /// held socket stalled half-open. Reconnect promptly to re-seed focus / let
    /// the poll revive the session, rather than blocking on a dead hold.
    HeldDead,
    /// Worker unrecoverable under this short — back off hard, await roster drop.
    Gone,
    /// Daemon control key missing/rotated/wrong (EAUTH) — keep-alive is globally
    /// broken. Back off hard like `Gone`, but keep retrying so a regenerated key
    /// is picked up on the next cycle.
    Unauthorized,
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

    /// EAUTH (control key missing/rotated/wrong) must map to `Unauthorized`, not
    /// the silent `Retry` bucket — otherwise keep-alive loops forever at debug
    /// level and every session crosses the 60s idle-retire (CCT-486).
    #[test]
    fn eauth_maps_to_unauthorized_not_retry() {
        assert_eq!(classify_reject("EAUTH"), AttachOutcome::Unauthorized);
        assert_ne!(classify_reject("EAUTH"), AttachOutcome::Retry);
        // The dead-worker codes still stop dialing.
        assert_eq!(classify_reject("ENOJOB"), AttachOutcome::Gone);
        assert_eq!(classify_reject("EUNVERIFIED"), AttachOutcome::Gone);
        // Unknown / transient codes still retry.
        assert_eq!(classify_reject("ERESPAWN"), AttachOutcome::Retry);
        assert_eq!(classify_reject("?"), AttachOutcome::Retry);
    }

    /// The liveness probe (CCT-487) treats only an explicit `alive:true` as
    /// alive; anything ambiguous (missing field, ok:false, non-bool) is dead, so
    /// a held-but-dead worker is never masked into a quiet hold.
    #[test]
    fn has_reports_alive_is_conservative() {
        assert!(has_reports_alive(&json!({"ok": true, "alive": true})));
        assert!(!has_reports_alive(&json!({"ok": true, "alive": false})));
        // Idle-retired / unknown short: no `alive` field → not alive.
        assert!(!has_reports_alive(&json!({"ok": true})));
        // Rejected probe → not alive.
        assert!(!has_reports_alive(&json!({"ok": false, "code": "ENOJOB"})));
        // Defensive: a non-bool `alive` must not read as alive.
        assert!(!has_reports_alive(&json!({"alive": "yes"})));
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
