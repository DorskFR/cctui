//! Newline-delimited JSON client for the `claude daemon` control socket.
//!
//! Wire format: one UTF-8 JSON object per line. Most ops are one-request /
//! one-response on a fresh connection; `subscribe` and `attach` keep the
//! connection open and stream multiple response lines.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// Control-socket ops that the claude daemon gates behind the control key.
/// Read ops (`ping`/`list`/`has`/`kill`) are ungated; only the
/// mutating `dispatch`/`reply`/`attach` ops are rejected with `EAUTH` when no
/// `auth` is presented. The daemon's request schema is a strict discriminated
/// union, so `auth` must ONLY ride on these ops — adding it to `ping`/`list`
/// would be rejected as a malformed request.
const AUTH_GATED_OPS: &[&str] = &["dispatch", "reply", "attach"];

/// Path to the claude daemon's control key, mirroring the CLI's own resolution
/// (`<config>/daemon/control.key`, where `<config>` is `$CLAUDE_CONFIG_DIR` or
/// `~/.claude`). Claude Code ≥2.1.168 generates this file (16 random bytes, hex,
/// mode 0600) and requires every gated op to echo it back.
fn control_key_path() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    Some(base.join("daemon").join("control.key"))
}

/// Read the daemon control key, if present. Read fresh each call (the file is
/// tiny) so a key rotation across a daemon restart is picked up without needing
/// to restart cctui-daemon. Absence is not an error here: older claude builds
/// have no key file and don't gate, so callers send no `auth` and succeed.
pub fn control_key() -> Option<String> {
    let path = control_key_path()?;
    let key = std::fs::read_to_string(path).ok()?;
    let key = key.trim();
    if key.is_empty() { None } else { Some(key.to_owned()) }
}

/// Inject the control key as a top-level `auth` field when `request` is an
/// auth-gated op and doesn't already carry one. No-op otherwise.
fn inject_auth(request: &mut Value) {
    let is_gated =
        request.get("op").and_then(Value::as_str).is_some_and(|op| AUTH_GATED_OPS.contains(&op));
    if !is_gated || request.get("auth").is_some() {
        return;
    }
    if let (Some(obj), Some(key)) = (request.as_object_mut(), control_key()) {
        obj.insert("auth".to_owned(), Value::String(key));
    }
}

/// Send a single request and read a single JSON-line response.
pub async fn one_shot(socket: &Path, request: &Value) -> Result<Value> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    // Echo the daemon control key on auth-gated ops (dispatch/reply); a clone
    // keeps the borrow read-only for callers that reuse the request.
    let mut request = request.clone();
    inject_auth(&mut request);
    let mut line = serde_json::to_string(&request)?;
    line.push('\n');
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;
    // Deliberately keep the write half open until the response is read. The
    // `dispatch` op drops the request as stale the moment it observes EOF on
    // the read side (`tengu_bg_dispatch_stale_drop`), so half-closing here —
    // as an earlier `drop(write_half)` did — silently killed every spawn.
    // Other ops don't care, so holding it open is universally safe.

    let mut reader = BufReader::new(read_half);
    let mut buf = String::new();
    let n = reader.read_line(&mut buf).await?;
    if n == 0 {
        bail!("daemon closed connection without response");
    }
    let resp: Value = serde_json::from_str(buf.trim())
        .with_context(|| format!("decoding response: {}", buf.trim()))?;
    Ok(resp)
}

/// Same as [`one_shot`] but deserialises directly into `T` and checks the
/// `ok: true` invariant. Returns an error if the daemon set `ok:false`.
pub async fn call<T: DeserializeOwned>(socket: &Path, request: &Value) -> Result<T> {
    let resp = one_shot(socket, request).await?;
    if resp.get("ok") == Some(&Value::Bool(false)) {
        let code = resp.get("code").and_then(Value::as_str).unwrap_or("?");
        let err = resp.get("error").and_then(Value::as_str).unwrap_or("?");
        bail!("daemon op failed: {code}: {err}");
    }
    Ok(serde_json::from_value(resp)?)
}

