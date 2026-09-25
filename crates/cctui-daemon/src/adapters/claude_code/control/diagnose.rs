use cctui_proto::diagnose::{
    AttachStatus, DiagnoseFact, DispatchStatus, EffectiveState, GatewayStatus, HookEvent,
    PendingPrompts, PtyOutputStats, SessionDiagnose, SocketStatus, TranscriptStatus,
};

use super::super::attach::AttachSnapshot;
use super::super::diagnose::{
    ActivityInput, ActivityVerdict, ArbitrationInput, arbitrate, arbitrate_activity, now_unix_ms,
    to_unix_ms,
};
use super::*;

/// Hook- and control-socket-side prompt state for one session.
struct PromptSignals {
    pending_ask: bool,
    parked_perm_hook: bool,
    pending_perm: Option<PendingPerm>,
}

impl Driver {
    /// Assemble the session-diagnose report: everything this driver
    /// already knows about `local_id`, each fact dated + sourced, and emit it
    /// back as an [`AdapterEvent::Diagnose`] echoing `request_id`.
    ///
    /// Fail-soft by construction: facts that cannot be produced right now
    /// come back `missing(reason)`; the only hard failure is the events
    /// channel being gone.
    pub(super) async fn handle_diagnose(
        &self,
        local_id: &str,
        request_id: uuid::Uuid,
    ) -> anyhow::Result<()> {
        let now_ms = now_unix_ms();
        let short =
            self.resolve_short(local_id).or_else(|_| self.resolve_short_for_removal(local_id)).ok();

        let signals = PromptSignals {
            pending_ask: self.pending_asks.lock().is_ok_and(|m| m.contains_key(local_id)),
            parked_perm_hook: self
                .pending_perm_hooks
                .lock()
                .is_ok_and(|m| m.contains_key(local_id)),
            pending_perm: short.as_deref().and_then(|s| self.pending_perms.get(s)).cloned(),
        };

        let attach_snap = short.as_deref().and_then(|s| self.attach.snapshot(s));

        let hook_age_ms = self
            .hook_log
            .lock()
            .ok()
            .and_then(|m| m.get(local_id).map(|(_, at)| to_unix_ms(*at)))
            .map(|at| now_ms - at);

        let effective_state = short.as_deref().map_or_else(
            || DiagnoseFact::missing("activity", "unknown session (no worker short resolvable)"),
            |short| {
                self.effective_state_fact(
                    short,
                    now_ms,
                    &signals,
                    hook_age_ms,
                    attach_snap.as_ref(),
                )
            },
        );

        let last_hook_event =
            self.hook_log.lock().ok().and_then(|m| m.get(local_id).cloned()).map_or_else(
                || DiagnoseFact::missing("hook", "no hook delivery seen for this session"),
                |(kind, at)| {
                    DiagnoseFact::observed(HookEvent { kind }, "hook", to_unix_ms(at), now_ms)
                },
            );

        let attach = attach_fact(attach_snap.as_ref(), now_ms);
        let pty_output = pty_output_fact(attach_snap.as_ref(), now_ms);
        let claude_socket = self.claude_socket_fact(now_ms).await;
        let transcript = self.transcript_fact(short.as_deref(), now_ms);

        let prompts = DiagnoseFact::fresh(
            PendingPrompts {
                pending_ask: signals.pending_ask,
                parked_perm_hook: signals.parked_perm_hook,
                control_needs: signals.pending_perm.as_ref().map(|p| p.needs.clone()),
                perm_request_id: signals.pending_perm.map(|p| p.request_id),
            },
            "hook+control_socket",
            now_ms,
        );

        let permission_mode = self.permission_mode_fact(short.as_deref());
        let dispatch = self.dispatch_fact(short.as_deref(), now_ms);

        let gateway = DiagnoseFact::fresh(
            GatewayStatus {
                server_configured: self.server.is_some() && self.machine_key.is_some(),
            },
            "daemon-config",
            now_ms,
        );

        let report = SessionDiagnose {
            local_id: local_id.to_owned(),
            short,
            generated_at_ms: now_ms,
            adapter: "claude-code".to_owned(),
            effective_state,
            last_hook_event,
            attach,
            pty_output,
            claude_socket,
            transcript,
            prompts,
            permission_mode,
            dispatch,
            gateway,
            codex: None,
        };
        self.events
            .send(AdapterEvent::Diagnose {
                local_id: local_id.to_owned(),
                request_id,
                report: Box::new(report),
            })
            .await
            .map_err(|_| anyhow::anyhow!("events channel closed while sending diagnose report"))
    }

