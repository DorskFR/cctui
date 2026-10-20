use super::{AdapterEvent, Driver, json, socket, transcript};

impl Driver {
    /// Repair only the idle worker named by a recovery request. Resolve its
    /// durable gateway binding BEFORE stopping it, then resume the same saved
    /// conversation. Never delete job metadata or emit a terminal session event.
    pub(super) async fn recover_gateway_auth(
        &self,
        sock: &std::path::Path,
        local_id: &str,
        hint: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let short = self.resolve_short_for_removal(local_id)?;
        anyhow::ensure!(
            self.auth_recovery_quiescent(&short),
            "auth recovery refused: worker is busy"
        );
        let st = super::StateJson::read(&self.cfg.jobs_root, &short)
            .ok_or_else(|| anyhow::anyhow!("auth recovery: missing saved job identity"))?;
        let cwd = st.cwd.as_deref().ok_or_else(|| anyhow::anyhow!("auth recovery: missing cwd"))?;
        let session_id = st
            .resume_session_id
            .as_deref()
            .or(st.session_id.as_deref())
            .ok_or_else(|| anyhow::anyhow!("auth recovery: missing transcript identity"))?;
        // Resolved once: both tail checks must read the same file.
        let path = self.recovery_transcript_path(&short, cwd, session_id);
        anyhow::ensure!(
            tail_has_gateway_auth_error(&path)?,
            "auth recovery refused: transcript has advanced beyond the authentication failure"
        );
        let launch = self.resolve_launch_env(local_id, hint).await?;
        anyhow::ensure!(
            valid_recovery_env(&launch.env),
            "auth recovery refused: no complete gateway credential"
        );
        // Recheck after the server round-trip; a user may have resumed meanwhile.
        anyhow::ensure!(
            self.auth_recovery_quiescent(&short) && tail_has_gateway_auth_error(&path)?,
            "auth recovery refused: worker made progress"
        );
        self.stop_worker(sock, &short).await;
        anyhow::ensure!(
            Self::await_worker_exit(sock, &short).await,
            "auth recovery aborted: old worker did not exit"
        );
        self.resume_if_hibernated(sock, &short, local_id, &launch.env).await?;
        tracing::info!(%local_id, "recovered gateway authentication in a fresh worker");
        Ok(())
    }

    /// The live transcript of the worker being repaired. `EnterWorktree`
    /// relocates it under the worktree's project slug, so the launch-cwd path
    /// may not exist: prefer the pinned tail location for this session, then
    /// the newest file for the session id across project dirs, and only then
    /// the path derived from the launch cwd.
    fn recovery_transcript_path(
        &self,
        short: &str,
        cwd: &str,
        session_id: &str,
    ) -> std::path::PathBuf {
        if let Some(loc) = self
            .transcript_locations
            .get(short)
            .filter(|loc| loc.offset_key == session_id && loc.path.exists())
        {
            return loc.path.clone();
        }
        transcript::newest_transcript_for_session(&self.cfg.projects_root, session_id)
            .unwrap_or_else(|| {
                transcript::transcript_path(&self.cfg.projects_root, cwd, session_id)
            })
    }

    fn auth_recovery_quiescent(&self, short: &str) -> bool {
        let Some(status) = self.last_status.get(short) else { return true };
        if status.tempo.as_deref().is_none_or(|tempo| matches!(tempo, "idle" | "blocked")) {
            return true;
        }
        let on_disk = super::StateJson::read(&self.cfg.jobs_root, short);
        silent_since_native_respawn(status, on_disk.as_ref())
    }

