use crate::git::{read_git_branch, read_git_remote};

use super::{
    AdapterEvent, DispatchDoneTracker, Driver, EndReason, HashMap, HashSet, Instant, LiveSnapshot,
    PendingPerm, SessionMeta, StateJson, StatusSnapshot, TranscriptLocation, json, transcript,
};

/// What one `list` poll means against the last known roster, decided
/// before any event is emitted.
#[derive(Debug)]
struct SnapshotPlan {
    /// Every listed job cctui did not dispatch, visible or not.
    foreign_shorts: HashSet<String>,
    /// Jobs that are sessions: no spares, no dying workers.
    visible: Vec<LiveSnapshot>,
    now_shorts: HashSet<String>,
    native_live: bool,
    /// A live (not dead) visible worker reports busy.
    roster_busy: bool,
    /// Visible shorts absent from the previous roster, in listing order.
    started: Vec<String>,
    /// Known shorts no longer listed as visible.
    gone: Vec<String>,
}

fn plan_snapshot(jobs: Vec<LiveSnapshot>, roster: &HashSet<String>) -> SnapshotPlan {
    let foreign_shorts = jobs.iter().filter(|j| j.is_foreign()).map(|j| j.short.clone()).collect();
    let visible: Vec<LiveSnapshot> =
        jobs.into_iter().filter(LiveSnapshot::is_user_visible).collect();
    let native_live = visible.iter().any(LiveSnapshot::is_foreign);
    let roster_busy = visible.iter().any(|j| {
        !j.is_dead() && DispatchDoneTracker::is_busy(j.tempo.as_deref(), j.state.as_deref())
    });
    let now_shorts: HashSet<String> = visible.iter().map(|j| j.short.clone()).collect();
    let started =
        visible.iter().filter(|j| !roster.contains(&j.short)).map(|j| j.short.clone()).collect();
    let gone = roster.difference(&now_shorts).cloned().collect();
    SnapshotPlan { foreign_shorts, visible, now_shorts, native_live, roster_busy, started, gone }
}

impl Driver {
    pub(super) async fn apply_snapshot(&mut self, jobs: Vec<LiveSnapshot>) {
        let mut plan = plan_snapshot(jobs, &self.roster);
        self.foreign_shorts = std::mem::take(&mut plan.foreign_shorts);
        self.native_live = plan.native_live;
        if plan.roster_busy {
            self.version_gate.note_roster_busy();
        }

        // Ground-truth effort for every live worker, reused across the per-job
        // Status build below.
        let observed_efforts = super::super::envcheck::worker_efforts(&plan.now_shorts).await;

        for job in plan.visible.iter().filter(|j| plan.started.contains(&j.short)) {
            self.adopt_started(job).await;
        }
        for job in &plan.visible {
            self.reconcile_job(job, &observed_efforts).await;
        }
        // Done after Status updates so the UI's identity fields land
        // before the message stream they describe.
        self.tail_transcripts().await;
        for short in &plan.gone {
            self.reap_gone(short).await;
        }

        // Keep a headless `attach` open for every live session so the worker
        // stays focused/awake and `reply` actually drives its PTY. Jobs cctui
        // did not start are excluded: a held attach forces our geometry on
        // someone's terminal and drives their PTY.
        self.attach.reconcile(
            plan.now_shorts
                .iter()
                .map(String::as_str)
                .filter(|short| !self.foreign_shorts.contains(*short)),
        );

        self.tick_dispatch_done(&plan.visible);

        self.roster = plan.now_shorts;
    }

    async fn adopt_started(&mut self, job: &LiveSnapshot) {
        let session_id = job.session_id().map_or_else(|| job.short.clone(), str::to_owned);
        self.short_by_session.insert(session_id.clone(), job.short.clone());
        // If this short was just forked or spawned as a subagent,
        // carry the parent link so the server resolves it into
        // `parent_id`. Consumed once.
        let parent = self.fork_parent_by_short.lock().ok().and_then(|mut m| m.remove(&job.short));
        let (parent_local_id, relation) = match parent {
            Some((parent, relation)) => (Some(parent), relation),
            None => (None, "root"),
        };
        let on_disk = StateJson::read(&self.cfg.jobs_root, &job.short);
        let created_at = on_disk.as_ref().and_then(|s| s.created_at.clone());
        let mut extra = json!({
            "short": job.short,
            "relation": relation,
        });
        // The server merges metadata with jsonb `||`, where an explicit
        // null overwrites. Omit what this poll could not read so a
        // rediscovery never erases what an earlier one published.
        for (key, value) in [
            ("cli_version", job.cli_version.clone()),
            ("created_at", created_at),
            ("git_branch", job.cwd.as_deref().and_then(read_git_branch)),
            ("git_remote", job.cwd.as_deref().and_then(read_git_remote)),
        ] {
            if let Some(value) = value {
                extra[key] = json!(value);
            }
        }
        self.emit(AdapterEvent::SessionStarted {
            local_id: session_id,
            meta: SessionMeta { working_dir: job.cwd.clone(), parent_local_id, extra },
        })
        .await;
    }