    /// Effective state + arbitration verdict for a resolved worker short.
    fn effective_state_fact(
        &self,
        short: &str,
        now_ms: i64,
        signals: &PromptSignals,
        hook_age_ms: Option<i64>,
        attach_snap: Option<&AttachSnapshot>,
    ) -> DiagnoseFact<EffectiveState> {
        let snap = self.last_status.get(short);
        let (verdict, source) = arbitrate(&ArbitrationInput {
            pending_ask: signals.pending_ask,
            parked_perm_hook: signals.parked_perm_hook,
            control_needs: signals.pending_perm.as_ref().map(|p| p.needs.as_str()),
            reported_dead: self.dead_shorts.contains(short),
            in_roster: self.roster.contains(short),
            state_json_on_disk: StateJson::read(&self.cfg.jobs_root, short).is_some(),
            tempo: snap.and_then(|s| s.tempo.as_deref()),
            state: snap.and_then(|s| s.state.as_deref()),
        });
        let pty_activity = self.pty_activity(short, now_ms, hook_age_ms, attach_snap);
        let value = EffectiveState {
            verdict,
            tempo: snap.and_then(|s| s.tempo.clone()),
            state: snap.and_then(|s| s.state.clone()),
            detail: snap.and_then(|s| s.detail.clone()),
            activity: pty_activity.or_else(|| snap.and_then(|s| s.activity.clone())),
        };
        match self.last_status_at.get(short) {
            Some(at) => DiagnoseFact::observed(value, source.as_str(), to_unix_ms(*at), now_ms),
            None => DiagnoseFact::undated(value, source.as_str()),
        }
    }

    /// Second (PTY) signal: herdr-style arbitration of held-attach byte flow
    /// against hook freshness. Surfaced on `activity` when it carries a real
    /// verdict, never clobbering a status one.
    fn pty_activity(
        &self,
        short: &str,
        now_ms: i64,
        hook_age_ms: Option<i64>,
        attach_snap: Option<&AttachSnapshot>,
    ) -> Option<String> {
        let av = arbitrate_activity(&ActivityInput {
            hook_age_ms,
            pty_last_output_age_ms: attach_snap
                .and_then(|s| s.last_output_at)
                .map(|at| now_ms - to_unix_ms(at)),
            pty_bytes_per_min: attach_snap
                .and_then(|s| s.bytes_per_min(SystemTime::now()))
                .unwrap_or(0.0),
            liveness_alive: attach_snap
                .and_then(|s| s.last_probe_alive)
                .unwrap_or_else(|| self.roster.contains(short)),
            idle_confirmations: attach_snap.map_or(0, |s| s.idle_confirmations),
        });
        match av {
            ActivityVerdict::Uncertain => None,
            v => Some(v.as_str().to_owned()),
        }
    }

    /// Live probe at report time: which socket discovery picks, and the
    /// full candidate list. Bounded (per-candidate probe timeout), no
    /// kickstart side effects.
    async fn claude_socket_fact(&self, now_ms: i64) -> DiagnoseFact<SocketStatus> {
        let candidates: Vec<String> = self
            .cfg
            .discovery
            .candidate_paths()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let live_sock = self.cfg.discovery.locate_live().await;
        DiagnoseFact::fresh(
            SocketStatus {
                live: live_sock.is_some(),
                path: live_sock.map(|p| p.to_string_lossy().into_owned()),
                candidates,
            },
            "discovery",
            now_ms,
        )
    }

