//! Restart, cold and once, a spare-claimed worker that came up without cctui's
//! hooks: a cold launch reads the `--settings` file a spare claim can miss.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use super::super::canary;
use super::lanes::LaneOutcome;
use super::reply::ReplyJob;
use super::{AdapterEvent, Driver, LiveSnapshot, removal, spawn};

const RESPAWN_FAILED: &str = "cctui could not restart the worker to load its hooks";

const REASK: &str = "cctui restarted this session to load its hooks, which closed your open \
                     AskUserQuestion before the user could answer it. Ask the same question \
                     again with AskUserQuestion.";

/// A point where the owed respawn may run. Without hooks an `AskUserQuestion`
/// form is invisible to cctui and its tempo stays `blocked`, so waiting for
/// `idle` would wait forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RespawnPoint {
    Idle,
    Asking,
}

impl RespawnPoint {
    pub(super) fn of(job: &LiveSnapshot) -> Option<Self> {
        if job.dying {
            return None;
        }
        match job.tempo.as_deref() {
            Some("idle") => Some(Self::Idle),
            Some("blocked") if job.needs.as_deref().is_some_and(is_question) => Some(Self::Asking),
            _ => None,
        }
    }
}

/// Claude Code's `needs` for an open `AskUserQuestion` form: `answer: <question>
/// (<options>)`, or `answer question` when it has no text.
fn is_question(needs: &str) -> bool {
    let needs = needs.trim();
    needs == "answer question" || needs.starts_with("answer:")
}

/// Whether `short`'s live roster entry still shows a question form. Checked
/// before an ESC, which would abort the turn if the form already closed.
pub(super) async fn still_asking(sock: &Path, short: &str) -> bool {
    let list = serde_json::json!({"proto": 1, "op": "list"});
    super::socket::call::<super::ListResponse>(sock, &list).await.is_ok_and(|resp| {
        resp.jobs
            .iter()
            .any(|j| j.short == short && RespawnPoint::of(j) == Some(RespawnPoint::Asking))
    })
}

const fn may_respawn(point: Option<RespawnPoint>, lane_busy: bool, ask_open: bool) -> bool {
    point.is_some() && !lane_busy && !ask_open
}

impl Driver {
    /// `points` is keyed by short, not session id.
    pub(super) fn respawn_hookless_workers(
        &mut self,
        sock: &Path,
        points: &HashMap<String, RespawnPoint>,
    ) {
        for (local_id, short) in canary::respawns_due() {
            let point = points.get(&short).copied();
            let ask_open = self.pending_asks.lock().is_ok_and(|m| m.contains_key(&local_id));
            if !may_respawn(point, self.lanes.is_busy(&local_id), ask_open)
                || !canary::claim_respawn(&local_id)
            {
                continue;
            }
            tracing::warn!(%local_id, %short, ?point, "restarting a hookless spare-claimed worker");
            let reask = (point == Some(RespawnPoint::Asking))
                .then(|| self.reply_job_for(sock, &short, &local_id, REASK, None, BTreeMap::new()));
            let op = respawn_cold(
                self.launcher(),
                self.remover(),
                sock.to_path_buf(),
                short,
                local_id.clone(),
                reask,
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
    reask: Option<ReplyJob>,
) -> LaneOutcome {
    let mut result = restart(&launcher, &remover, &sock, &short, &local_id).await;
    if let (Ok(()), Some(job)) = (&result, reask) {
        result = Box::pin(job.deliver()).await.context("asking the model to re-ask its question");
    }
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
    use super::{LiveSnapshot, RespawnPoint, may_respawn};

    fn job(tempo: &str, needs: Option<&str>) -> LiveSnapshot {
        serde_json::from_value(serde_json::json!({
            "short": "abcd1234",
            "tempo": tempo,
            "needs": needs,
        }))
        .unwrap()
    }

    #[test]
    fn the_respawn_waits_for_a_stopping_point_with_nothing_in_flight() {
        let idle = Some(RespawnPoint::Idle);
        assert!(may_respawn(idle, false, false));
        assert!(may_respawn(Some(RespawnPoint::Asking), false, false));
        assert!(!may_respawn(None, false, false), "a turn is still running");
        assert!(!may_respawn(idle, true, false), "a reply or command holds the lane");
        assert!(!may_respawn(idle, false, true), "a hook-delivered question is open");
    }

    #[test]
    fn a_worker_blocked_on_a_question_form_is_a_respawn_point() {
        let asking = job("blocked", Some("answer: Which runner? (B1 · B2 · B3)"));
        assert_eq!(RespawnPoint::of(&asking), Some(RespawnPoint::Asking));
        let untitled = job("blocked", Some("answer question"));
        assert_eq!(RespawnPoint::of(&untitled), Some(RespawnPoint::Asking));
        assert_eq!(RespawnPoint::of(&job("idle", None)), Some(RespawnPoint::Idle));
    }

    #[test]
    fn other_blocked_or_busy_workers_are_not_respawn_points() {
        assert_eq!(RespawnPoint::of(&job("active", None)), None);
        assert_eq!(RespawnPoint::of(&job("blocked", Some("approve Bash: ls"))), None);
        assert_eq!(RespawnPoint::of(&job("blocked", Some("approve plan"))), None);
        assert_eq!(RespawnPoint::of(&job("blocked", Some("answering the review"))), None);
        assert_eq!(RespawnPoint::of(&job("blocked", None)), None);
        let mut dying = job("idle", None);
        dying.dying = true;
        assert_eq!(RespawnPoint::of(&dying), None);
    }
}