    /// Deliver a user message to a worker, handling a pending `AskUserQuestion`
    /// form. With structured `ask_picks` and the hook-captured questions we
    /// answer the form *natively* — keystrokes on the real form, so claude
    /// records a genuine `tool_result` with the selected labels.
    /// Otherwise (free-text answer, missing questions, keystroke failure) fall
    /// back to dismiss-then-reply: attach+ESC the form away, then `reply` the
    /// text (claude records the ask as declined and reads the text as a new
    /// user turn).
    // Sequential reply-delivery pipeline (resolve short → resume-on-reply →
    // ask-form vs text → submit) whose complexity is linear `?`-propagating I/O
    // steps plus hibernation/ENOJOB recovery branches, not nesting. Splitting risks
    // the resume/recovery control flow; kept whole deliberately.
    #[allow(clippy::cognitive_complexity)]
    pub(super) async fn deliver_reply(
        &self,
        sock: &std::path::Path,
        local_id: &str,
        text: &str,
        ask_picks: Option<Vec<Vec<usize>>>,
        env: &std::collections::BTreeMap<String, String>,
        turn_id: Option<uuid::Uuid>,
    ) -> anyhow::Result<()> {
        // Recorded before the op so the transcript tail cannot observe the
        // injected turn ahead of its id. A reply with no id clears any previous
        // one rather than letting it leak onto an unrelated turn.
        self.note_turn(local_id, turn_id);
        self.note_delivered(local_id, text);
        // Hibernated sessions (worker exited, job state still on disk)
        // have left `short_by_session`, so fall back to deriving the
        // short from the session id — same as the removal path. The derived
        // short is a pure function of the session id, so it "resolves" on a
        // machine that has never seen the session; without the on-disk job
        // check a misrouted reply would cold-resume a duplicate worker for
        // another daemon's session.
        let short = if let Ok(short) = self.resolve_short(local_id) {
            short
        } else {
            let short = self.resolve_short_for_removal(local_id)?;
            anyhow::ensure!(
                self.cfg.jobs_root.join(&short).is_dir(),
                "session {local_id} is not on this machine",
            );
            short
        };
        // Resume-on-reply: a reply to an exited worker is
        // ENOJOB'd by the claude daemon and silently lost. Revive it
        // first via a resume `dispatch`, then deliver as normal. Live
        // workers take the existing path with zero extra ops.
        self.resume_if_hibernated(sock, &short, local_id, env).await?;
        // If an AskUserQuestion form is up in the worker's PTY, a bare
        // `reply` just presses Enter on the highlighted option — claude
        // records option 1 ("Proceed"-style) and the user's text is
        // swallowed.
        let peer = crate::adapters::is_peer_envelope(text);
        let pending_ask = match self.pending_asks.lock() {
            Ok(m) if peer && m.contains_key(local_id) => return Err(AskOpened.into()),
            Ok(mut m) => m.remove(local_id),
            Err(_) => None,
        };
        let had_pending_ask = pending_ask.is_some();
        if let Some(questions) = pending_ask {
            // Native answer first: drive the real form via keystrokes.
            let native_picks = ask_picks
                .as_ref()
                .and_then(|picks| questions.as_ref().and_then(|q| ask_keystrokes(q, picks)));
            if let Some(chunks) = native_picks {
                match socket::attach_answer_keys(sock, &short, &chunks).await {
                    Ok(()) => {
                        tracing::info!(%short, "answered ask form natively via keystrokes");
                        // PostToolUse fires for the real answer and emits
                        // `resolved`, but synthesize one too so the live card
                        // drops immediately (it's idempotent client-side).
                        let _ = self
                            .events
                            .send(AdapterEvent::AskResolved { local_id: local_id.to_owned() })
                            .await;
                        // A plan prompt is stored in the same pending map; emit
                        // PlanResolved too so a live Plan card drops. Idempotent
                        // (clients only clear their own kind).
                        let _ = self
                            .events
                            .send(AdapterEvent::PlanResolved { local_id: local_id.to_owned() })
                            .await;
                        return Ok(());
                    }
                    Err(err) => {
                        // do NOT fall through to attach+ESC here. The
                        // pending-ask record can be stale (the form already
                        // resolved in the native TUI or timed out, and the
                        // `resolved` hook hasn't reached us yet), in which case
                        // an ESC lands on whatever is now on screen — typically a
                        // running tool — and aborts the turn. That is exactly the
                        // "answering interrupted the tool" symptom. A stray text
                        // reply is harmless by comparison, so just deliver it.
                        tracing::warn!(%err, %short, "native ask answer failed; delivering text reply without ESC");
                    }
                }
            } else if ask_picks.is_none() {
                // Genuine free-text answer: the user typed prose rather than
                // picking options, so the form must be dismissed before the text
                // lands or claude records option 1 + swallows the text.
                // This is the only path that intentionally dismisses the form.
                if let Err(err) = socket::attach_interrupt(sock, &short).await {
                    tracing::warn!(%err, %short, "failed to dismiss pending ask form");
                } else {
                    tracing::info!(%short, "dismissed pending ask form before free-text reply");
                    // PostToolUse never fires for a cancelled ask, so synthesize
                    // `resolved` so the server/clients drop the live card.
                    let _ = self
                        .events
                        .send(AdapterEvent::AskResolved { local_id: local_id.to_owned() })
                        .await;
                    let _ = self
                        .events
                        .send(AdapterEvent::PlanResolved { local_id: local_id.to_owned() })
                        .await;
                    // Give the TUI a beat to settle after the ESC before the
                    // reply lands.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
            // Fallback paths (native answer failed, or option picks with an
            // unanswerable shape) deliver the text reply without dismissing the
            // form. The user has still answered, so tell clients to drop the live
            // card; idempotent if a real `resolved` hook follows, and a harmless
            // repeat of the free-text branch's own emit above.
            let _ =
                self.events.send(AdapterEvent::AskResolved { local_id: local_id.to_owned() }).await;
            let _ = self
                .events
                .send(AdapterEvent::PlanResolved { local_id: local_id.to_owned() })
                .await;
        }
        // The `reply` op types into the live composer, which is not empty after
        // an ESC ESC draft restore. Only an idle worker with no form up is safe
        // to clear (mid-turn the composer is a queue, and a form eats the keys).
        if !had_pending_ask && !self.is_busy(&short) {
            match socket::attach_clear_composer(sock, &short).await {
                Ok(()) => tracing::debug!(%short, "cleared composer before reply"),
                Err(err) => tracing::warn!(%err, %short, "failed to clear composer before reply"),
            }
        }
        // Baseline before the reply op: a build that auto-submits multiline
        // replies grows the transcript immediately, and a later baseline would
        // hide that submit from the confirm loop.
        let confirm = self.submit_confirm(&short, local_id);
        let resp =
            socket::one_shot(sock, &json!({"proto":1,"op":"reply","short":short,"text":text}))
                .await?;
        tracing::debug!(?resp, %short, "reply ack");
        if text.contains('\n')
            && let Err(err) = Box::pin(socket::attach_submit(sock, &short, &confirm)).await
        {
            tracing::warn!(%err, %short, "failed to submit multiline reply draft");
        }
        Ok(())
    }

    /// Pick the submit-confirmation signal for a worker: transcript growth
    /// when idle (the only signal image ingestion can't fake), the composer
    /// emptying when mid-turn (a submit only queues the message, so the
    /// transcript won't grow and the spinner repaints either way), repaint when
    /// no transcript can be located.
    pub(super) fn submit_confirm(&self, short: &str, session_id: &str) -> socket::SubmitConfirm {
        if self.is_busy(short) {
            return socket::SubmitConfirm::Composer;
        }
        let path = self.transcript_locations.get(short).map(|loc| loc.path.clone()).or_else(|| {
            transcript::newest_transcript_for_session(&self.cfg.projects_root, session_id)
        });
        path.map_or(socket::SubmitConfirm::Repaint, |path| {
            let baseline = std::fs::metadata(&path).map_or(0, |m| m.len());
            socket::SubmitConfirm::Transcript { path, baseline }
        })
    }

    /// Whether the worker's last status reported a non-idle tempo (mid-turn).
    pub(super) fn is_busy(&self, short: &str) -> bool {
        self.last_status
            .get(short)
            .and_then(|s| s.tempo.as_deref())
            .is_some_and(|tempo| tempo != "idle")
    }
}

/// Read a bounded tail, ignoring metadata but refusing to restart after a new
/// user message, a tool call, or a successful assistant response.
fn tail_has_gateway_auth_error(path: &std::path::Path) -> anyhow::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(256 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(gateway_auth_at_tail(&String::from_utf8_lossy(&bytes)))
}

fn gateway_auth_at_tail(tail: &str) -> bool {
    for line in tail.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match value["type"].as_str() {
            Some("user") => return false,
            Some("assistant") => {
                let content = &value["message"]["content"];
                if let Some(text) = content.as_str() {
                    return cctui_proto::adapter::is_gateway_auth_error(text);
                }
                let Some(blocks) = content.as_array() else { return false };
                return blocks.len() == 1
                    && blocks[0]["type"] == "text"
                    && blocks[0]["text"]
                        .as_str()
                        .is_some_and(cctui_proto::adapter::is_gateway_auth_error);
            }
            _ => {}
        }
    }
    false
}

/// A worker the native supervisor revived keeps the `active` tempo it was
/// respawned with until the worker itself reports again. When the worker has
/// persisted nothing since that respawn and its last durable tempo is idle, the
/// live `active` is the supervisor's stale seed, not a turn in flight. Live
/// workers rewrite `state.json` as soon as they start a turn, so any write after
/// the respawn keeps the busy guard in force.
fn silent_since_native_respawn(
    status: &super::StatusSnapshot,
    on_disk: Option<&super::StateJson>,
) -> bool {
    let (Some("respawn"), Some(started_at), Some(on_disk)) =
        (status.source.as_deref(), status.started_at, on_disk)
    else {
        return false;
    };
    if on_disk.tempo.as_deref() != Some("idle") {
        return false;
    }
    on_disk
        .updated_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| at.timestamp_millis() <= started_at)
}