    fn transcript_fact(&self, short: Option<&str>, now_ms: i64) -> DiagnoseFact<TranscriptStatus> {
        short.and_then(|s| self.transcript_locations.get(s)).map_or_else(
            || DiagnoseFact::missing("filesystem", "no transcript pinned for this session yet"),
            |loc| {
                let meta = std::fs::metadata(&loc.path).ok();
                let mtime = meta.as_ref().and_then(|m| m.modified().ok());
                let parsed = self.last_parsed.get(&loc.offset_key);
                let value = TranscriptStatus {
                    path: loc.path.to_string_lossy().into_owned(),
                    mtime_ms: mtime.map(to_unix_ms),
                    size_bytes: meta.as_ref().map(std::fs::Metadata::len),
                    tail_offset: self.offsets.get(&loc.offset_key),
                    last_parsed_event: parsed.map(|(kind, _)| kind.clone()),
                    last_parsed_at_ms: parsed.map(|(_, at)| to_unix_ms(*at)),
                };
                match mtime {
                    Some(at) => DiagnoseFact::observed(value, "filesystem", to_unix_ms(at), now_ms),
                    None => DiagnoseFact::undated(value, "filesystem"),
                }
            },
        )
    }

    fn permission_mode_fact(&self, short: Option<&str>) -> DiagnoseFact<String> {
        let recorded = short
            .and_then(|s| self.spawn_permission_mode.lock().ok().and_then(|m| m.get(s).cloned()));
        match recorded {
            Some(label) => DiagnoseFact::undated(label, "spawn"),
            // The spawn-time record dies with the daemon process; the whip
            // Stop hook in the managed settings file survives on disk.
            None if short.is_some_and(detect_whip_from_settings) => {
                DiagnoseFact::undated("whip".to_owned(), "settings-file")
            }
            None => DiagnoseFact::missing(
                "spawn",
                "not recorded (session predates this daemon process or was launched externally)",
            ),
        }
    }

    fn dispatch_fact(&self, short: Option<&str>, now_ms: i64) -> DiagnoseFact<DispatchStatus> {
        self.dispatch_done
            .lock()
            .ok()
            .and_then(|guard| {
                guard.as_ref().and_then(|t| {
                    (Some(t.short()) == short).then(|| DispatchStatus {
                        seen_busy: t.seen_busy(),
                        done: t.is_done(),
                        marker_path: t.marker_path().to_string_lossy().into_owned(),
                    })
                })
            })
            .map_or_else(
                || {
                    DiagnoseFact::missing(
                        "dispatch",
                        "not a dispatched session (no turn-complete watcher armed)",
                    )
                },
                |value| DiagnoseFact::fresh(value, "dispatch", now_ms),
            )
    }
}

fn attach_fact(attach_snap: Option<&AttachSnapshot>, now_ms: i64) -> DiagnoseFact<AttachStatus> {
    attach_snap.map_or_else(
        || DiagnoseFact::missing("attach", "no keep-alive attach task for this session"),
        |snap| {
            let value = AttachStatus {
                phase: snap.phase.clone(),
                backoff_ms: snap.backoff.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                last_probe_alive: snap.last_probe_alive,
                last_probe_at_ms: snap.last_probe_at.map(to_unix_ms),
            };
            match snap.updated_at {
                Some(at) => DiagnoseFact::observed(value, "attach", to_unix_ms(at), now_ms),
                None => DiagnoseFact::undated(value, "attach"),
            }
        },
    )
}

