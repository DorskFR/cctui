//! Headless stream-json drivers for the claude-code adapter (CCT-497).
//!
//! These are **stubs**: the [`Mode::Oneshot`](super::mode::Mode::Oneshot) and
//! [`Mode::Sdk`](super::mode::Mode::Sdk) dispatch points wire up here so the
//! adapter selects them without panicking, but the real run loop (spawn the
//! CLI / SDK, pump [`streamjson`](super::streamjson) over its stdio, forward
//! events + accept commands) lands in follow-up tickets.
//!
//! Until then each driver logs that the mode is not yet implemented and idles
//! until daemon shutdown, observing the [`AdapterCtx`] contract (return cleanly
//! when the shutdown token fires) rather than erroring — an unimplemented mode
//! shouldn't take the whole daemon down.

use crate::adapter_runtime::AdapterCtx;

/// Single-shot `claude --print --output-format stream-json` driver (stub).
pub(super) struct OneshotDriver {
    ctx: AdapterCtx,
}

impl OneshotDriver {
    pub(super) fn new(ctx: AdapterCtx) -> Self {
        Self { ctx }
    }

    pub(super) async fn run(self) -> anyhow::Result<()> {
        run_stub("oneshot", self.ctx).await
    }
}

/// Claude Agent SDK stream-json driver (stub).
pub(super) struct SdkDriver {
    ctx: AdapterCtx,
}

impl SdkDriver {
    pub(super) fn new(ctx: AdapterCtx) -> Self {
        Self { ctx }
    }

    pub(super) async fn run(self) -> anyhow::Result<()> {
        run_stub("sdk", self.ctx).await
    }
}

/// Shared stub body: warn once, then park until shutdown. Draining `commands`
/// keeps the daemon's command channel from backing up while the real driver is
/// unimplemented; each command is acknowledged only by being dropped.
async fn run_stub(mode: &'static str, mut ctx: AdapterCtx) -> anyhow::Result<()> {
    tracing::warn!(
        mode,
        "claude-code {mode} driver is not yet implemented (CCT-497 stub); idling until shutdown",
    );
    loop {
        tokio::select! {
            () = ctx.shutdown.cancelled() => return Ok(()),
            cmd = ctx.commands.recv() => match cmd {
                Some(cmd) => tracing::debug!(?cmd, mode, "claude-code stub driver dropping command"),
                // Sender closed: nothing more will arrive — wait for shutdown.
                None => {
                    ctx.shutdown.cancelled().await;
                    return Ok(());
                }
            },
        }
    }
}