fn valid_recovery_env(env: &std::collections::BTreeMap<String, String>) -> bool {
    env.get("ANTHROPIC_AUTH_TOKEN").is_some_and(|t| t.starts_with("cctui_s_"))
        && env.get("ANTHROPIC_BASE_URL").is_some_and(|url| !url.trim().is_empty())
}

/// Translate a structured ask answer into the keystroke chunks that drive the
/// real `AskUserQuestion` form. `questions` is the raw
/// `tool_input.questions` array captured by the ask-hook; `picks` is one list
/// of 0-based option indices per question, in question order.
///
/// Form grammar (verified live against claude 2.1.162):
///   - single-select: the option digit (`1`-`9`) selects and auto-advances
///   - multiSelect: digits toggle options; `Tab` advances to the next question
///   - every form except a lone single-select question ends on a "Review your
///     answers" screen whose option 1 is "Submit answers" → final `1` submits
///
/// Returns `None` when the answer can't be expressed as form keystrokes
/// (count mismatch, out-of-range/duplicate picks, empty pick on a question,
/// several picks on a single-select) — the caller then falls back to the
/// dismiss-then-reply path, which handles free-text answers too.
pub(super) fn ask_keystrokes(
    questions: &serde_json::Value,
    picks: &[Vec<usize>],
) -> Option<Vec<Vec<u8>>> {
    let qs = questions.as_array()?;
    if qs.is_empty() || qs.len() != picks.len() {
        return None;
    }
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut any_multi = false;
    for (q, p) in qs.iter().zip(picks) {
        let n_opts = q.get("options").and_then(serde_json::Value::as_array)?.len();
        // Digits only address rows 1-9; real forms have ≤4 options, so >9 means
        // we're misreading the payload — bail to the fallback.
        if n_opts == 0 || n_opts > 9 || p.is_empty() || p.iter().any(|&i| i >= n_opts) {
            return None;
        }
        if q.get("multiSelect").and_then(serde_json::Value::as_bool).unwrap_or(false) {
            any_multi = true;
            let mut sorted = p.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() != p.len() {
                return None; // duplicate picks — toggling twice would deselect
            }
            for &i in &sorted {
                chunks.push(vec![b'1' + u8::try_from(i).ok()?]);
            }
            chunks.push(vec![b'\t']); // advance to the next question / review
        } else {
            if p.len() != 1 {
                return None;
            }
            chunks.push(vec![b'1' + u8::try_from(p[0]).ok()?]);
        }
    }
    // The review screen ("1. Submit answers") shows for every form except a
    // lone single-select question, which submits straight from its digit.
    if qs.len() > 1 || any_multi {
        chunks.push(vec![b'1']);
    }
    Some(chunks)
}

