use super::{
    AdapterEvent, Driver, EndReason, HashMap, Instant, Path, PathBuf, TranscriptLocation,
    transcript,
};

impl Driver {
    /// Periodic (and churn-`force`d) divergence check. The forward tail keeps
    /// `server_marks` level with the offset as it emits, so the periodic pass
    /// only fires for a session whose offset moved without an emit. A forced
    /// pass re-sends to sessions the server has not acked up to the offset.
    /// Either way only the gap behind the offset is re-sent, never bytes the
    /// server acked.
    pub(super) async fn reconcile_tail(&mut self, force: bool) {
        self.last_reconcile = Instant::now();
        let locations: Vec<TranscriptLocation> =
            self.transcript_locations.values().cloned().collect();
        for loc in locations {
            let local = self.offsets.get(&loc.offset_key);
            let acked = self.acked_marks.get(&loc.offset_key).copied();
            if acked.is_some_and(|a| a >= local) {
                continue;
            }
            let server = self.server_marks.get(&loc.offset_key).copied().unwrap_or(0);
            if force || local > server {
                let from = if force { acked } else { Some(server).max(acked) };
                self.resend_window(&loc, from).await;
                self.server_marks.insert(loc.offset_key.clone(), local);
            }
        }
    }

    /// Path of a session's live transcript, following a worktree move. The
    /// launch-cwd path is authoritative while it exists (cheap, no scan); once
    /// `EnterWorktree` relocates the file the launch path vanishes, so fall back
    /// to the newest `<sess>.jsonl` found across project dirs.
    pub(super) fn resolve_live_transcript(&self, cwd: &str, sess: &str) -> PathBuf {
        let launch = transcript::transcript_path(&self.cfg.projects_root, cwd, sess);
        if launch.exists() {
            return launch;
        }
        transcript::newest_transcript_for_session(&self.cfg.projects_root, sess).unwrap_or(launch)
    }

    /// One last forward tail on shutdown so the tail of the conversation (a
    /// final `tool_use` and its error) reaches the server before a dispatched
    /// pod is reaped. Best-effort: the claude daemon usually outlives us at
    /// teardown, but a failed poll must not stall the shutdown.
    pub(super) async fn flush_before_teardown(&mut self) {
        if let Err(err) = self.poll_once().await {
            tracing::debug!(%err, "final teardown tail failed");
        }
        self.reconcile_tail(true).await;
        self.offsets.flush();
    }

    /// The offset to tail a session from, fast-forwarded to a server resume mark
    /// that sits AHEAD of our persisted offset — the cold-start /
    /// restart case where in-memory offsets are empty but the server already
    /// holds the transcript. Bounded by the file length so a stale mark past a
    /// truncated/rotated file can't skip live bytes. Persists the clamp so a
    /// later poll doesn't re-clamp.
    pub(super) fn resume_offset(&mut self, key: &str, path: &Path) -> u64 {
        let local = self.offsets.get(key);
        if let Some(&mark) = self.server_marks.get(key)
            && mark > local
        {
            let bounded = clamp_to_file_len(path, mark);
            if bounded > local {
                self.offsets.set(key.to_owned(), bounded);
                return bounded;
            }
        }
        local
    }

    /// Apply server-pushed transcript resume marks: record each mark,
    /// clamp the cursor of any session already ahead-clampable forward, and heal
    /// a session we already tail whose offset has run ahead of (or has no) mark
    /// with a single bounded re-send window — the one-time heal that replaces the
    /// old periodic re-tail.
    pub(super) async fn apply_resume_marks(&mut self, marks: Vec<(String, u64)>) {
        let mark_map: HashMap<String, u64> = marks.into_iter().collect();
        for (key, mark) in &mark_map {
            let entry = self.server_marks.entry(key.clone()).or_insert(0);
            *entry = (*entry).max(*mark);
            self.acked_marks.insert(key.clone(), *mark);
        }
        let locations: Vec<TranscriptLocation> =
            self.transcript_locations.values().cloned().collect();
        let mut dirty = false;
        for loc in locations {
            let prev = self.offsets.get(&loc.offset_key);
            if self.resume_offset(&loc.offset_key, &loc.path) != prev {
                // Clamped forward: the server already has this, no re-send.
                dirty = true;
                continue;
            }
            let behind_or_absent = match mark_map.get(&loc.offset_key) {
                Some(&mark) => mark < prev,
                None => true,
            };
            if behind_or_absent && prev > 0 {
                self.resend_window(&loc, mark_map.get(&loc.offset_key).copied()).await;
                self.server_marks.insert(loc.offset_key.clone(), prev);
            }
        }
        if dirty {
            self.offsets.flush();
        }
    }

