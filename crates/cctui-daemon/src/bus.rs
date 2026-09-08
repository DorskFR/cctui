//! In-process event/command bus.
//!
//! Each adapter owns a pair of bounded mpsc channels:
//!   * `events`   — adapter → daemon → server (256-deep)
//!   * `commands` — server → daemon → adapter (64-deep)
//!
//! The supervisor instantiates one `AdapterChannels` per active adapter
//! and multiplexes them onto the WS.

use cctui_proto::adapter::{AdapterCommand, AdapterEvent};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::adapter_runtime::AdapterCtx;

const EVENT_BUFFER: usize = 256;
const COMMAND_BUFFER: usize = 64;

pub struct AdapterChannels {
    pub events_rx: mpsc::Receiver<AdapterEvent>,
    pub commands_tx: mpsc::Sender<AdapterCommand>,
}

#[must_use]
pub fn build_ctx(
    config: serde_json::Value,
    shutdown: CancellationToken,
    server: Option<crate::client::ServerClient>,
    machine_key: Option<String>,
    connected: &broadcast::Sender<()>,
) -> (AdapterCtx, AdapterChannels) {
    let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
    let (commands_tx, commands_rx) = mpsc::channel(COMMAND_BUFFER);
    let ctx = AdapterCtx {
        events: events_tx,
        commands: commands_rx,
        shutdown,
        config,
        server,
        machine_key,
        // Subscribed here, synchronously, so the connect edge that follows this
        // adapter's first build is not missed: a broadcast receiver only gets
        // sends made after it subscribed.
        connected: connected.subscribe(),
    };
    let channels = AdapterChannels { events_rx, commands_tx };
    (ctx, channels)
}