/// A peer turn reached the reply path while an ask form was up: it was not
/// delivered, and the form was left alone.
#[derive(Debug)]
pub(super) struct AskOpened;

impl std::fmt::Display for AskOpened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an ask form is open; the peer turn waits for it")
    }
}

impl std::error::Error for AskOpened {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_keystrokes_single_question_single_select() {
        // One single-select question: the digit submits directly, no review.
        let qs =
            json!([{ "question": "Red or blue?", "options": [{"label":"Red"},{"label":"Blue"}] }]);
        assert_eq!(ask_keystrokes(&qs, &[vec![1]]), Some(vec![b"2".to_vec()]));
    }

    #[test]
    fn ask_keystrokes_multiselect_tabs_then_submits() {
        // One multiSelect question: toggle digits, Tab to review, 1 submits.
        let qs = json!([{ "question": "Which?", "multiSelect": true,
            "options": [{"label":"A"},{"label":"B"},{"label":"C"}] }]);
        assert_eq!(
            ask_keystrokes(&qs, &[vec![2, 0]]), // unsorted on purpose
            Some(vec![b"1".to_vec(), b"3".to_vec(), b"\t".to_vec(), b"1".to_vec()])
        );
    }

    #[test]
    fn ask_keystrokes_multi_question_ends_on_review() {
        // multiSelect then single-select: toggles+Tab, digit, then review `1`.
        let qs = json!([
            { "question": "Fruits?", "multiSelect": true,
              "options": [{"label":"Apple"},{"label":"Banana"},{"label":"Cherry"}] },
            { "question": "Drink?", "options": [{"label":"Tea"},{"label":"Coffee"}] },
        ]);
        assert_eq!(
            ask_keystrokes(&qs, &[vec![0, 2], vec![1]]),
            Some(vec![b"1".to_vec(), b"3".to_vec(), b"\t".to_vec(), b"2".to_vec(), b"1".to_vec()])
        );
    }

