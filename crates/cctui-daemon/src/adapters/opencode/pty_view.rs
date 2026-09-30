//! `OpenCode`'s live view: the adapter-neutral ring viewer, fed by the session
//! driver's `SessionCommand::Diagnose` snapshot.

use cctui_proto::adapter::AdapterEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::session::{LiveRegistry, SessionCommand};
use crate::adapter_runtime::PtyWatch;
use crate::adapters::ring_view::{POLL_INTERVAL, RingViewManager, TrafficSnapshot};

/// Drains the adapter's out-of-band `WatchPty` channel, so a live view is
/// served while the serial command loop is busy with a turn.
pub(super) struct PtyWatchPump {
    views: RingViewManager,
    live: LiveRegistry,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
}

impl PtyWatchPump {
    pub(super) fn new(
        live: LiveRegistry,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        Self { views: RingViewManager::default(), live, events, shutdown }
    }

    pub(super) async fn run(self, mut watches: mpsc::Receiver<PtyWatch>) {
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                watch = watches.recv() => {
                    let Some((local_id, watch)) = watch else { return };
                    if watch {
                        let live = self.live.clone();
                        self.views.watch(
                            local_id,
                            self.events.clone(),
                            &self.shutdown,
                            move |id| {
                                let live = live.clone();
                                async move { snapshot(&live, &id).await }
                            },
                        );
                    } else {
                        self.views.unwatch(&local_id);
                    }
                }
            }
        }
    }
}

pub(super) async fn snapshot(live: &LiveRegistry, local_id: &str) -> Option<TrafficSnapshot> {
    let tx = live.lock().await.get(local_id).map(|s| s.commands.clone())?;
    let (reply, mut rx) = mpsc::channel(1);
    tx.send(SessionCommand::Diagnose { reply }).await.ok()?;
    let snap = tokio::time::timeout(POLL_INTERVAL, rx.recv()).await.ok().flatten()?;
    Some(TrafficSnapshot {
        protocol_errors: snap.protocol_errors,
        stderr_tail: snap.stderr_tail,
        rpc_tail: snap.rpc_tail,
    })
}