    /// Status update for one listed job: live snapshot + on-disk state.json
    /// reconciliation, dead-while-listed detection and transcript pinning.
    async fn reconcile_job(
        &mut self,
        job: &LiveSnapshot,
        observed_efforts: &HashMap<String, String>,
    ) {
        // The emitted `local_id` is STABLE for a worker's whole life. Once a
        // transcript is pinned we keep reusing its `local_id` even when the
        // session id rotates in place (`/clear`, `/compact`), so every
        // message lands in the one session the server already knows. Only
        // the very first pin derives the id from the live `session_id`.
        let local_id = self
            .transcript_locations
            .get(&job.short)
            .map(|loc| loc.local_id.clone())
            .or_else(|| job.session_id().map(str::to_owned))
            .unwrap_or_else(|| job.short.clone());
        let on_disk = StateJson::read(&self.cfg.jobs_root, &job.short);

        // Observation timestamp for the diagnose report: when
        // this short was last seen on the control socket.
        self.last_status_at.insert(job.short.clone(), std::time::SystemTime::now());

        if job.is_dead() {
            self.mark_dead(job, &local_id, on_disk.is_some()).await;
            return;
        }
        // Revived: claude reports this short alive again after we marked it
        // dead — clear the sticky flag so live status flows again.
        self.dead_shorts.remove(&job.short);

        // Gateway-env delivery is handled entirely at the launch chokepoint
        // (`resolve_launch_env`): the resolved env rides the per-session
        // `--settings` file + `reattachEnv`, both of which the claude daemon
        // re-applies on its own autonomous respawns (`/clear`, `/compact`,
        // spare-claim), so a revived worker keeps its routing without cctui
        // killing it. A genuinely env-less launch fails LOUD in
        // `launch_env_decision`.

        // Surface (or clear) a tool-permission prompt from the live
        // `tempo`/`needs` signal, before the Status emit below.
        self.reconcile_permission(
            &job.short,
            &local_id,
            job.tempo.as_deref(),
            job.needs.as_deref(),
        )
        .await;

        self.pin_transcript(job, &local_id, on_disk.as_ref()).await;
        self.emit_status(job, local_id, on_disk.as_ref(), observed_efforts).await;
    }

    /// Dead-but-still-listed: claude can keep a session in `daemon list`
    /// while its worker process is gone (e.g. it died while the supervisor was
    /// down). Surface the dead state within one poll rather than waiting for
    /// the short to drop off the roster.
    async fn mark_dead(&mut self, job: &LiveSnapshot, local_id: &str, state_on_disk: bool) {
        // Emit the terminal transition exactly once, then mark the
        // short sticky so the still-present roster entry can't re-emit
        // a non-terminal Status and re-green it (daemon-side mirror of
        // the server's sticky terminal status). Mirrors the
        // roster-disappearance path: hibernated if job state survives
        // on disk (revivable red dot), else SessionEnded.
        //
        // Sticky: the caller skips transcript re-pin + Status for this poll.
        // The roster-disappearance branch still cleans up if it later drops
        // off; a revive clears `dead_shorts` so live status resumes.
        if !self.dead_shorts.insert(job.short.clone()) {
            return;
        }
        self.clear_permission(&job.short).await;
        if state_on_disk {
            self.emit(AdapterEvent::Status {
                local_id: local_id.to_owned(),
                tempo: Some("hibernated".to_owned()),
                state: None,
                detail: None,
                activity: None,
                name: None,
                intent: None,
                model: None,
                effort: None,
                permission_mode: None,
                children: Vec::new(),
            })
            .await;
        } else {
            self.emit(AdapterEvent::SessionEnded {
                local_id: local_id.to_owned(),
                reason: EndReason::Completed,
            })
            .await;
            // Truly gone — no job state left to cold resume from, so
            // the spawn flags and per-session config files are dead.
            if let Ok(mut m) = self.spawn_model_effort.lock() {
                m.remove(&job.short);
            }
            crate::configsweep::remove_session_files(&job.short);
        }
        // Drop the cached status so a later revive (worker reports
        // alive again) is detected as a change and re-emitted.
        self.last_status.remove(&job.short);
    }

