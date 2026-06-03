//! Newline-delimited JSON client for the `claude daemon` control socket.
//!
//! Wire format: one UTF-8 JSON object per line. Most ops are one-request /
//! one-response on a fresh connection; `subscribe` and `attach` keep the
//! connection open and stream multiple response lines.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Send a single request and read a single JSON-line response.
pub async fn one_shot(socket: &Path, request: &Value) -> Result<Value> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;
    // Deliberately keep the write half open until the response is read. The
    // `dispatch` op drops the request as stale the moment it observes EOF on
    // the read side (`tengu_bg_dispatch_stale_drop`), so half-closing here —
    // as an earlier `drop(write_half)` did — silently killed every spawn
    // (CCT-131). Other ops don't care, so holding it open is universally safe.

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

/// Interrupt the in-flight turn of a claude worker WITHOUT killing it
/// (CCT-210). The control socket has no turn-interrupt op, but the `attach`
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
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();

    let mut line = serde_json::to_string(
        &serde_json::json!({"proto":1,"op":"attach","short":short,"cols":120,"rows":40}),
    )?;
    line.push('\n');
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;

    // Read the attach ack so we don't fire ESC at a dead/unattachable worker.
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

    // Raw ESC byte → PTY → the TUI aborts the in-flight turn.
    write_half.write_all(b"\x1b").await?;
    write_half.flush().await?;
    // Hold the connection so the lone ESC is parsed as Escape, then detach
    // (dropping the stream fires the worker's attacher-close cleanup).
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

    /// Proves CCT-135: seeding `name`/`nameSource` makes the daemon write the
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
}
