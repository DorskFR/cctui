//! Restart, cold and once, a spare-claimed worker that came up without cctui's
//! hooks: a cold launch reads the `--settings` file a spare claim can miss.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::super::canary;
use super::lanes::LaneOutcome;
use super::{AdapterEvent, Driver, removal, spawn};

const RESPAWN_FAILED: &str = "cctui could not restart the worker to load its hooks";

const fn may_respawn(idle: bool, lane_busy: bool, ask_open: bool) -> bool {
    idle && !lane_busy && !ask_open
}

impl Driver {
    /// `idle` holds shorts, not session ids.
    pub(super) fn respawn_hookless_workers(&mut self, sock: &Path, idle: &HashSet<String>) {
        for (local_id, short) in canary::respawns_due() {
            let ask_open = self.pending_asks.lock().is_ok_and(|m| m.contains_key(&local_id));
            if !may_respawn(idle.contains(&short), self.lanes.is_busy(&local_id), ask_open)
                || !canary::claim_respawn(&local_id)
            {
                continue;
            }
            tracing::warn!(%local_id, %short, "restarting a hookless spare-claimed worker");
            let op = respawn_cold(
                self.launcher(),
                self.remover(),
                sock.to_path_buf(),
                short,
                local_id.clone(),
            );
            self.run_in_lane(&local_id, None, op);
        }
    }
}

async fn respawn_cold(
    launcher: spawn::Launcher,
    remover: removal::Remover,
    sock: PathBuf,
    short: String,
    local_id: String,
) -> LaneOutcome {
    let result = restart(&launcher, &remover, &sock, &short, &local_id).await;
    let text = match &result {
        Ok(()) => canary::RESPAWNED.to_owned(),
        Err(err) => format!("{RESPAWN_FAILED}: {err:#}"),
    };
    let payload = canary::marker_payload(&text);
    let message = AdapterEvent::Message { local_id, payload, turn_id: None };
    let _ = launcher.events.send(message).await;
    result.into()
}

/// The launch pull is checked first: a worker stopped without one to relaunch
/// on would lose its gateway routing.
async fn restart(
    launcher: &spawn::Launcher,
    remover: &removal::Remover,
    sock: &Path,
    short: &str,
    local_id: &str,
) -> anyhow::Result<()> {
    let launch = launcher.resolve_launch_env(local_id, &BTreeMap::new()).await?;
    anyhow::ensure!(launch.pulled, "the server did not answer the launch pull");
    remover.stop_worker(sock, short).await;
    anyhow::ensure!(
        removal::Remover::await_worker_exit(sock, short).await,
        "the worker did not exit"
    );
    launcher.resume_worker(sock, short, local_id, None, None, &launch.env).await
}

#[cfg(test)]
mod tests {
    use super::may_respawn;

    #[test]
    fn the_respawn_waits_for_an_idle_worker_with_nothing_in_flight() {
        assert!(may_respawn(true, false, false));
        assert!(!may_respawn(false, false, false), "a turn is still running");
        assert!(!may_respawn(true, true, false), "a reply or command holds the lane");
        assert!(!may_respawn(true, false, true), "a question is open");
    }
}
