use super::super::session_registry;
use super::*;

impl Driver {
    /// Poll the `has` op until the worker is no longer alive (or we give up).
    /// `claude rm` is documented to work on already-exited sessions; racing it
    /// against a still-live worker is undefined, so we drain the kill first.
    /// Best-effort: a socket error or timeout just falls through to `claude rm`.
    /// `false` when the daemon could not confirm the exit: a socket error, or a
    /// worker still alive after the wait. `alive:false` from a daemon that does
    /// not host the job is not proof of death either, so the caller re-checks
    /// against the CLI's session registry.
    pub(super) async fn await_worker_exit(sock: &std::path::Path, short: &str) -> bool {
        for _ in 0..20 {
            match socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await {
                Ok(resp) => {
                    let alive =
                        resp.get("alive").and_then(serde_json::Value::as_bool).unwrap_or(false);
                    if !alive {
                        return true;
                    }
                }
                // Socket gone / op failed — nothing more to wait on.
                Err(_) => return false,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::warn!(%short, "worker still live 2s after kill; proceeding to `claude rm`");
        false
    }

    /// Run `claude rm <short>` to delete the job metadata + Claude-created
    /// worktree. A job the CLI no longer knows is already gone and counts as
    /// success; any other non-zero exit (typically a worktree with
    /// uncommitted changes) is a real failure the archive must report.
    pub(super) async fn claude_rm(&self, short: &str) -> anyhow::Result<ClaudeRmOutcome> {
        let mut cmd = tokio::process::Command::new(&self.cfg.claude_bin);
        cmd.arg("rm")
            .arg(short)
            // `claude` lives in `~/.local/bin`, off launchd's minimal PATH
            // — give the child an augmented PATH so exec succeeds.
            .env("PATH", crate::childenv::child_path());
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
        let out = cmd
            .output()
            .await
            .with_context(|| format!("spawning `{} rm {short}`", self.cfg.claude_bin))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let outcome = classify_claude_rm(out.status.code(), out.status.success(), &stdout, &stderr);
        match &outcome {
            ClaudeRmOutcome::Removed => {
                tracing::info!(%short, "removed claude job via `claude rm`");
            }
            ClaudeRmOutcome::AlreadyGone => {
                tracing::info!(%short, "claude job already gone; nothing to remove");
            }
            ClaudeRmOutcome::Occupied { pid, kind, job, detail } => {
                tracing::warn!(
                    %short, ?pid, ?kind, ?job, %detail,
                    "`claude rm` kept the job: its worktree holds a live session"
                );
            }
            ClaudeRmOutcome::Refused(detail) => {
                tracing::warn!(%short, %detail, "`claude rm` refused");
            }
        }
        Ok(outcome)
    }

    /// Whether this `Remove` may touch `short`. A claude job cctui did not
    /// start is removed only when a human asked; no automatic path may kill it
    /// or `claude rm` it.
    pub(super) fn removal_allowed(
        foreign: &HashSet<String>,
        short: &str,
        initiator: RemoveInitiator,
    ) -> bool {
        initiator == RemoveInitiator::User || !foreign.contains(short)
    }

    /// Stop the worker behind `short` and delete its job metadata.
    ///
    /// The connected `claude daemon` is not authoritative: it answers `ENOJOB`
    /// for a worker it does not host (a survivor of a daemon cycle), and
    /// `claude rm` then refuses because the process is still running. So an
    /// unacknowledged kill falls back to the CLI's own session registry and
    /// signals the pid directly.
    ///
    /// An occupancy is resolved in-band — the occupant is removed, or its pid
    /// signalled, and the removal retried at once — rather than reported as a
    /// failure for the purge to re-attempt on a timer, which cannot help.
    pub(super) async fn remove_job(
        &self,
        sock: &std::path::Path,
        short: &str,
        local_id: &str,
        initiator: RemoveInitiator,
    ) -> anyhow::Result<()> {
        let mut stack = vec![short.to_owned()];
        let mut attempts: HashMap<String, u8> = HashMap::new();
        while let Some(target) = stack.last().cloned() {
            let tries = attempts.entry(target.clone()).or_default();
            *tries += 1;
            if *tries > MAX_REMOVE_ATTEMPTS {
                anyhow::bail!("claude rm {target}: still occupied after {tries} attempts");
            }
            if !Self::removal_allowed(&self.foreign_shorts, &target, initiator) {
                tracing::info!(
                    %target,
                    "skipping automatic removal of a claude job cctui did not start"
                );
                stack.pop();
                continue;
            }
            self.stop_worker(sock, &target).await;
            match self.claude_rm(&target).await? {
                ClaudeRmOutcome::Removed | ClaudeRmOutcome::AlreadyGone => {
                    stack.pop();
                }
                ClaudeRmOutcome::Occupied { pid, kind, job, detail } => {
                    self.report_occupied(local_id, &detail).await;
                    let occupant = job.filter(|j| {
                        j != &target
                            && !stack.contains(j)
                            && Self::removal_allowed(&self.foreign_shorts, j, initiator)
                    });
                    if let Some(occupant) = occupant {
                        tracing::info!(%target, %occupant, "removing the occupant first");
                        stack.push(occupant);
                    } else if let Some(pid) =
                        pid.filter(|p| session_registry::proc_start_of(*p).is_some())
                    {
                        tracing::info!(%target, pid, ?kind, "terminating the occupant pid");
                        session_registry::terminate(pid).await;
                    } else {
                        anyhow::bail!("claude rm {target} kept the job: {detail}");
                    }
                }
                ClaudeRmOutcome::Refused(detail) => {
                    anyhow::bail!("claude rm {target} failed: {detail}")
                }
            }
        }
        Ok(())
    }

    /// Stop the worker behind `short`, falling back to the CLI's own session
    /// registry when the connected daemon does not confirm the kill.
    pub(super) async fn stop_worker(&self, sock: &std::path::Path, short: &str) {
        let kill = socket::one_shot(sock, &json!({"proto":1,"op":"kill","short":short})).await;
        let acked = kill
            .as_ref()
            .is_ok_and(|r| r.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false));
        let exited = Self::await_worker_exit(sock, short).await;
        if !acked || !exited {
            tracing::info!(
                %short, acked, exited,
                "claude daemon did not confirm the kill; falling back to the session registry"
            );
            self.terminate_registered_worker(short).await;
        }
    }

    /// SIGTERM the worker the CLI's session registry records for `short`, when
    /// one is there and its start time still matches.
    pub(super) async fn terminate_registered_worker(&self, short: &str) {
        let Some(dir) = session_registry::default_dir() else { return };
        let Some(pid) = session_registry::live_pid_for_job(&dir, short) else {
            tracing::debug!(%short, "no verifiable live pid in the claude session registry");
            return;
        };
        if session_registry::terminate(pid).await {
            tracing::info!(%short, pid, "terminated the orphaned worker directly");
        } else {
            tracing::warn!(%short, pid, "orphaned worker survived SIGTERM");
        }
    }

    /// Surface a `claude rm` occupancy on the session itself, so the UI can
    /// explain why an archived session still owns a job.
    pub(super) async fn report_occupied(&self, local_id: &str, detail: &str) {
        self.emit(AdapterEvent::Status {
            local_id: local_id.to_owned(),
            tempo: None,
            state: None,
            detail: Some(format!("job kept: {detail}")),
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
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClaudeRmOutcome {
    Removed,
    AlreadyGone,
    /// The CLI kept the job because its worktree is the working directory of a
    /// live session. Retrying on a timer cannot help — the occupant has to go
    /// first.
    Occupied {
        pid: Option<i32>,
        kind: Option<String>,
        job: Option<String>,
        detail: String,
    },
    Refused(String),
}

/// The CLI explains a refusal on **stdout** (`kept <short> — worktree is the
/// working directory of a live session (pid …)`, then what to do about it)
/// and reserves stderr for errors (`No job matching`, `couldn't remove …
/// EACCES`). Both go into the detail, whitespace collapsed, so the operator
/// reads the same message in the daemon log as at the prompt: a bare
/// `exit 1` cost hours of guesswork on a worktree held by a live session.
pub(super) fn classify_claude_rm(
    code: Option<i32>,
    success: bool,
    stdout: &str,
    stderr: &str,
) -> ClaudeRmOutcome {
    if success {
        return ClaudeRmOutcome::Removed;
    }
    if stderr.contains("No job matching") {
        return ClaudeRmOutcome::AlreadyGone;
    }
    let code = code.map_or_else(|| "signal".to_owned(), |c| c.to_string());
    let output =
        [stderr, stdout].iter().flat_map(|s| s.split_whitespace()).collect::<Vec<_>>().join(" ");
    let detail =
        if output.is_empty() { format!("exit {code}") } else { format!("exit {code}: {output}") };
    if let Some(Occupant { pid, kind, job }) = parse_occupant(&output) {
        return ClaudeRmOutcome::Occupied { pid, kind, job, detail };
    }
    ClaudeRmOutcome::Refused(detail)
}

pub(super) struct Occupant {
    pid: Option<i32>,
    kind: Option<String>,
    job: Option<String>,
}

/// Pull the occupant out of the CLI's refusal line, in either of the two shapes
/// it prints: `… live session (pid 42, agent)` and
/// `… background session deadbeef, pid 42`.
pub(super) fn parse_occupant(output: &str) -> Option<Occupant> {
    if !output.contains("working directory of a live session")
        && !output.contains("working directory of a background session")
    {
        return None;
    }
    let pid = output.split("pid ").nth(1).and_then(|rest| {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    });
    let kind = output
        .split("(pid ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|inside| inside.split(',').nth(1))
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    let job = output
        .split("background session ")
        .nth(1)
        .map(|rest| {
            rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>()
        })
        .filter(|j| !j.is_empty());
    Some(Occupant { pid, kind, job })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn only_a_user_remove_may_touch_a_foreign_job() {
        let foreign: HashSet<String> = std::iter::once("beefbeef".to_owned()).collect();

        assert!(!Driver::removal_allowed(&foreign, "beefbeef", RemoveInitiator::Automatic));
        assert!(Driver::removal_allowed(&foreign, "beefbeef", RemoveInitiator::User));
        // A fleet job is removed by either.
        assert!(Driver::removal_allowed(&foreign, "f1eetf1e", RemoveInitiator::Automatic));
        assert!(Driver::removal_allowed(&foreign, "f1eetf1e", RemoveInitiator::User));
    }

    #[tokio::test]
    async fn an_automatic_remove_leaves_a_foreign_job_alone() {
        let (mut d, _rx) = driver();
        d.cfg.claude_bin = "/nonexistent/claude-bin".to_owned();
        let mut human = snap("beefbeef", "working", None);
        human.source = Some("bg".into());
        d.apply_snapshot(vec![human]).await;

        // No socket and no `claude` binary: reaching either the kill or
        // `claude rm` would surface as an error.
        d.remove_job(
            std::path::Path::new("/nonexistent/control.sock"),
            "beefbeef",
            "sess-1",
            RemoveInitiator::Automatic,
        )
        .await
        .expect("an automatic remove must report success without acting");
    }

    #[tokio::test]
    async fn a_user_remove_still_removes_a_foreign_job() {
        let (mut d, _rx) = driver();
        d.cfg.claude_bin = "/nonexistent/claude-bin".to_owned();
        let mut human = snap("beefbeef", "working", None);
        human.source = Some("bg".into());
        d.apply_snapshot(vec![human]).await;

        let err = d
            .remove_job(
                std::path::Path::new("/nonexistent/control.sock"),
                "beefbeef",
                "sess-1",
                RemoveInitiator::User,
            )
            .await
            .expect_err("a user remove must reach `claude rm`");
        assert!(err.to_string().contains("/nonexistent/claude-bin"), "{err}");
    }

    #[test]
    fn claude_rm_outcome_distinguishes_gone_from_refused() {
        use super::{ClaudeRmOutcome, classify_claude_rm};
        assert_eq!(classify_claude_rm(Some(0), true, "", ""), ClaudeRmOutcome::Removed);
        assert_eq!(
            classify_claude_rm(Some(1), false, "", "No job matching 'ad162ca8'\n"),
            ClaudeRmOutcome::AlreadyGone
        );
        assert_eq!(
            classify_claude_rm(Some(1), false, "", "worktree has uncommitted changes: /w\n"),
            ClaudeRmOutcome::Refused("exit 1: worktree has uncommitted changes: /w".into())
        );
        assert_eq!(
            classify_claude_rm(None, false, "", ""),
            ClaudeRmOutcome::Refused("exit signal".into())
        );
        // stderr first when both speak: the error before the narration.
        assert_eq!(
            classify_claude_rm(Some(1), false, "kept x\n", "EACCES: permission denied\n"),
            ClaudeRmOutcome::Refused("exit 1: EACCES: permission denied kept x".into())
        );
    }

    #[test]
    fn claude_rm_occupancy_is_classified_with_its_occupant() {
        use super::{ClaudeRmOutcome, classify_claude_rm};

        // The CLI's own explanation of a refusal is on stdout: it must reach
        // the log, not be dropped for a bare exit code.
        let with_kind = classify_claude_rm(
            Some(1),
            false,
            "kept 229a5a4f — worktree is the working directory of a live session (pid 42, agent)\n  worktree kept at /w\n  exit that session, then run 'claude rm 229a5a4f' again\n",
            "",
        );
        match with_kind {
            ClaudeRmOutcome::Occupied { pid, kind, job, detail } => {
                assert_eq!(pid, Some(42));
                assert_eq!(kind.as_deref(), Some("agent"));
                assert_eq!(job, None);
                assert!(detail.contains("worktree kept at /w"), "{detail}");
            }
            other => panic!("expected Occupied, got {other:?}"),
        }

        let with_job = classify_claude_rm(
            Some(1),
            false,
            "kept 229a5a4f — worktree is the working directory of a background session d6df7150, pid 4242\n",
            "",
        );
        match with_job {
            ClaudeRmOutcome::Occupied { pid, job, .. } => {
                assert_eq!(pid, Some(4242));
                assert_eq!(job.as_deref(), Some("d6df7150"));
            }
            other => panic!("expected Occupied, got {other:?}"),
        }

        // A refusal that is not an occupancy stays a plain Refused, so the
        // caller still backs off on it.
        assert_eq!(
            classify_claude_rm(Some(1), false, "", "worktree has uncommitted changes: /w\n"),
            ClaudeRmOutcome::Refused("exit 1: worktree has uncommitted changes: /w".into())
        );
    }
}