    /// Re-emit the transcript from the server's mark `from` (or, without one,
    /// one bounded window behind the persisted offset) to heal a gap, then
    /// surface our offset as a mark. The persisted offset is left untouched —
    /// this is a pure catch-up replay the server dedups.
    pub(super) async fn resend_window(&self, loc: &TranscriptLocation, from: Option<u64>) {
        let off = self.offsets.get(&loc.offset_key);
        if from.is_some_and(|m| m >= off) {
            return;
        }
        let read = match from {
            Some(mark) if off - mark <= transcript::RECONCILE_BACKUP_BYTES => {
                transcript::tail_once(&loc.path, &loc.local_id, mark).map(|(events, _)| events)
            }
            _ => transcript::reconcile_tail(&loc.path, &loc.local_id, off),
        };
        match read {
            Ok(events) => {
                for evt in events {
                    self.emit(evt).await;
                }
                self.emit(AdapterEvent::TranscriptMark {
                    local_id: loc.local_id.clone(),
                    offset: off,
                })
                .await;
            }
            Err(err) => {
                tracing::debug!(%err, path = %loc.path.display(), "resume-mark re-send failed");
            }
        }
    }

    pub(super) async fn flush_roster(&mut self, reason: EndReason) {
        // The daemon/socket is gone — stop dialing it from every attach task.
        self.attach.cancel_all();
        let shorts: Vec<String> = self.roster.drain().collect();
        self.last_status.clear();
        // do NOT clear heal bookkeeping here. A flush fires when the
        // control socket is momentarily unreachable (on-demand daemon
        // idle-shutdown / kickstart race) — that is NOT evidence the workers
        // died; they stay alive and reappear on the next successful poll.
        // Forgetting their launched-with-env trust here made cctui mistake its
        // own live, account-bound sessions for env-less autonomous respawns and
        // force-kill them as soon as the socket returned (the observed kill
        // loop). Trust is dropped only when a worker genuinely leaves a
        // *successful* roster snapshot (`apply_snapshot`'s `gone` handling),
        // which a real death/respawn does and a socket blip does not.
        for short in shorts {
            self.clear_permission(&short).await;
            self.emit(AdapterEvent::SessionEnded { local_id: short, reason: reason.clone() }).await;
        }
    }

    /// Record `session_id → local_id` in the shared map the ask-hook listener
    /// reads. Lock poisoning is non-fatal here (the map is best-effort routing
    /// metadata), so we recover the guard rather than panic.
    pub(super) fn map_session(&self, session_id: &str, local_id: &str) {
        let mut guard =
            self.session_to_local.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(session_id.to_owned(), local_id.to_owned());
    }
}