/// Interrupt the in-flight turn of a claude worker WITHOUT killing it.
/// The control socket has no turn-interrupt op, but the `attach`
/// op opens a live PTY mirror: after the attach ack the supervisor pipes any
/// raw bytes written on this connection straight into the worker's PTY as
/// keystrokes (`Y.on("data", o => L.write(i.write(o)))`). So we do exactly
/// what a human does to abort a turn — attach, send a bare ESC, then detach.
/// The worker, session, and transcript all stay live and resumable.
///
/// The lone ESC is held briefly before we drop the connection so claude's
/// input parser resolves it as the Escape key rather than waiting for the
/// continuation byte of an escape sequence (the classic terminal ESC-timeout).
pub async fn attach_interrupt(socket: &Path, short: &str) -> Result<()> {
    // Raw ESC byte → PTY → the TUI aborts the in-flight turn.
    attach_send_keys(socket, short, b"\x1b").await
}

/// Answer a pending tool-permission prompt over the PTY. The control
/// socket's `permission-response` op is a no-op stub in current claude (it acks
/// `ok:true` but never resolves the prompt), so — exactly as the interrupt path
/// does for ESC — we attach and inject the keystroke a human would press:
/// `1`+Enter to approve (the highlighted "Yes" option), ESC to deny. Verified
/// against claude 2.1.161: approve runs the gated tool, deny skips it and the
/// worker returns to `tempo:"idle"`.
pub async fn attach_permission_response(socket: &Path, short: &str, allow: bool) -> Result<()> {
    if allow {
        attach_send_keys(socket, short, b"1\r").await
    } else {
        attach_send_keys(socket, short, b"\x1b").await
    }
}

/// How [`attach_submit`] decides an Enter actually submitted the draft.
#[derive(Debug, Clone)]
pub enum SubmitConfirm {
    /// Any PTY repaint within the confirm window. Only valid mid-turn, where
    /// the submit queues the message and the transcript won't grow until the
    /// turn picks it up; also the fallback when no transcript path is known.
    Repaint,
    /// A `"type":"user"` line appended to the transcript past `baseline`
    /// bytes — required for an idle worker: image-path ingestion repaints for
    /// seconds, so a swallowed Enter still "repaints" and the weaker check
    /// passes while the draft sits unsent.
    Transcript { path: PathBuf, baseline: u64 },
}

/// True once the transcript gained a complete `"type":"user"` line past
/// `baseline`. Reads only the appended region; partial trailing lines (a
/// write in flight) don't parse and simply don't match yet.
fn transcript_gained_user_entry(path: &Path, baseline: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    if !file.metadata().is_ok_and(|m| m.len() > baseline) {
        return false;
    }
    if file.seek(SeekFrom::Start(baseline)).is_err() {
        return false;
    }
    let mut appended = String::new();
    if file.read_to_string(&mut appended).is_err() {
        return false;
    }
    appended.lines().any(|line| {
        serde_json::from_str::<Value>(line)
            .ok()
            .is_some_and(|v| v.get("type").and_then(Value::as_str) == Some("user"))
    })
}