    /// Pin (or re-pin) the transcript location. A resume or an in-process
    /// reset (`/clear`, `/compact`) changes the session's `sessionId` and
    /// starts a NEW transcript file (`<newId>.jsonl`); if we kept tailing
    /// the original file the message stream would silently stop while
    /// `list`/Status polls kept the heartbeat fresh. So re-pin whenever the
    /// live `session_id` differs from the one we cached, following the
    /// transcript to the new file.
    ///
    /// A reset keeps the same worker `short`, so the "newly started" path
    /// never fires for the new id. We deliberately keep emitting under the
    /// ORIGINAL `local_id` (set on the first pin, kept in `loc.local_id`) and
    /// only move `path`/`offset_key` to the new file — so the post-reset
    /// transcript appends to the one session the server already knows.
    /// Splitting it into a second session would be worse: archive is
    /// worker-scoped (`claude rm <short>`), so a single archive would wipe
    /// both conversations at once. Instead we inject a `context_reset`
    /// boundary marker so the cut is visible in the UI.
    async fn pin_transcript(
        &mut self,
        job: &LiveSnapshot,
        local_id: &str,
        on_disk: Option<&StateJson>,
    ) {
        // `/clear` rotates the live session into a new transcript
        // file but the control socket's `list` keeps reporting the stale
        // spawn `sessionId` (it's the immutable `--session-id` launch arg in
        // `roster.json`). The rotated id only surfaces in `state.json`'s
        // `resumeSessionId`, so prefer that; fall back to the snapshot id
        // when no reset has happened. Without this the rotation check below
        // never fires for `/clear` and the message stream silently stops.
        let live_session =
            on_disk.and_then(|s| s.resume_session_id.as_deref()).or_else(|| job.session_id());
        let (Some(cwd), Some(sess)) = (job.cwd.as_deref(), live_session) else {
            return;
        };
        let rotated =
            self.transcript_locations.get(&job.short).is_some_and(|loc| loc.offset_key != sess);
        let first_pin = !self.transcript_locations.contains_key(&job.short);
        let path = self.resolve_live_transcript(cwd, sess);
        let moved = !first_pin
            && !rotated
            && self.transcript_locations.get(&job.short).is_some_and(|loc| loc.path != path);
        if moved {
            // Same session id, new file: the session entered/left a git
            // worktree so claude relocated the transcript. The move is
            // content-continuous, so keep offset_key + offset and the
            // stable local_id — only follow the path.
            if let Some(loc) = self.transcript_locations.get_mut(&job.short) {
                tracing::info!(
                    short = %job.short,
                    from = %loc.path.display(),
                    to = %path.display(),
                    "transcript moved (worktree enter/exit); following"
                );
                loc.path.clone_from(&path);
            }
        }
        if first_pin {
            self.short_by_session.insert(sess.to_owned(), job.short.clone());
            self.map_session(sess, local_id);
            self.transcript_locations.insert(
                job.short.clone(),
                TranscriptLocation {
                    path,
                    local_id: local_id.to_owned(),
                    cwd: cwd.to_owned(),
                    offset_key: sess.to_owned(),
                },
            );
        } else if rotated {
            // Follow the file, keep the stable `local_id`. The new
            // `sess` is mapped to the same `short` too so command
            // dispatch keeps working if a snapshot ever reports the new
            // id directly. The rotated id maps to the unchanged stable
            // `local_id` so a hook firing post-`/clear` still resolves
            // to the session the server knows.
            self.short_by_session.insert(sess.to_owned(), job.short.clone());
            self.map_session(sess, local_id);
            if let Some(loc) = self.transcript_locations.get_mut(&job.short) {
                loc.path = path;
                sess.clone_into(&mut loc.offset_key);
            }
            self.emit(AdapterEvent::Message {
                local_id: local_id.to_owned(),
                payload: json!({
                    "role": "context_reset",
                    "text": "context reset (/clear · /compact)",
                    // The new session id keys this marker uniquely so a
                    // second reset isn't collapsed by the server's
                    // content-hash dedup (identical text would hash the
                    // same).
                    "session_id": sess,
                }),
                turn_id: None,
            })
            .await;
        }
    }

    async fn emit_status(
        &mut self,
        job: &LiveSnapshot,
        local_id: String,
        on_disk: Option<&StateJson>,
        observed_efforts: &HashMap<String, String>,
    ) {
        let name = on_disk.and_then(|s| s.name.clone()).or_else(|| job.name.clone());
        let intent = on_disk.and_then(|s| s.intent.clone()).or_else(|| job.intent.clone());
        let activity = on_disk.and_then(|s| s.activity.clone());
        // Prefer the on-disk state.json; fall back to the spawn-time flags
        // we remembered while state.json is absent/transient.
        let spawned = self.spawn_model_effort.lock().ok().and_then(|m| m.get(&job.short).cloned());
        let model = on_disk
            .and_then(|s| s.model.clone())
            .or_else(|| spawned.as_ref().and_then(|(m, _)| m.clone()));
        // Prefer the GROUND-TRUTH effort the live worker actually booted at
        // (read from its `CLAUDE_EFFORT` env), so the UI shows what the
        // session is running rather than what we requested — a spare-claim or
        // a silent background clamp can make them differ. Fall back
        // to the requested value (state.json flags, then the spawn cache)
        // while the worker is mid-exec / not yet found in `/proc`.
        let effort = observed_efforts
            .get(&job.short)
            .cloned()
            .or_else(|| on_disk.and_then(|s| s.effort.clone()))
            .or_else(|| spawned.as_ref().and_then(|(_, e)| e.clone()));
        let children = on_disk.map(StateJson::proto_children).unwrap_or_default();

        // NB: live `AskUserQuestion` surfacing is NOT derived from status
        // here. Real questions report `state:"done"`, not `blocked`, and a
        // `blocked` state is a background status (e.g. "needs input"), not a
        // question. The `AskUserQuestion` PreToolUse hook delivers the real
        // prompt over the daemon socket.

        let snap = StatusSnapshot {
            tempo: job.tempo.clone(),
            state: job.state.clone(),
            detail: job.detail.clone(),
            name: name.clone(),
            activity: activity.clone(),
            model: model.clone(),
            effort: effort.clone(),
        };
        if self.last_status.get(&job.short) == Some(&snap) {
            return;
        }
        self.last_status.insert(job.short.clone(), snap);
        self.emit(AdapterEvent::Status {
            local_id,
            tempo: job.tempo.clone(),
            state: job.state.clone(),
            detail: job.detail.clone(),
            activity,
            name,
            intent,
            model,
            effort,
            permission_mode: None,
            children,
        })
        .await;
    }