/// A server resume mark bounded by the transcript's current length: a
/// mark past EOF (stale after a `/clear` truncation or rotation) must never seek
/// beyond live bytes. A missing/unreadable file yields 0 so the tail restarts
/// from the top rather than trusting the mark.
pub(super) fn clamp_to_file_len(path: &Path, mark: u64) -> u64 {
    std::fs::metadata(path).map_or(0, |m| mark.min(m.len()))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;

    #[tokio::test]
    async fn resume_mark_clamps_cursor_forward_and_skips_replay() {
        // a server mark ahead of the (cold-start empty) local offset
        // fast-forwards the tail cursor, so the bytes the server already has are
        // never re-emitted.
        let (mut d, mut rx) = driver();
        let l0 = text_line("first");
        let sess = write_main_transcript(&d, "abcd1234", &[&l0]);
        // First poll establishes the location and tails "first".
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let seen = drain_messages(&mut rx);
        assert!(seen.contains(&"first".to_owned()));
        let mark = d.offsets.get(&sess);
        assert!(mark > 0);

        // Append two more lines, then wipe the local offset to simulate a
        // daemon restart (in prod offsets are in-memory only).
        let l1 = text_line("second");
        let l2 = text_line("third");
        write_main_transcript(&d, "abcd1234", &[&l1, &l2]);
        d.offsets.set(sess.clone(), 0);

        // The server hands back its stored mark (end of "first").
        d.apply_resume_marks(vec![(sess.clone(), mark)]).await;
        assert_eq!(d.offsets.get(&sess), mark, "cursor clamps forward to the mark");

        // Next poll resumes from the mark: only the two new lines, never "first".
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let seen = drain_messages(&mut rx);
        assert!(seen.contains(&"second".to_owned()) && seen.contains(&"third".to_owned()));
        assert!(!seen.contains(&"first".to_owned()), "clamped bytes must not replay");
    }

    #[tokio::test]
    async fn absent_mark_triggers_one_bounded_resend_then_idle_is_silent() {
        // acceptance: a session we already tail with NO server mark gets
        // exactly one bounded re-send window; once the offsets agree, repeated
        // periodic passes at idle emit nothing.
        let (mut d, mut rx) = driver();
        let l0 = text_line("alpha");
        let l1 = text_line("beta");
        write_main_transcript(&d, "abcd1234", &[&l0, &l1]);
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let _ = drain_messages(&mut rx);

        // No mark for this session → one bounded window re-send (server dedups).
        d.apply_resume_marks(vec![]).await;
        let resent = drain_messages(&mut rx);
        assert!(resent.contains(&"alpha".to_owned()) && resent.contains(&"beta".to_owned()));

        // Now offsets and the recorded server mark agree: several periodic
        // reconcile passes must emit ZERO frames (the whole point of the ticket).
        for _ in 0..5 {
            d.reconcile_tail(false).await;
        }
        assert!(rx.try_recv().is_err(), "idle periodic reconcile must emit nothing");
    }

    #[tokio::test]
    async fn divergent_mark_behind_local_triggers_resend() {
        // the server's mark is BEHIND our persisted offset (a send
        // dropped before reconnect) — heal the gap with one bounded window.
        let (mut d, mut rx) = driver();
        let l0 = text_line("one");
        let l1 = text_line("two");
        let sess = write_main_transcript(&d, "abcd1234", &[&l0, &l1]);
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let _ = drain_messages(&mut rx);
        let local = d.offsets.get(&sess);
        assert!(local > 1);

        d.apply_resume_marks(vec![(sess.clone(), 1)]).await;
        let resent = drain_messages(&mut rx);
        assert!(!resent.is_empty(), "a mark behind the local offset must re-send the window");
        assert_eq!(d.offsets.get(&sess), local, "the persisted offset is never rewound");
    }

    #[tokio::test]
    async fn fully_acked_session_resends_nothing() {
        let (mut d, mut rx) = driver();
        let sess = write_main_transcript(&d, "abcd1234", &[&text_line("one"), &text_line("two")]);
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let _ = drain_messages(&mut rx);
        let local = d.offsets.get(&sess);

        d.apply_resume_marks(vec![(sess.clone(), local)]).await;
        for force in [false, true, false, true] {
            d.reconcile_tail(force).await;
        }
        assert!(rx.try_recv().is_err(), "an acked session must emit zero resend frames");
    }

    #[tokio::test]
    async fn forced_reconcile_resends_only_the_unacked_gap() {
        let (mut d, mut rx) = driver();
        let l0 = text_line("acked");
        let sess = write_main_transcript(&d, "abcd1234", &[&l0]);
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let _ = drain_messages(&mut rx);
        let mark = d.offsets.get(&sess);
        d.apply_resume_marks(vec![(sess.clone(), mark)]).await;

        write_main_transcript(&d, "abcd1234", &[&text_line("fresh")]);
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        assert_eq!(drain_messages(&mut rx), vec!["fresh".to_owned()]);

        d.reconcile_tail(true).await;
        let resent = drain_messages(&mut rx);
        assert_eq!(resent, vec!["fresh".to_owned()], "only bytes past the acked mark");
    }
}