/// Submit the current draft in a worker PTY. Used after multiline `reply`
/// payloads: current Claude builds can accept the text into the composer but
/// leave it as an unsent draft, which is most visible for attachment messages
/// because the web UI appends staged file paths on separate lines.
///
/// The composer silently drops an Enter that arrives while it is still
/// ingesting the paste — image paths take seconds (claude reads the file and
/// swaps it for an `[Image #N]` placeholder), so no fixed delay is safe.
/// Wait for the screen to go quiet, press Enter, and re-press until `confirm`
/// proves it landed.
pub async fn attach_submit(socket: &Path, short: &str, confirm: &SubmitConfirm) -> Result<()> {
    /// PTY quiet period treated as "composer settled".
    const QUIET: std::time::Duration = std::time::Duration::from_millis(600);
    /// Cap on the settle wait. Multi-image ingest can repaint well past 10s,
    /// so a swallowed Enter here is expected — the confirm loop retries it.
    const SETTLE_MAX: std::time::Duration = std::time::Duration::from_secs(30);
    /// Window in which a submitted Enter must produce its confirm signal.
    const CONFIRM: std::time::Duration = std::time::Duration::from_millis(1500);
    /// Poll cadence for the transcript while draining PTY bytes.
    const POLL: std::time::Duration = std::time::Duration::from_millis(250);

    let (attempts, confirm_window) = match confirm {
        SubmitConfirm::Repaint => (3, CONFIRM),
        // Transcript growth lags the keypress (claude appends after the turn
        // starts), and each Enter swallowed mid-ingest burns an attempt: give
        // the strict mode more room.
        SubmitConfirm::Transcript { .. } => (6, std::time::Duration::from_secs(4)),
    };

    let (mut reader, mut write_half) = attach_handshake(socket, short).await?;
    let mut buf = [0_u8; 8192];

    let settle_start = tokio::time::Instant::now();
    loop {
        match tokio::time::timeout(QUIET, reader.read(&mut buf)).await {
            Ok(Ok(0)) => bail!("worker detached before draft submit"),
            Ok(Ok(_)) if settle_start.elapsed() < SETTLE_MAX => {}
            Ok(Ok(_)) | Err(_) => break,
            Ok(Err(err)) => return Err(err.into()),
        }
    }

    for attempt in 1..=attempts {
        write_half.write_all(b"\r").await?;
        write_half.flush().await?;
        let deadline = tokio::time::Instant::now() + confirm_window;
        let confirmed = loop {
            let submitted = match confirm {
                SubmitConfirm::Repaint => false,
                SubmitConfirm::Transcript { path, baseline } => {
                    transcript_gained_user_entry(path, *baseline)
                }
            };
            if submitted {
                break true;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break false;
            }
            // Drain PTY bytes while waiting: detects detach, and in Repaint
            // mode any output IS the confirmation.
            match tokio::time::timeout((deadline - now).min(POLL), reader.read(&mut buf)).await {
                Ok(Ok(0)) => bail!("worker detached during draft submit"),
                Ok(Ok(_)) if matches!(confirm, SubmitConfirm::Repaint) => break true,
                Ok(Ok(_)) | Err(_) => {}
                Ok(Err(err)) => return Err(err.into()),
            }
        };
        if confirmed {
            if attempt > 1 {
                tracing::info!(%short, attempt, "draft submit needed a retry Enter");
            }
            // Hold the connection so the keystroke is fully parsed before
            // detach fires the worker's attacher-close cleanup.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            return Ok(());
        }
    }
    bail!("draft submit Enter went unconfirmed after {attempts} attempts")
}

/// Answer a pending `AskUserQuestion` form natively: inject the
/// keystroke sequence a human would press, one chunk per UI step, paced so the
/// form's renderer keeps up between steps. The grammar (verified live against
/// claude 2.1.162):
///   - single-select question: the option digit selects AND auto-advances
///   - multiSelect question: digits toggle each option, Tab advances
///   - multi-question forms and any multiSelect end on a "Review your answers"
///     screen whose first option is "Submit answers" → a final `1` submits;
///     a lone single-select question submits straight from the digit.
///
/// Claude then records a genuine `tool_result` with the selected labels — no
/// "User declined to answer questions", no extra user turn.
pub async fn attach_answer_keys(socket: &Path, short: &str, chunks: &[Vec<u8>]) -> Result<()> {
    attach_send_chunks(socket, short, chunks).await
}

/// Attach to a worker's PTY mirror, inject `keys` as raw keystroke bytes, hold
/// briefly so the worker's input parser settles (notably so a lone ESC resolves
/// as Escape rather than the lead-in of an escape sequence), then detach. The
/// attach ack is read first so we never fire keys at a dead/unattachable worker.
async fn attach_send_keys(socket: &Path, short: &str, keys: &[u8]) -> Result<()> {
    attach_send_chunks(socket, short, std::slice::from_ref(&keys.to_vec())).await
}

/// Open an `attach` connection and read the ack. Post-ack the read half
/// streams raw PTY bytes and anything written is injected as keystrokes.
async fn attach_handshake(
    socket: &Path,
    short: &str,
) -> Result<(BufReader<OwnedReadHalf>, OwnedWriteHalf)> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();

    let mut attach_req =
        serde_json::json!({"proto":1,"op":"attach","short":short,"cols":120,"rows":40});
    inject_auth(&mut attach_req);
    let mut line = serde_json::to_string(&attach_req)?;
    line.push('\n');
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;

    let mut reader = BufReader::new(read_half);
    let mut buf = String::new();
    if reader.read_line(&mut buf).await? == 0 {
        bail!("daemon closed connection before attach ack");
    }
    let ack: Value = serde_json::from_str(buf.trim())
        .with_context(|| format!("decoding attach ack: {}", buf.trim()))?;
    if ack.get("ok") == Some(&Value::Bool(false)) {
        let code = ack.get("code").and_then(Value::as_str).unwrap_or("?");
        let err = ack.get("error").and_then(Value::as_str).unwrap_or("?");
        bail!("attach failed: {code}: {err}");
    }
    Ok((reader, write_half))
}