    /// Tail transcripts for every pinned session (and their subagents) and
    /// emit new events.
    async fn tail_transcripts(&mut self) {
        let mut dirty_offsets = false;
        let locations: Vec<TranscriptLocation> =
            self.transcript_locations.values().cloned().collect();
        for loc in locations {
            let prev = self.offsets.get(&loc.offset_key);
            let off = self.resume_offset(&loc.offset_key, &loc.path);
            if off != prev {
                dirty_offsets = true;
            }
            match transcript::tail_once(&loc.path, &loc.local_id, off) {
                Ok((events, new_off)) => {
                    if new_off != off {
                        self.offsets.set(loc.offset_key.clone(), new_off);
                        self.server_marks.insert(loc.offset_key.clone(), new_off);
                        dirty_offsets = true;
                    }
                    if let Some(last) = events.last() {
                        // "Last parsed event" for diagnose.
                        self.last_parsed.insert(
                            loc.offset_key.clone(),
                            (
                                super::super::diagnose::event_kind(last).to_owned(),
                                std::time::SystemTime::now(),
                            ),
                        );
                    }
                    for evt in events {
                        self.emit(evt).await;
                    }
                    if new_off != off {
                        self.emit(AdapterEvent::TranscriptMark {
                            local_id: loc.local_id.clone(),
                            offset: new_off,
                        })
                        .await;
                    }
                }
                Err(err) => {
                    tracing::debug!(%err, path = %loc.path.display(), "transcript tail failed");
                }
            }
        }
        // Discover + tail Task-tool subagents nested under each live parent.
        // Runs after the parent tail so a subagent's parent row
        // exists before its own SessionStarted references it.
        self.scan_subagents(&mut dirty_offsets).await;

        if dirty_offsets {
            self.offsets.flush();
        }
    }

    async fn reap_gone(&mut self, short: &str) {
        self.last_status.remove(short);
        let was_dead = self.dead_shorts.remove(short);
        self.clear_permission(short).await;
        if let Some(loc) = self.transcript_locations.remove(short) {
            // Hibernated, not gone: the worker process exited but
            // its job state survives on disk, so a reply will revive it
            // (resume-on-reply above). Mark the session so the UI can show
            // the claude-style "exited, will resume on reply" red dot
            // instead of a plain dead one. Carried in `tempo` (not
            // `agent_state`) so the bucket classifier still sees the final
            // state (`done` → Completed); a revived worker's next live
            // snapshot overwrites it.
            //
            // Skip if we already emitted this short's dead transition while
            // it was still listed (`dead_shorts`) — the hibernated
            // Status already went out; re-emitting it here is redundant.
            if !was_dead && StateJson::read(&self.cfg.jobs_root, short).is_some() {
                self.emit(AdapterEvent::Status {
                    local_id: loc.local_id.clone(),
                    tempo: Some("hibernated".to_owned()),
                    state: None,
                    detail: None,
                    activity: None,
                    name: None,
                    intent: None,
                    model: None,
                    effort: None,
                    permission_mode: None,
                    children: Vec::new(),
                })
                .await;
            }
            self.short_by_session.remove(&loc.local_id);
            self.session_to_local
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|_, v| v != &loc.local_id);
            self.end_subagents_of(&loc.local_id).await;
        }
        // We don't retain the session_id mapping after roster removal,
        // so fall back to the short as the local_id. Sessions on the
        // server side are indexed by (machine_id, adapter_id,
        // local_id), and the prior SessionStarted carried the real
        // session_id; the server reconciles on the running row.
        self.emit(AdapterEvent::SessionEnded {
            local_id: short.to_owned(),
            reason: EndReason::Completed,
        })
        .await;
    }

    /// Feed the dispatch turn-complete watcher one roster snapshot
    /// and write the `dispatch_done` marker when it fires. No-op on normal
    /// daemons (`dispatch_done` is only armed by `maybe_dispatch_on_start`).
    pub(super) fn tick_dispatch_done(&self, jobs: &[LiveSnapshot]) {
        let Ok(mut guard) = self.dispatch_done.lock() else { return };
        let Some(tracker) = guard.as_mut() else { return };
        let job = jobs.iter().find(|j| j.short == tracker.short());
        // Absent from the roster before it ever ran isn't idleness — it's a
        // cold start still booting (the entrypoint's boot deadline bounds
        // that). Once seen busy, absence (session ended/retired) does count
        // toward settle.
        if job.is_none() && !tracker.seen_busy() {
            return;
        }
        // A growing transcript is authoritative liveness: the control-socket
        // snapshot can read idle while a worktree-entered session is still
        // working, so never let the settle clock run purely on that signal.
        let transcript_offset = self
            .transcript_locations
            .get(tracker.short())
            .map_or(0, |loc| self.offsets.get(&loc.offset_key));
        let grew = tracker.transcript_grew(transcript_offset);
        let busy = grew
            || job.is_some_and(|j| {
                !j.is_dead() && DispatchDoneTracker::is_busy(j.tempo.as_deref(), j.state.as_deref())
            });
        if grew {
            self.version_gate.note_roster_busy();
        }
        if tracker.observe(busy, Instant::now()) {
            let path = tracker.marker_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(path, b"") {
                Ok(()) => {
                    tracing::info!(
                        short = %tracker.short(),
                        path = %path.display(),
                        "dispatched session settled idle after work; wrote dispatch_done marker"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        %err,
                        path = %path.display(),
                        "failed to write dispatch_done marker"
                    );
                }
            }
        }
    }

    /// Reconcile the pending tool-permission prompt for one worker against the
    /// live snapshot. A `needs` of `"approve <Tool>: <detail>"` (set
    /// while the worker is `tempo:"blocked"`) is a permission prompt; a fresh or
    /// changed one emits `PermissionRequest`, and clearing it emits
    /// `PermissionResolved`. Deduped so an unchanged prompt isn't re-emitted on
    /// every 2s poll.
    pub(super) async fn reconcile_permission(
        &mut self,
        short: &str,
        local_id: &str,
        tempo: Option<&str>,
        needs: Option<&str>,
    ) {
        let pending_needs = match needs.map(str::trim) {
            // `tempo:"blocked"` + an `approve …` need is the interactive
            // tool-permission prompt. Other `needs`/blocked states (e.g. a
            // background "needs input") are not permission prompts.
            Some(n) if tempo == Some("blocked") && n.starts_with("approve ") => Some(n.to_owned()),
            _ => None,
        };

        match pending_needs {
            Some(n) => {
                // Already surfaced this exact prompt? Nothing to do.
                if self.pending_perms.get(short).is_some_and(|p| p.needs == n) {
                    return;
                }
                // A changed `needs` means the prior prompt was superseded —
                // resolve it before emitting the new one so no stale card lingers.
                if let Some(prev) = self.pending_perms.remove(short) {
                    self.emit(AdapterEvent::PermissionResolved {
                        local_id: prev.local_id,
                        request_id: prev.request_id,
                    })
                    .await;
                }
                self.perm_seq += 1;
                let request_id = format!("{short}#perm{}", self.perm_seq);
                let (tool, description) = parse_permission_needs(&n);
                self.pending_perms.insert(
                    short.to_owned(),
                    PendingPerm {
                        request_id: request_id.clone(),
                        local_id: local_id.to_owned(),
                        needs: n.clone(),
                    },
                );
                self.emit(AdapterEvent::PermissionRequest {
                    local_id: local_id.to_owned(),
                    request_id,
                    tool,
                    input: json!({ "description": description, "needs": n }),
                })
                .await;
            }
            None => {
                if let Some(prev) = self.pending_perms.remove(short) {
                    self.emit(AdapterEvent::PermissionResolved {
                        local_id: prev.local_id,
                        request_id: prev.request_id,
                    })
                    .await;
                }
            }
        }
    }

    /// Drop any pending permission for a worker that left the roster, emitting a
    /// `PermissionResolved` so clients dismiss a prompt whose session is gone.
    pub(super) async fn clear_permission(&mut self, short: &str) {
        if let Some(prev) = self.pending_perms.remove(short) {
            self.emit(AdapterEvent::PermissionResolved {
                local_id: prev.local_id,
                request_id: prev.request_id,
            })
            .await;
        }
    }
}

