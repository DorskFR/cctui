use std::process::Stdio;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::config::AppServerConfig;
use super::requests::{initialize_req, initialized_notification};
use super::rpc::{RPC_TIMEOUT, RUN_BASE, response_outcome, write_json};

/// A native codex thread lifecycle operation. Each maps to a single
/// JSON-RPC method taking `{ threadId }`. Archive/unarchive are wired to the
/// CCTUI archive/reopen actions; `Delete` implements the third native op for
/// parity (no CCTUI destructive-delete action wires to it yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleOp {
    Archive,
    Unarchive,
    #[allow(dead_code)]
    Delete,
}

impl LifecycleOp {
    #[must_use]
    const fn method(self) -> &'static str {
        match self {
            Self::Archive => "thread/archive",
            Self::Unarchive => "thread/unarchive",
            Self::Delete => "thread/delete",
        }
    }
}

pub(super) fn thread_lifecycle_req(id: i64, op: LifecycleOp, thread_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": op.method(),
        "params": {"threadId": thread_id},
    })
}

/// Whether a `thread/{archive,unarchive,delete}` JSON-RPC error can be treated
/// as success for idempotency: the thread is already in the target
/// state or no longer exists, so CCTUI and native lifecycle state can't wedge
/// each other. Matched on the codex error text since the app-server exposes no
/// stable machine codes for these.
#[must_use]
pub fn is_idempotent_lifecycle_error(op: LifecycleOp, err: &str) -> bool {
    let e = err.to_lowercase();
    // A missing thread makes any lifecycle op a no-op success.
    let missing = e.contains("not found")
        || e.contains("no such")
        || e.contains("does not exist")
        || e.contains("doesn't exist")
        || e.contains("unknown thread")
        || e.contains("no thread");
    // Already in the requested terminal state.
    let already = match op {
        LifecycleOp::Archive => e.contains("already archived"),
        LifecycleOp::Unarchive => e.contains("already unarchived") || e.contains("not archived"),
        LifecycleOp::Delete => e.contains("already deleted"),
    };
    missing || already
}