/// PTY output age/throughput from the held-attach drain loop: the raw
/// activity signal state derivation weighs against hook freshness.
fn pty_output_fact(
    attach_snap: Option<&AttachSnapshot>,
    now_ms: i64,
) -> DiagnoseFact<PtyOutputStats> {
    match attach_snap.filter(|s| s.last_output_at.is_some()) {
        Some(snap) => {
            let last = snap.last_output_at.expect("filtered to Some");
            let value = PtyOutputStats {
                last_output_age_ms: Some(now_ms - to_unix_ms(last)),
                recent_bytes_per_min: snap.bytes_per_min(SystemTime::now()),
            };
            DiagnoseFact::observed(value, "pty", to_unix_ms(last), now_ms)
        }
        None => DiagnoseFact::missing("pty", "no PTY output observed on the held attach yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    /// the diagnose assembly aggregates the driver's live state —
    /// resolved short, activity-sourced verdict with an observation timestamp,
    /// pinned transcript, and honest `missing` facts for signals not present.
    #[tokio::test]
    async fn diagnose_assembles_dated_facts_for_live_session() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("abcd1234", "working", Some("ours"))]).await;

        let request_id = uuid::Uuid::new_v4();
        d.handle_diagnose("abcd1234-uuid", request_id).await.unwrap();
        let report = recv_diagnose(&mut rx, request_id).await;

        assert_eq!(report.local_id, "abcd1234-uuid");
        assert_eq!(report.short.as_deref(), Some("abcd1234"));
        assert_eq!(report.adapter, "claude-code");
        assert!(report.generated_at_ms > 0);

        // Effective state: derived from the poll snapshot (activity source),
        // dated by the poll observation.
        let es = &report.effective_state;
        assert_eq!(es.source, "activity");
        let v = es.value.as_ref().expect("effective state present");
        assert_eq!(v.verdict, "active/working");
        assert_eq!(v.tempo.as_deref(), Some("active"));
        assert!(es.observed_at_ms.is_some());
        assert!(es.age_ms.is_some_and(|a| a >= 0));

        // Transcript was pinned by the snapshot; the file doesn't exist yet so
        // the fact is present-but-undated with offset 0.
        let t = report.transcript.value.as_ref().expect("transcript pinned");
        assert!(t.path.ends_with("abcd1234-uuid.jsonl"), "{}", t.path);
        assert_eq!(t.tail_offset, 0);
        assert_eq!(t.mtime_ms, None);

        // No socket in the temp discovery base.
        let sock = report.claude_socket.value.as_ref().unwrap();
        assert!(!sock.live);
        assert!(sock.path.is_none());

        // Nothing pending, nothing recorded → honest missing/false facts.
        let p = report.prompts.value.as_ref().unwrap();
        assert!(!p.pending_ask && !p.parked_perm_hook);
        // No held-attach task in this unit driver, so no PTY output observed.
        assert!(report.pty_output.value.is_none(), "no attach → no PTY output");
        assert!(report.pty_output.missing_reason.as_deref().unwrap().contains("PTY output"));
        assert!(report.dispatch.value.is_none(), "not a dispatched session");
        assert!(report.permission_mode.value.is_none(), "posture never recorded");
        assert!(report.last_hook_event.value.is_none());
        assert!(!report.gateway.value.as_ref().unwrap().server_configured);
    }

    /// a pending ask (hook signal) wins the arbitration and surfaces
    /// in both the verdict and the prompts fact; an unknown session still
    /// produces a fail-soft report rather than an error.
    #[tokio::test]
    async fn diagnose_hook_signal_wins_and_unknown_session_fails_soft() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        d.pending_asks.lock().unwrap().insert("abcd1234-uuid".into(), None);

        let request_id = uuid::Uuid::new_v4();
        d.handle_diagnose("abcd1234-uuid", request_id).await.unwrap();
        let report = recv_diagnose(&mut rx, request_id).await;
        assert_eq!(report.effective_state.source, "hook");
        assert!(report.effective_state.value.unwrap().verdict.contains("ask"));
        assert!(report.prompts.value.unwrap().pending_ask);

        // Unknown session: no short resolvable → missing facts, not an Err.
        let request_id = uuid::Uuid::new_v4();
        d.handle_diagnose("not-a-known-session", request_id).await.unwrap();
        let report = recv_diagnose(&mut rx, request_id).await;
        assert!(report.short.is_none());
        assert!(report.effective_state.value.is_none());
        assert!(
            report.effective_state.missing_reason.as_deref().unwrap().contains("unknown session")
        );
    }
}