/// Parse a permission `needs` string (`"approve <Tool>: <detail>"`) into a
/// `(tool, description)` pair. Falls back to the whole remainder as both tool
/// and description when there's no `": "` separator.
pub(super) fn parse_permission_needs(needs: &str) -> (String, String) {
    let rest = needs.strip_prefix("approve ").unwrap_or(needs).trim();
    match rest.split_once(": ") {
        Some((tool, detail)) => (tool.trim().to_owned(), detail.trim().to_owned()),
        None => (rest.to_owned(), rest.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::SPARE_SOURCE;
    use super::super::test_support::*;
    use super::*;

    #[tokio::test]
    async fn foreign_jobs_are_registered_but_never_attached() {
        let (mut d, mut rx) = driver();
        let mut human = snap("beefbeef", "working", None);
        human.source = Some("bg".into());
        d.apply_snapshot(vec![human, snap("f1eetf1e", "working", None)]).await;

        assert!(d.roster.contains("f1eetf1e"));
        assert!(d.roster.contains("beefbeef"), "a human's own claude job must be registered");
        assert!(d.foreign_shorts.contains("beefbeef"));
        assert!(!d.foreign_shorts.contains("f1eetf1e"));

        let mut started: Vec<String> = Vec::new();
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::SessionStarted { local_id, .. } = evt {
                started.push(local_id);
            }
        }
        started.sort();
        assert_eq!(started, ["beefbeef-uuid", "f1eetf1e-uuid"]);

        // `reconcile` holds an attach only for the fleet short.
        assert!(d.attach.snapshot("f1eetf1e").is_some());
        assert!(
            d.attach.snapshot("beefbeef").is_none(),
            "no attach may be held on a job cctui did not start"
        );
    }

    #[tokio::test]
    async fn a_foreign_job_that_leaves_the_roster_ends_as_completed() {
        let (mut d, mut rx) = driver();
        let mut human = snap("beefbeef", "working", None);
        human.source = Some("bg".into());
        d.apply_snapshot(vec![human]).await;
        while rx.try_recv().is_ok() {}

        d.apply_snapshot(vec![]).await;
        let mut ends: Vec<EndReason> = Vec::new();
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::SessionEnded { reason, .. } = evt {
                ends.push(reason);
            }
        }
        assert_eq!(ends, vec![EndReason::Completed]);
    }

    #[tokio::test]
    async fn snapshot_emits_started_and_status() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionStarted { .. }));
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::Status { .. }));
    }

    #[tokio::test]
    async fn snapshot_filters_spare_and_dying() {
        let (mut d, mut rx) = driver();
        let mut spare = snap("11111111", "working", None);
        spare.source = Some("spare".into());
        let mut dying = snap("22222222", "working", None);
        dying.dying = true;
        d.apply_snapshot(vec![spare, dying]).await;
        assert!(rx.try_recv().is_err(), "filtered jobs should emit nothing");
    }

    #[tokio::test]
    async fn snapshot_emits_ended_when_short_disappears() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("aaaa0001", "working", None)]).await;
        // Drain Started + Status.
        rx.recv().await.unwrap();
        rx.recv().await.unwrap();
        d.apply_snapshot(vec![]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionEnded { .. }));
    }

    #[tokio::test]
    async fn dead_in_roster_emits_hibernated_with_state_json() {
        // B2: a still-listed session that claude reports dead emits a
        // hibernated Status (state.json survives → revivable red dot) within
        // one poll, without waiting for roster disappearance.
        let (mut d, mut rx) = driver();
        // Start it live.
        d.apply_snapshot(vec![snap("aaaa0001", "working", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));

        // Persist on-disk job state so the dead transition picks hibernated.
        let short_dir = d.cfg.jobs_root.join("aaaa0001");
        std::fs::create_dir_all(&short_dir).unwrap();
        std::fs::write(
            short_dir.join("state.json"),
            r#"{"sessionId":"sess-a","cwd":"/tmp","state":"working"}"#,
        )
        .unwrap();

        // Same short, now reported dead but STILL listed.
        let mut dead = snap("aaaa0001", "working", None);
        dead.gone = true;
        d.apply_snapshot(vec![dead]).await;
        let evt = rx.recv().await.unwrap();
        match evt {
            AdapterEvent::Status { tempo, .. } => {
                assert_eq!(tempo.as_deref(), Some("hibernated"));
            }
            other => panic!("expected hibernated Status, got {other:?}"),
        }

        // B3 sticky: a second poll still reporting dead must NOT re-emit
        // (no Status, no SessionEnded) — the dot can't be re-greened.
        let mut dead2 = snap("aaaa0001", "working", None);
        dead2.gone = true;
        d.apply_snapshot(vec![dead2]).await;
        assert!(rx.try_recv().is_err(), "dead-in-roster is sticky: no re-emit");
    }

    #[tokio::test]
    async fn dead_in_roster_emits_ended_without_state_json() {
        // B2: dead-but-listed with no surviving job state → SessionEnded
        // (the server marks the row `ended`, which is sticky).
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("bbbb0002", "working", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));

        let mut dead = snap("bbbb0002", "working", None);
        dead.status = Some("exited".into());
        d.apply_snapshot(vec![dead]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionEnded { .. }));
    }

    #[tokio::test]
    async fn revive_clears_dead_sticky_and_resumes_status() {
        // if claude reports the short alive again after we marked it
        // dead, the sticky flag clears and live Status flows once more.
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("cccc0003", "working", None)]).await;
        rx.recv().await.unwrap(); // Started
        rx.recv().await.unwrap(); // Status

        let mut dead = snap("cccc0003", "working", None);
        dead.dead = true;
        d.apply_snapshot(vec![dead]).await;
        // SessionEnded (no state.json).
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionEnded { .. }));

        // Revived: alive again → a fresh Status is emitted.
        d.apply_snapshot(vec![snap("cccc0003", "busy", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));
    }

    #[tokio::test]
    async fn transcript_repins_when_session_id_changes_on_reset() {
        // An in-process reset (`/clear`, `/compact`) or a
        // resume keeps the same `short` but gets a new `sessionId` (and a new
        // transcript file). We must follow the file to the new id, but keep
        // emitting under the ORIGINAL `local_id` so the post-reset transcript
        // appends to the one session the server already knows (splitting it
        // would let a worker-scoped `claude rm` archive wipe both at once).
        let (mut d, _rx) = driver();
        let mut s1 = snap("deadbeef", "working", None);
        s1.session_id = Some("sess-1".into());
        d.apply_snapshot(vec![s1]).await;
        let loc1 = d.transcript_locations.get("deadbeef").expect("pinned");
        assert_eq!(loc1.offset_key, "sess-1");
        assert_eq!(loc1.local_id, "sess-1");
        let path1 = loc1.path.clone();
        assert_eq!(d.short_by_session.get("sess-1").map(String::as_str), Some("deadbeef"));

        let mut s2 = snap("deadbeef", "working", None);
        s2.session_id = Some("sess-2".into());
        d.apply_snapshot(vec![s2]).await;
        let loc2 = d.transcript_locations.get("deadbeef").expect("re-pinned");
        assert_eq!(loc2.offset_key, "sess-2", "should follow the reset transcript");
        assert_ne!(loc2.path, path1, "transcript path should move to the new session id");
        assert_eq!(loc2.local_id, "sess-1", "local_id stays stable across the reset");
        // Both ids resolve to the worker for command dispatch.
        assert_eq!(d.short_by_session.get("sess-2").map(String::as_str), Some("deadbeef"));
        assert_eq!(d.short_by_session.get("sess-1").map(String::as_str), Some("deadbeef"));
    }

    #[tokio::test]
    async fn reset_emits_boundary_marker_under_original_session() {
        // a reset must not start/end a session — it injects a single
        // `context_reset` marker under the original `local_id` so the cut is
        // visible while the stream stays in one session.
        let (mut d, mut rx) = driver();
        let mut s1 = snap("deadbeef", "working", None);
        s1.session_id = Some("sess-1".into());
        d.apply_snapshot(vec![s1]).await;
        // Drain the first SessionStarted + Status.
        while rx.try_recv().is_ok() {}

        let mut s2 = snap("deadbeef", "working", None);
        s2.session_id = Some("sess-2".into());
        d.apply_snapshot(vec![s2]).await;

        let mut marker: Option<serde_json::Value> = None;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { .. } | AdapterEvent::SessionEnded { .. } => {
                    panic!("a reset must not start or end a session");
                }
                AdapterEvent::Message { local_id, payload, .. }
                    if payload.get("role").and_then(|r| r.as_str()) == Some("context_reset") =>
                {
                    assert_eq!(local_id, "sess-1", "marker rides the original session");
                    marker = Some(payload);
                }
                _ => {}
            }
        }
        let payload = marker.expect("a context_reset marker should be emitted");
        // The new session id keys the marker so a second reset isn't deduped.
        assert_eq!(payload.get("session_id").and_then(|s| s.as_str()), Some("sess-2"));
    }

    #[tokio::test]
    async fn blocked_state_does_not_emit_phantom_ask_question() {
        // Status never drives AskQuestion; the real prompt arrives via the
        // PreToolUse hook. A blocked snapshot must emit Status, never
        // AskQuestion.
        let (mut d, mut rx) = driver();
        let mut blocked = snap("abcd1234", "blocked", None);
        blocked.detail = Some("needs go-ahead to build & ship".into());
        d.apply_snapshot(vec![blocked]).await;

        while let Ok(evt) = rx.try_recv() {
            assert!(
                !matches!(evt, AdapterEvent::AskQuestion { .. } | AdapterEvent::AskResolved { .. }),
                "blocked status must not synthesize an Ask event"
            );
        }
    }

    #[test]
    fn parse_permission_needs_splits_tool_and_detail() {
        assert_eq!(
            parse_permission_needs("approve Bash: touch /tmp/x"),
            ("Bash".to_owned(), "touch /tmp/x".to_owned())
        );
        // No "approve " prefix, no separator → whole string as both.
        assert_eq!(parse_permission_needs("Edit"), ("Edit".to_owned(), "Edit".to_owned()));
        // Prefix but no separator.
        assert_eq!(
            parse_permission_needs("approve WebFetch"),
            ("WebFetch".to_owned(), "WebFetch".to_owned())
        );
    }

    #[tokio::test]
    async fn blocked_approve_emits_permission_request_then_resolves() {
        // a `tempo:"blocked"` snapshot whose `needs` reads
        // "approve <Tool>: <detail>" surfaces a PermissionRequest; clearing the
        // block (next poll) emits PermissionResolved exactly once.
        let (mut d, mut rx) = driver();
        let mut blocked = snap("abcd1234", "running", None);
        blocked.tempo = Some("blocked".into());
        blocked.needs = Some("approve Bash: touch /tmp/x".into());
        d.apply_snapshot(vec![blocked]).await;

        let mut request: Option<(String, String, String)> = None;
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::PermissionRequest { local_id, request_id, tool, input } = evt {
                assert_eq!(tool, "Bash");
                assert_eq!(input.get("description").and_then(|d| d.as_str()), Some("touch /tmp/x"));
                request = Some((local_id, request_id, tool));
            }
        }
        let (_, request_id, _) = request.expect("PermissionRequest expected for blocked+approve");

        // Re-poll while still blocked on the SAME prompt: no duplicate emit.
        let mut still = snap("abcd1234", "running", None);
        still.tempo = Some("blocked".into());
        still.needs = Some("approve Bash: touch /tmp/x".into());
        d.apply_snapshot(vec![still]).await;
        while let Ok(evt) = rx.try_recv() {
            assert!(
                !matches!(evt, AdapterEvent::PermissionRequest { .. }),
                "an unchanged prompt must not re-emit PermissionRequest"
            );
        }

        // Prompt clears (answered / tempo back to active) → resolve once.
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let mut resolved = 0;
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::PermissionResolved { request_id: rid, .. } = evt {
                assert_eq!(rid, request_id, "resolved id matches the emitted request");
                resolved += 1;
            }
        }
        assert_eq!(resolved, 1, "clearing the prompt resolves it exactly once");
    }

    #[tokio::test]
    async fn permission_resolved_when_session_ends_while_blocked() {
        // A worker that disappears mid-prompt must not leave a stale card.
        let (mut d, mut rx) = driver();
        let mut blocked = snap("abcd1234", "running", None);
        blocked.tempo = Some("blocked".into());
        blocked.needs = Some("approve Bash: rm -rf /tmp/x".into());
        d.apply_snapshot(vec![blocked]).await;
        while rx.try_recv().is_ok() {}
        d.apply_snapshot(vec![]).await;
        let mut resolved = false;
        while let Ok(evt) = rx.try_recv() {
            if matches!(evt, AdapterEvent::PermissionResolved { .. }) {
                resolved = true;
            }
        }
        assert!(resolved, "a vanished blocked session emits PermissionResolved");
    }

    #[tokio::test]
    async fn pinning_records_session_to_local_map() {
        // the ask-hook listener resolves a hook's live `session_id`
        // through this map. First pin maps the id to itself; a `/clear`
        // rotation maps the NEW id to the stable original `local_id`.
        let (mut d, _rx) = driver();
        let mut s1 = snap("deadbeef", "working", None);
        s1.session_id = Some("sess-1".into());
        d.apply_snapshot(vec![s1]).await;
        assert_eq!(
            d.session_map().lock().unwrap().get("sess-1").map(String::as_str),
            Some("sess-1")
        );

        let mut s2 = snap("deadbeef", "working", None);
        s2.session_id = Some("sess-2".into());
        d.apply_snapshot(vec![s2]).await;
        assert_eq!(
            d.session_map().lock().unwrap().get("sess-2").map(String::as_str),
            Some("sess-1"),
            "rotated id resolves to the stable local_id"
        );
    }

    #[tokio::test]
    async fn status_dedup_skips_unchanged_polls() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("c0ffee00", "working", Some("ours"))]).await;
        rx.recv().await.unwrap(); // started
        rx.recv().await.unwrap(); // status
        d.apply_snapshot(vec![snap("c0ffee00", "working", Some("ours"))]).await;
        assert!(rx.try_recv().is_err(), "identical poll should emit nothing");
    }

    fn job(short: &str, edit: impl FnOnce(&mut LiveSnapshot)) -> LiveSnapshot {
        let mut j = snap(short, "done", None);
        j.tempo = Some("idle".into());
        edit(&mut j);
        j
    }

    fn set(shorts: &[&str]) -> HashSet<String> {
        shorts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn plan_snapshot_diffs_the_listing_against_the_roster() {
        struct Case {
            name: &'static str,
            roster: &'static [&'static str],
            jobs: Vec<LiveSnapshot>,
            visible: &'static [&'static str],
            foreign: &'static [&'static str],
            native_live: bool,
            roster_busy: bool,
            started: &'static [&'static str],
            gone: &'static [&'static str],
        }
        let cases = [
            Case {
                name: "first poll adopts every fleet job in listing order",
                roster: &[],
                jobs: vec![job("bbbb0002", |_| {}), job("aaaa0001", |_| {})],
                visible: &["bbbb0002", "aaaa0001"],
                foreign: &[],
                native_live: false,
                roster_busy: false,
                started: &["bbbb0002", "aaaa0001"],
                gone: &[],
            },
            Case {
                name: "spares and dying workers are not sessions",
                roster: &[],
                jobs: vec![
                    job("5a5a0001", |j| j.source = Some(SPARE_SOURCE.into())),
                    job("d1e00001", |j| j.dying = true),
                    job("cccc0003", |_| {}),
                ],
                visible: &["cccc0003"],
                foreign: &["5a5a0001"],
                native_live: false,
                roster_busy: false,
                started: &["cccc0003"],
                gone: &[],
            },
            Case {
                name: "a listed foreign job marks native activity",
                roster: &["eeee0005"],
                jobs: vec![job("eeee0005", |j| j.source = Some("cli".into()))],
                visible: &["eeee0005"],
                foreign: &["eeee0005"],
                native_live: true,
                roster_busy: false,
                started: &[],
                gone: &[],
            },
            Case {
                name: "known shorts missing from the listing are gone",
                roster: &["aaaa0001", "ffff0006"],
                jobs: vec![job("aaaa0001", |_| {})],
                visible: &["aaaa0001"],
                foreign: &[],
                native_live: false,
                roster_busy: false,
                started: &[],
                gone: &["ffff0006"],
            },
            Case {
                name: "a busy live worker marks the roster busy",
                roster: &["aaaa0001"],
                jobs: vec![job("aaaa0001", |j| j.tempo = Some("active".into()))],
                visible: &["aaaa0001"],
                foreign: &[],
                native_live: false,
                roster_busy: true,
                started: &[],
                gone: &[],
            },
            Case {
                name: "a dead worker never counts as busy",
                roster: &["aaaa0001"],
                jobs: vec![job("aaaa0001", |j| {
                    j.state = Some("working".into());
                    j.gone = true;
                })],
                visible: &["aaaa0001"],
                foreign: &[],
                native_live: false,
                roster_busy: false,
                started: &[],
                gone: &[],
            },
        ];
        for case in cases {
            let plan = plan_snapshot(case.jobs, &set(case.roster));
            let visible: Vec<&str> = plan.visible.iter().map(|j| j.short.as_str()).collect();
            assert_eq!(visible, case.visible, "{}: visible", case.name);
            assert_eq!(plan.now_shorts, set(case.visible), "{}: now_shorts", case.name);
            assert_eq!(plan.foreign_shorts, set(case.foreign), "{}: foreign", case.name);
            assert_eq!(plan.native_live, case.native_live, "{}: native_live", case.name);
            assert_eq!(plan.roster_busy, case.roster_busy, "{}: roster_busy", case.name);
            assert_eq!(plan.started, case.started, "{}: started", case.name);
            let mut gone = plan.gone;
            gone.sort();
            assert_eq!(gone, case.gone, "{}: gone", case.name);
        }
    }
}