/// Run a native codex thread lifecycle op via a short-lived stdio
/// `codex app-server`, mirroring the one-shot pattern the
/// [`crate::adapters::codex::thread_list`] inventory poll uses. Spawns the app-server, sends
/// `initialize` → `initialized` → the lifecycle RPC, correlates the response by
/// id, and reaps the process. Idempotent: an "already in target state" /
/// "thread missing" error resolves as success ([`is_idempotent_lifecycle_error`])
/// so CCTUI and native lifecycle state can't wedge each other. No gateway env is
/// needed — no turn is started.
pub async fn run_thread_lifecycle(
    app: &AppServerConfig,
    daemon: Option<&crate::adapters::codex::daemon::SharedDaemon>,
    thread_id: &str,
    op: LifecycleOp,
) -> Result<()> {
    if let Some(shared) = daemon
        && let Some(handle) = shared.handle().await
    {
        match handle.request(op.method(), json!({"threadId": thread_id})).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                let msg = err.to_string();
                if is_idempotent_lifecycle_error(op, &msg) {
                    tracing::info!(
                        %thread_id,
                        op = op.method(),
                        "codex lifecycle op idempotent no-op: {msg}"
                    );
                    return Ok(());
                }
                tracing::debug!(%err, op = op.method(), "codex: shared lifecycle op failed, using stdio");
            }
        }
    }
    let mut cmd = Command::new(&app.bin);
    cmd.arg("app-server")
        // No turn is started, so sandbox mode only matters because codex
        // refuses to boot when it cannot create the bwrap namespace on some
        // kernels — pass the configured (host-default) mode through.
        .arg("-c")
        .arg(format!("sandbox_mode=\"{}\"", app.sandbox_mode))
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().context("codex app-server stdin unavailable")?;
    let stdout = child.stdout.take().context("codex app-server stdout unavailable")?;

    let req_id = RUN_BASE;
    let outcome = tokio::time::timeout(RPC_TIMEOUT, async {
        let mut lines = BufReader::new(stdout).lines();
        write_json(&mut stdin, &initialize_req()).await?;
        write_json(&mut stdin, &initialized_notification()).await?;
        write_json(&mut stdin, &thread_lifecycle_req(req_id, op, thread_id)).await?;
        while let Some(line) = lines.next_line().await? {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
            if v.get("id").and_then(Value::as_i64) == Some(req_id) {
                return anyhow::Ok(response_outcome(&v));
            }
        }
        anyhow::bail!("codex {} response not received before EOF", op.method())
    })
    .await;

    // Close stdin and reap regardless of how the read went.
    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;

    match outcome {
        Err(_) => anyhow::bail!("codex {} timed out", op.method()),
        Ok(Err(e)) => Err(e),
        Ok(Ok(Ok(_))) => Ok(()),
        Ok(Ok(Err(msg))) => {
            if is_idempotent_lifecycle_error(op, &msg) {
                tracing::info!(
                    %thread_id,
                    op = op.method(),
                    "codex lifecycle op idempotent no-op: {msg}"
                );
                Ok(())
            } else {
                Err(anyhow::anyhow!(msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn lifecycle_request_shapes() {
        for (op, method) in [
            (LifecycleOp::Archive, "thread/archive"),
            (LifecycleOp::Unarchive, "thread/unarchive"),
            (LifecycleOp::Delete, "thread/delete"),
        ] {
            let req = thread_lifecycle_req(7, op, "thread-abc");
            assert_eq!(req["jsonrpc"], "2.0");
            assert_eq!(req["id"], 7);
            assert_eq!(req["method"], method);
            assert_eq!(req["params"]["threadId"], "thread-abc");
        }
    }

    #[test]
    fn lifecycle_idempotency_maps_already_in_state() {
        assert!(is_idempotent_lifecycle_error(
            LifecycleOp::Archive,
            "codex app-server error: thread is already archived"
        ));
        assert!(is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "thread is not archived"));
        assert!(is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "already unarchived"));
        assert!(is_idempotent_lifecycle_error(LifecycleOp::Delete, "already deleted"));
    }

    #[test]
    fn lifecycle_idempotency_maps_missing_thread_for_every_op() {
        for op in [LifecycleOp::Archive, LifecycleOp::Unarchive, LifecycleOp::Delete] {
            assert!(is_idempotent_lifecycle_error(op, "thread not found"));
            assert!(is_idempotent_lifecycle_error(op, "No such thread: abc"));
            assert!(is_idempotent_lifecycle_error(op, "thread does not exist"));
        }
    }

    #[test]
    fn lifecycle_idempotency_rejects_real_errors() {
        assert!(!is_idempotent_lifecycle_error(
            LifecycleOp::Archive,
            "codex app-server error 500: internal error"
        ));
        assert!(!is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "permission denied"));
        // An archive-specific "already" must not mask a genuine unarchive fault.
        assert!(!is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "already archived"));
    }

    /// The per-op lifecycle child is gone: archive/unarchive are answered over
    /// the shared socket with no `codex` binary present to spawn.
    #[tokio::test]
    async fn lifecycle_ops_never_spawn_a_child_when_the_daemon_answers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("app-server.sock");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let _srv =
            crate::adapters::codex::daemon::testserver::spawn(&sock, move |method, params| {
                recorder.lock().unwrap().push(method.to_owned());
                assert_eq!(params["threadId"], "tid-1");
                json!({})
            });

        let shutdown = CancellationToken::new();
        let shared = crate::adapters::codex::daemon::SharedDaemon::from_endpoint(
            crate::adapters::codex::daemon::DaemonEndpoint { socket: sock },
            shutdown.clone(),
        );
        let mut app = AppServerConfig::from_value(&json!({}));
        app.bin = "/nonexistent/codex-must-not-be-spawned".to_owned();

        for op in [LifecycleOp::Archive, LifecycleOp::Unarchive] {
            run_thread_lifecycle(&app, Some(&shared), "tid-1", op)
                .await
                .expect("served over the ws");
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["thread/archive".to_owned(), "thread/unarchive".to_owned()]
        );
        shutdown.cancel();
    }
}
