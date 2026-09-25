use super::{AdapterEvent, Driver, json, socket, transcript};

impl Driver {
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
        let pending_ask = self.pending_asks.lock().ok().and_then(|mut m| m.remove(local_id));
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
            && let Err(err) = socket::attach_submit(sock, &short, &confirm).await
        {
            tracing::warn!(%err, %short, "failed to submit multiline reply draft");
        }
        Ok(())
    }

    /// Pick the submit-confirmation signal for a worker: transcript growth
    /// when idle (the only signal image ingestion can't fake), repaint when
    /// mid-turn (a submit only queues the message, so the transcript won't
    /// grow) or when no transcript can be located.
    pub(super) fn submit_confirm(&self, short: &str, session_id: &str) -> socket::SubmitConfirm {
        if self.is_busy(short) {
            return socket::SubmitConfirm::Repaint;
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
