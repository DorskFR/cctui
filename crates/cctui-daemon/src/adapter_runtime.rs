//! Runtime side of the adapter contract.
//!
//! The wire types ([`cctui_proto::adapter::AdapterEvent`] and friends) live
//! in `cctui-proto` and stay runtime-free. The `Adapter` trait here adds
//! the async/tokio surface — adapters compiled into the daemon implement
//! this trait, and the supervisor drives them.

use cctui_proto::adapter::{AdapterCommand, AdapterEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Per-adapter execution context handed to [`Adapter::start`].
pub struct AdapterCtx {
    /// Outbound: adapter pushes events here; the daemon multiplexes them
    /// into the server WS.
    pub events: mpsc::Sender<AdapterEvent>,
    /// Inbound: daemon pushes commands targeting this adapter here.
    pub commands: mpsc::Receiver<AdapterCommand>,
    /// Daemon-wide shutdown signal. Adapters MUST observe it and return
    /// cleanly when it fires.
    pub shutdown: CancellationToken,
    /// Adapter-specific declarative config from `adapters_enabled.config`.
    pub config: serde_json::Value,
    /// Authenticated client back to the cctui-server. Lets an adapter
    /// pull launch-time data the server owns — currently the per-session gateway
    /// env resolved from `sessions.account_id`. `None` outside a real daemon run
    /// (tests construct ctx without a server).
    pub server: Option<crate::client::ServerClient>,
    /// The daemon's machine key, paired with `server` for authenticated pulls.
    pub machine_key: Option<String>,
}

#[async_trait::async_trait]
pub trait Adapter: Send + Sync {
    fn id(&self) -> &'static str;
    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()>;
}

/// Compile-time-registered adapter factory.
pub trait AdapterFactory: Send + Sync {
    fn id(&self) -> &'static str;
    fn build(&self, config: serde_json::Value) -> Box<dyn Adapter>;
}