    #[test]
    fn ask_keystrokes_rejects_unanswerable_shapes() {
        let qs = json!([{ "question": "Q", "options": [{"label":"A"},{"label":"B"}] }]);
        // count mismatch / empty pick / out of range / multi-pick on single-select
        assert_eq!(ask_keystrokes(&qs, &[]), None);
        assert_eq!(ask_keystrokes(&qs, &[vec![]]), None);
        assert_eq!(ask_keystrokes(&qs, &[vec![2]]), None);
        assert_eq!(ask_keystrokes(&qs, &[vec![0, 1]]), None);
        // duplicate toggles on multiSelect would cancel out
        let mq = json!([{ "question": "Q", "multiSelect": true,
            "options": [{"label":"A"},{"label":"B"}] }]);
        assert_eq!(ask_keystrokes(&mq, &[vec![0, 0]]), None);
        // not an array at all
        assert_eq!(ask_keystrokes(&json!({}), &[vec![0]]), None);
    }
}

#[cfg(test)]
mod auth_recovery_tests {
    use super::*;

    #[test]
    fn auth_recovery_stops_at_real_progress_or_user_input() {
        let failure = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Please run /login · API Error: 401 Invalid bearer token"}]}}"#;
        assert!(gateway_auth_at_tail(failure));
        assert!(gateway_auth_at_tail(&format!("{failure}\n{{\"type\":\"system\"}}")));
        for next in [
            r#"{"type":"user","message":{"content":"retry"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done"}]}}"#,
        ] {
            assert!(!gateway_auth_at_tail(&format!("{failure}\n{next}")));
        }
    }

    #[tokio::test]
    async fn recovery_refuses_a_busy_worker_before_touching_the_socket() {
        let (mut driver, _events) = super::super::test_support::driver();
        let short = "aabbccdd";
        let status = super::super::StatusSnapshot {
            tempo: Some("active".into()),
            state: None,
            detail: None,
            name: None,
            activity: None,
            model: None,
            effort: None,
            source: None,
            started_at: None,
        };
        driver.last_status.insert(short.into(), status);
        let error = driver
            .recover_gateway_auth(
                std::path::Path::new("/nonexistent/socket"),
                "aabbccdd-0000-0000-0000-000000000000",
                &std::collections::BTreeMap::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("worker is busy"));
    }