/// Shared attach-and-type core: one PTY attach, then each chunk of `chunks`
/// written 350ms apart so the TUI processes every step (a digit that selects,
/// a Tab that switches question, the submit confirm) before the next arrives.
/// The trailing hold doubles as the ESC-disambiguation delay `attach_send_keys`
/// has always needed.
async fn attach_send_chunks(socket: &Path, short: &str, chunks: &[Vec<u8>]) -> Result<()> {
    let (_reader, mut write_half) = attach_handshake(socket, short).await?;

    for (i, keys) in chunks.iter().enumerate() {
        if i > 0 {
            // Pace successive steps so the form renderer keeps up (verified
            // stable at 350ms against claude 2.1.162; digits/Tab dropped when
            // fired back-to-back).
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
        write_half.write_all(keys).await?;
        write_half.flush().await?;
    }
    // Hold the connection so the keystroke is parsed before detach (dropping
    // the stream fires the worker's attacher-close cleanup).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    Ok(())
}

/// Health check — `{"op":"ping"}` (no `proto` field per §4.2 of the
/// protocol doc). Currently used only in integration tests / manual
/// probing; the `list` op double-serves as a liveness check.
#[cfg(test)]
#[allow(dead_code)]
pub async fn ping(socket: &Path) -> Result<Value> {
    one_shot(socket, &serde_json::json!({"op": "ping"})).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end check against a *live* `claude daemon` control socket:
    /// proves the half-open fix (we must NOT close the write half) and that
    /// `call` surfaces `ok:false`. Spawns a real `fleet` session via the same
    /// payload shape `Driver::spawn` builds, then kills it. Ignored by
    /// default — run with a daemon present:
    ///   `cargo test -p cctui-daemon live_dispatch_roundtrip -- --ignored`
    #[tokio::test]
    #[ignore = "requires a live `claude daemon` control socket"]
    async fn live_dispatch_roundtrip() {
        let sock = super::super::discovery::Discovery::for_current_user()
            .locate()
            .expect("no live claude daemon socket — start `claude` first");

        let session_id = uuid::Uuid::new_v4().to_string();
        let short = &session_id[..8];
        let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        let req = serde_json::json!({
            "proto": 1, "op": "dispatch", "timeoutMs": 15000,
            "d": {
                "proto": 1, "short": short, "nonce": nonce, "sessionId": session_id,
                "createdAt": 0u64, "source": "fleet", "cwd": cwd,
                "launch": {"mode": "prompt", "args": [
                    "--session-id", session_id, "--agent", "claude", "--", "wait quietly"
                ]},
                "env": {}, "isolation": "none", "respawnFlags": ["--agent", "claude"],
                "agent": "claude", "seed": {"intent": "socket roundtrip test"},
                "cols": 120, "rows": 40,
            }
        });

        let resp: Value = call(&sock, &req).await.expect("dispatch should be accepted");
        assert_eq!(resp.get("op").and_then(Value::as_str), Some("dispatch"));

        // Clean up the session we just spawned.
        let _ = one_shot(
            &sock,
            &serde_json::json!({
                "proto": 1, "op": "kill", "short": short, "signal": "SIGTERM"
            }),
        )
        .await;
    }

    /// Proves seeding `name`/`nameSource` makes the daemon write the
    /// display name into `~/.claude/jobs/<short>/state.json`. Run with a live
    /// daemon: `cargo test -p cctui-daemon live_dispatch_seeds_name -- --ignored`.
    #[tokio::test]
    #[ignore = "requires a live `claude daemon` control socket"]
    async fn live_dispatch_seeds_name() {
        let sock = super::super::discovery::Discovery::for_current_user()
            .locate()
            .expect("no live claude daemon socket — start `claude` first");

        let session_id = uuid::Uuid::new_v4().to_string();
        let short = &session_id[..8];
        let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        let want_name = "cct-135 seed name probe";
        let req = serde_json::json!({
            "proto": 1, "op": "dispatch", "timeoutMs": 15000,
            "d": {
                "proto": 1, "short": short, "nonce": nonce, "sessionId": session_id,
                "createdAt": 0u64, "source": "fleet", "cwd": cwd,
                "launch": {"mode": "prompt", "args": [
                    "--session-id", session_id, "--agent", "claude",
                    "--name", want_name, "--", "wait quietly"
                ]},
                "env": {}, "isolation": "none", "respawnFlags": ["--agent", "claude"],
                "agent": "claude",
                "seed": {"intent": "", "name": want_name, "nameSource": "user"},
                "cols": 120, "rows": 40,
            }
        });
        let resp: Value = call(&sock, &req).await.expect("dispatch should be accepted");
        assert_eq!(resp.get("op").and_then(Value::as_str), Some("dispatch"));

        // The daemon writes state.json asynchronously; poll briefly.
        let jobs_root = super::super::state::default_jobs_root();
        let mut got = None;
        for _ in 0..40 {
            if let Some(s) = super::super::state::StateJson::read(&jobs_root, short)
                && s.name.is_some()
            {
                got = s.name;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let _ = one_shot(
            &sock,
            &serde_json::json!({
                "proto": 1, "op": "kill", "short": short, "signal": "SIGTERM"
            }),
        )
        .await;

        assert_eq!(got.as_deref(), Some(want_name), "seeded name should land in state.json");
    }

    /// Fake worker whose composer is still ingesting a pasted image when the
    /// first Enter arrives (it streams bytes past the quiet window, then
    /// swallows that Enter without repainting): `attach_submit` must wait out
    /// the ingest, detect the missing repaint, and land a retry Enter.
    #[tokio::test]
    async fn attach_submit_retries_swallowed_enter() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let mut req = String::new();
            reader.read_line(&mut req).await.unwrap();
            let req: Value = serde_json::from_str(req.trim()).unwrap();
            assert_eq!(req.get("op").and_then(Value::as_str), Some("attach"));
            write_half.write_all(b"{\"ok\":true,\"op\":\"attach\"}\n").await.unwrap();

            for _ in 0..3 {
                write_half.write_all(b"ingesting image...").await.unwrap();
                write_half.flush().await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }

            let mut key = [0_u8; 1];
            reader.read_exact(&mut key).await.unwrap();
            assert_eq!(key[0], b'\r', "first submit keystroke");

            reader.read_exact(&mut key).await.unwrap();
            assert_eq!(key[0], b'\r', "retry keystroke after no repaint");
            write_half.write_all(b"composer cleared, turn started").await.unwrap();
            write_half.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        });

        attach_submit(&sock, "aaaaaaaa", &SubmitConfirm::Repaint)
            .await
            .expect("submit should succeed via retry");
        server.await.unwrap();
    }

    /// Transcript mode must NOT trust repaints: the fake worker keeps
    /// streaming ingest output after the first Enter (the exact false-confirm
    /// of the image-upload bug) and only appends the user entry to the
    /// transcript after the second Enter — success requires that retry.
    #[tokio::test]
    async fn attach_submit_transcript_ignores_repaint_noise() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("control.sock");
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(&transcript, "{\"type\":\"assistant\"}\n").unwrap();
        let baseline = std::fs::metadata(&transcript).unwrap().len();
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();

        let transcript_srv = transcript.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let mut req = String::new();
            reader.read_line(&mut req).await.unwrap();
            write_half.write_all(b"{\"ok\":true,\"op\":\"attach\"}\n").await.unwrap();

            let mut key = [0_u8; 1];
            reader.read_exact(&mut key).await.unwrap();
            assert_eq!(key[0], b'\r', "first submit keystroke");
            for _ in 0..4 {
                write_half.write_all(b"still ingesting [Image #2]...").await.unwrap();
                write_half.flush().await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }

            reader.read_exact(&mut key).await.unwrap();
            assert_eq!(key[0], b'\r', "retry despite repaint noise");
            let mut f = std::fs::OpenOptions::new().append(true).open(&transcript_srv).unwrap();
            std::io::Write::write_all(&mut f, b"{\"type\":\"user\"}\n").unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        });

        attach_submit(&sock, "aaaaaaaa", &SubmitConfirm::Transcript { path: transcript, baseline })
            .await
            .expect("submit should confirm via transcript growth");
        server.await.unwrap();
    }

    /// A dead worker (server FIN right after the ack) must surface as an error,
    /// not a silent success.
    #[tokio::test]
    async fn attach_submit_errors_on_detach() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut req = String::new();
            reader.read_line(&mut req).await.unwrap();
            write_half.write_all(b"{\"ok\":true,\"op\":\"attach\"}\n").await.unwrap();
        });

        let err = attach_submit(&sock, "aaaaaaaa", &SubmitConfirm::Repaint)
            .await
            .expect_err("FIN must be an error");
        assert!(err.to_string().contains("detached"), "unexpected error: {err}");
        server.await.unwrap();
    }
}
