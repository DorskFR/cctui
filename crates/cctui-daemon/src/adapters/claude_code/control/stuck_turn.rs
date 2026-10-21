use std::time::Instant;

use super::super::stuck_turn::{self, StuckAction, StuckTurnWatchdog};
use super::{AdapterEvent, DispatchDoneTracker, Driver, EndReason, LiveSnapshot};

impl Driver {
    pub(super) fn arm_stuck_turn(&self, session_id: &str) {
        let env = |name: &str| std::env::var(name).ok();
        let reprompt = stuck_turn::secs_from_env(
            env("CCTUI_STUCK_TURN_REPROMPT_SECS").as_deref(),
            stuck_turn::DEFAULT_REPROMPT_SECS,
        );
        let end = stuck_turn::secs_from_env(
            env("CCTUI_STUCK_TURN_END_SECS").as_deref(),
            stuck_turn::DEFAULT_END_SECS,
        );
        if let Ok(mut guard) = self.stuck_turn.lock() {
            *guard = Some(StuckTurnWatchdog::new(session_id, reprompt, end));
        }
    }

    /// No-op unless `maybe_dispatch_on_start` armed the watchdog.
    pub(super) async fn tick_stuck_turn(&self, jobs: &[LiveSnapshot]) {
        let marker_written = self
            .dispatch_done
            .lock()
            .is_ok_and(|g| g.as_ref().is_some_and(DispatchDoneTracker::is_done));
        if marker_written {
            return;
        }
        let Some((short, action)) = self.observe_stuck_turn(jobs) else { return };
        let Some(local_id) = self.transcript_locations.get(&short).map(|l| l.local_id.clone())
        else {
            return;
        };
        match action {
            StuckAction::Reprompt => self.reprompt_stuck_turn(&short, &local_id).await,
            StuckAction::End => self.end_stuck_turn(&short, local_id).await,
        }
    }

    fn observe_stuck_turn(&self, jobs: &[LiveSnapshot]) -> Option<(String, StuckAction)> {
        let mut guard = self.stuck_turn.lock().ok()?;
        let watchdog = guard.as_mut()?;
        let loc = self.transcript_locations.get(watchdog.short())?;
        let job = jobs.iter().find(|j| j.short == watchdog.short())?;
        let turn_running =
            !job.is_dead() && matches!(job.tempo.as_deref(), Some("active" | "blocked"));
        let grew = watchdog.ledger.scan(&loc.path);
        let outputs = watchdog.outputs_changed();
        let action = watchdog.observe(turn_running || grew || outputs, Instant::now())?;
        Some((watchdog.short().to_owned(), action))
    }

    async fn reprompt_stuck_turn(&self, short: &str, local_id: &str) {
        tracing::warn!(
            %short,
            "dispatched session idle on an unfinished background task; re-prompting"
        );
        let delivered = match self.ensure_socket().await {
            Ok(sock) => {
                let env = std::collections::BTreeMap::default();
                Box::pin(self.deliver_reply(
                    &sock,
                    local_id,
                    stuck_turn::REPROMPT_TEXT,
                    None,
                    &env,
                    None,
                ))
                .await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = delivered {
            tracing::warn!(%err, %short, "stuck-turn re-prompt failed");
        }
    }

    async fn end_stuck_turn(&self, short: &str, local_id: String) {
        tracing::warn!(%short, "dispatched session still idle after the re-prompt; ending it");
        self.emit(AdapterEvent::SessionEnded {
            local_id,
            reason: EndReason::Other { detail: stuck_turn::END_DETAIL.to_owned() },
        })
        .await;
        let marker = self.cfg.jobs_root.join(short).join("dispatch_done");
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&marker, b"") {
            tracing::warn!(%err, path = %marker.display(), "failed to write dispatch_done marker");
        }
    }
}