    #[tokio::test]
    async fn recovery_refuses_missing_gateway_credentials_without_killing() {
        let (driver, _events) = super::super::test_support::driver();
        let short = "aabbccdd";
        let session = "aabbccdd-0000-0000-0000-000000000000";
        let job = driver.cfg.jobs_root.join(short);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            serde_json::json!({"sessionId":session,"cwd":"/project"}).to_string(),
        )
        .unwrap();
        let transcript =
            transcript::transcript_path(&driver.cfg.projects_root, "/project", session);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(transcript, r#"{"type":"assistant","message":{"content":[{"type":"text","text":"API Error: 401 Invalid bearer token"}]}}"#).unwrap();
        let error = driver
            .recover_gateway_auth(
                std::path::Path::new("/nonexistent/socket"),
                session,
                &std::collections::BTreeMap::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no complete gateway credential"), "{error}");
        assert!(job.join("state.json").exists());
    }

    const AUTH_FAILURE: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"API Error: 401 Invalid bearer token"}]}}"#;

    #[tokio::test]
    async fn recovery_follows_a_transcript_moved_into_a_worktree() {
        // Production 0.24.5: a session that ran `EnterWorktree` has its
        // transcript only under the worktree's project slug; deriving the path
        // from the launch cwd failed with ENOENT and the session stayed in 401.
        let (driver, _events) = super::super::test_support::driver();
        let short = "aabbccdd";
        let session = "aabbccdd-0000-0000-0000-000000000000";
        let job = driver.cfg.jobs_root.join(short);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            json!({"sessionId":session,"cwd":"/project"}).to_string(),
        )
        .unwrap();
        let moved = transcript::transcript_path(
            &driver.cfg.projects_root,
            "/project/.claude/worktrees/feature",
            session,
        );
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::write(&moved, AUTH_FAILURE).unwrap();
        assert!(
            !transcript::transcript_path(&driver.cfg.projects_root, "/project", session).exists()
        );
        let error = driver
            .recover_gateway_auth(
                std::path::Path::new("/nonexistent/socket"),
                session,
                &std::collections::BTreeMap::new(),
            )
            .await
            .unwrap_err();
        // Past both transcript checks: only the credential guard remains.
        assert!(error.to_string().contains("no complete gateway credential"), "{error}");
    }

    #[tokio::test]
    async fn recovery_reads_the_pinned_transcript_location_first() {
        let (mut driver, _events) = super::super::test_support::driver();
        let short = "aabbccdd";
        let session = "aabbccdd-0000-0000-0000-000000000000";
        let job = driver.cfg.jobs_root.join(short);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            json!({"sessionId":session,"cwd":"/project"}).to_string(),
        )
        .unwrap();
        // The pinned file is the live tail and has moved on; a newer stale copy
        // elsewhere still ends on the 401 and must not be trusted.
        let pinned = transcript::transcript_path(&driver.cfg.projects_root, "/pinned", session);
        std::fs::create_dir_all(pinned.parent().unwrap()).unwrap();
        std::fs::write(&pinned, r#"{"type":"user","message":{"content":"retry"}}"#).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&pinned)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH)
            .unwrap();
        let launch = transcript::transcript_path(&driver.cfg.projects_root, "/project", session);
        std::fs::create_dir_all(launch.parent().unwrap()).unwrap();
        std::fs::write(&launch, AUTH_FAILURE).unwrap();
        driver.transcript_locations.insert(
            short.into(),
            super::super::TranscriptLocation {
                path: pinned,
                local_id: session.into(),
                cwd: "/project".into(),
                offset_key: session.into(),
            },
        );
        let error = driver
            .recover_gateway_auth(
                std::path::Path::new("/nonexistent/socket"),
                session,
                &std::collections::BTreeMap::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("transcript has advanced"), "{error}");
    }

    #[tokio::test]
    async fn recovery_restarts_only_the_failed_worker_with_gateway_settings() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (driver, _events) = super::super::test_support::driver();
        let session = uuid::Uuid::new_v4().to_string();
        let short = session[..8].to_owned();
        let job = driver.cfg.jobs_root.join(&short);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            json!({"sessionId":session,"cwd":"/project"}).to_string(),
        )
        .unwrap();
        let transcript =
            transcript::transcript_path(&driver.cfg.projects_root, "/project", &session);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(transcript, r#"{"type":"assistant","message":{"content":[{"type":"text","text":"API Error: 401 Invalid bearer token"}]}}"#).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("daemon.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let expected_short = short.clone();
        let expected_session = session.clone();
        let mock = tokio::spawn(async move {
            let mut dispatched = false;
            let mut killed = false;
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                tokio::io::BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let operation = request["op"].as_str().unwrap();
                let response = match operation {
                    "kill" => {
                        assert_eq!(request["short"], expected_short);
                        killed = true;
                        json!({"ok":true})
                    }
                    "has" => {
                        assert_eq!(request["short"], expected_short);
                        json!({"ok":true,"alive":dispatched})
                    }
                    "dispatch" => {
                        assert!(killed);
                        assert_eq!(request["d"]["short"], expected_short);
                        assert_eq!(request["d"]["sessionId"], expected_session);
                        assert_eq!(
                            request["d"]["env"]["ANTHROPIC_BASE_URL"],
                            "https://gateway.test/gateway/anthropic"
                        );
                        let args = request["d"]["launch"]["args"].as_array().unwrap();
                        assert!(args.iter().any(|a| a == "--settings"));
                        assert!(args.iter().any(|a| a == "--resume"));
                        dispatched = true;
                        json!({"ok":true})
                    }
                    other => panic!("unexpected {other}"),
                };
                write.write_all(format!("{response}\n").as_bytes()).await.unwrap();
                if dispatched && operation == "has" {
                    break;
                }
            }
        });
        let env = super::super::test_support::env_of(&[
            ("ANTHROPIC_AUTH_TOKEN", "cctui_s_example"),
            ("ANTHROPIC_BASE_URL", "https://gateway.test/gateway/anthropic"),
        ]);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            driver.recover_gateway_auth(&sock, &session, &env),
        )
        .await
        .unwrap()
        .unwrap();
        mock.await.unwrap();
        assert!(job.join("state.json").exists());
        crate::configsweep::remove_session_files(&short);
    }

    fn respawned_status(started_at: i64) -> super::super::StatusSnapshot {
        super::super::StatusSnapshot {
            tempo: Some("active".into()),
            state: Some("running".into()),
            detail: None,
            name: None,
            activity: None,
            model: None,
            effort: None,
            source: Some("respawn".into()),
            started_at: Some(started_at),
        }
    }

    fn on_disk(tempo: &str, updated_at: &str) -> super::super::StateJson {
        let mut st: super::super::StateJson = serde_json::from_value(json!({})).unwrap();
        st.tempo = Some(tempo.into());
        st.updated_at = Some(updated_at.into());
        st
    }

    #[test]
    fn a_native_respawn_that_never_reported_is_not_busy() {
        // Observed on 2.1.295: respawned 11:35:58.474Z, last write 11:35:58.384Z.
        let started = 1_791_545_758_474;
        let status = respawned_status(started);
        assert!(silent_since_native_respawn(
            &status,
            Some(&on_disk("idle", "2026-10-09T11:35:58.384Z"))
        ));
        // The worker wrote after its respawn: it may be mid-turn.
        assert!(!silent_since_native_respawn(
            &status,
            Some(&on_disk("idle", "2026-10-09T11:35:59.000Z"))
        ));
        // Durable tempo is not idle.
        assert!(!silent_since_native_respawn(
            &status,
            Some(&on_disk("active", "2026-10-09T11:35:58.384Z"))
        ));
        // Missing evidence never relaxes the guard.
        assert!(!silent_since_native_respawn(&status, None));
        assert!(!silent_since_native_respawn(&status, Some(&on_disk("idle", "garbage"))));
        let mut fleet = respawned_status(started);
        fleet.source = Some("fleet".into());
        assert!(!silent_since_native_respawn(
            &fleet,
            Some(&on_disk("idle", "2026-10-09T11:35:58.384Z"))
        ));
        let mut unknown_start = respawned_status(started);
        unknown_start.started_at = None;
        assert!(!silent_since_native_respawn(
            &unknown_start,
            Some(&on_disk("idle", "2026-10-09T11:35:58.384Z"))
        ));
    }

    #[test]
    fn recovery_guard_reads_the_durable_state_of_a_silent_respawn() {
        let (mut driver, _events) = super::super::test_support::driver();
        let short = "aabbccdd";
        let job = driver.cfg.jobs_root.join(short);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            json!({"sessionId":"s","cwd":"/p","tempo":"idle","state":"stopped","updatedAt":"2026-10-09T11:35:58.384Z"}).to_string(),
        )
        .unwrap();
        driver.last_status.insert(short.into(), respawned_status(1_791_545_758_474));
        assert!(driver.auth_recovery_quiescent(short));
        std::fs::write(
            job.join("state.json"),
            json!({"sessionId":"s","cwd":"/p","tempo":"active","updatedAt":"2026-10-09T11:40:00.000Z"}).to_string(),
        )
        .unwrap();
        assert!(!driver.auth_recovery_quiescent(short));
    }

    #[test]
    fn recovery_requires_both_gateway_keys_and_a_session_token() {
        let mut env = std::collections::BTreeMap::new();
        assert!(!valid_recovery_env(&env));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), "cctui_s_example".into());
        assert!(!valid_recovery_env(&env));
        env.insert("ANTHROPIC_BASE_URL".into(), "https://gateway.test/gateway/anthropic".into());
        assert!(valid_recovery_env(&env));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), "provider-token".into());
        assert!(!valid_recovery_env(&env));
    }
}
