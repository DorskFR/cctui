use super::*;

impl Driver {
    /// The `meta.extra` a newly discovered subagent is announced with. The
    /// `.meta.json` sidecar supplies the agent type and the exact parent
    /// `Task` tool call; Workflow-tool agents add run context so the UI can
    /// group them under a named "Workflow: <name> (<runId>)" node. A
    /// workflow agent with no sidecar keeps the `workflow-subagent` default.
    pub(super) fn subagent_extra(
        agent_id: &str,
        workflow: Option<&transcript::WorkflowContext>,
        meta: Option<&transcript::SubagentMeta>,
    ) -> serde_json::Value {
        let mut extra = json!({ "subagent": true, "agent_id": agent_id });
        let obj = extra.as_object_mut().expect("json object literal");
        if let Some(wf) = workflow {
            obj.insert("workflow_run_id".into(), json!(wf.run_id));
            if let Some(name) = &wf.name {
                obj.insert("workflow_name".into(), json!(name));
            }
        }
        let agent_type = meta
            .and_then(|m| m.agent_type.clone())
            .or_else(|| workflow.is_some().then(|| "workflow-subagent".to_owned()));
        if let Some(agent_type) = agent_type {
            obj.insert("agent_type".into(), json!(agent_type));
        }
        if let Some(m) = meta {
            if let Some(tool_use_id) = &m.tool_use_id {
                obj.insert("tool_use_id".into(), json!(tool_use_id));
            }
            if let Some(parent_agent_id) = &m.parent_agent_id {
                obj.insert("parent_agent_id".into(), json!(parent_agent_id));
            }
            if let Some(depth) = m.spawn_depth {
                obj.insert("spawn_depth".into(), json!(depth));
            }
        }
        extra
    }

    /// Carry the sidecar's `description` on the ordinary session-name path, so
    /// it persists like any other name and a sidecarless subagent keeps the
    /// 6-char-id fallback.
    pub(super) fn subagent_name_status(agent_id: &str, name: String) -> AdapterEvent {
        AdapterEvent::Status {
            local_id: agent_id.to_owned(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: Some(name),
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: Vec::new(),
        }
    }

    /// Discover and tail Task-tool subagents for every live parent session.
    /// Each subagent transcript lives at
    /// `<encoded-cwd>/<parent-session-id>/subagents/agent-<agentId>.jsonl`
    /// and reuses the standard transcript parser. Subagents are observe-only
    /// (no worker `short` → no command dispatch); lifecycle end is inferred
    /// from transcript quiescence.
    pub(super) async fn scan_subagents(&mut self, dirty_offsets: &mut bool) {
        // Snapshot parent locations to avoid borrowing `self` across the
        // `emit`/offset mutations below.
        let parents: Vec<(String, PathBuf, String)> = self
            .transcript_locations
            .values()
            .map(|loc| (loc.offset_key.clone(), loc.path.clone(), loc.cwd.clone()))
            .collect();

        for (parent_id, parent_path, cwd) in parents {
            let dir = transcript::subagents_dir(&parent_path);
            for entry in transcript::discover_subagents(&dir) {
                let transcript::SubagentEntry { agent_id, path, workflow, meta } = entry;
                if self.ended_subagents.contains(&agent_id) {
                    continue;
                }
                if !self.subagents.contains_key(&agent_id) {
                    self.subagents.insert(
                        agent_id.clone(),
                        SubagentState { parent_local_id: parent_id.clone(), idle_ticks: 0 },
                    );
                    self.emit(AdapterEvent::SessionStarted {
                        local_id: agent_id.clone(),
                        meta: SessionMeta {
                            working_dir: Some(cwd.clone()),
                            parent_local_id: Some(parent_id.clone()),
                            extra: Self::subagent_extra(
                                &agent_id,
                                workflow.as_ref(),
                                meta.as_ref(),
                            ),
                        },
                    })
                    .await;
                    if let Some(name) = meta.as_ref().and_then(|m| m.description.clone()) {
                        self.emit(Self::subagent_name_status(&agent_id, name)).await;
                    }
                }

                let off = self.offsets.get(&agent_id);
                match transcript::tail_once(&path, &agent_id, off) {
                    Ok((events, new_off)) => {
                        let grew = new_off != off;
                        if grew {
                            self.offsets.set(agent_id.clone(), new_off);
                            self.server_marks.insert(agent_id.clone(), new_off);
                            *dirty_offsets = true;
                        }
                        for evt in events {
                            self.emit(evt).await;
                        }
                        if grew {
                            self.emit(AdapterEvent::TranscriptMark {
                                local_id: agent_id.clone(),
                                offset: new_off,
                            })
                            .await;
                        }
                        if let Some(st) = self.subagents.get_mut(&agent_id) {
                            st.idle_ticks = if grew { 0 } else { st.idle_ticks + 1 };
                        }
                    }
                    Err(err) => {
                        tracing::debug!(%err, path = %path.display(), "subagent tail failed");
                    }
                }
            }
        }

        // Quiescence-based end: a subagent whose transcript has not grown for
        // SUBAGENT_IDLE_TICKS_TO_END consecutive polls has finished.
        let done: Vec<String> = self
            .subagents
            .iter()
            .filter(|(_, st)| st.idle_ticks >= SUBAGENT_IDLE_TICKS_TO_END)
            .map(|(id, _)| id.clone())
            .collect();
        for agent_id in done {
            self.subagents.remove(&agent_id);
            self.ended_subagents.insert(agent_id.clone());
            self.emit(AdapterEvent::SessionEnded {
                local_id: agent_id,
                reason: EndReason::Completed,
            })
            .await;
        }
    }

    /// End any still-tracked subagents whose parent has left the roster — a
    /// backstop for the quiescence heuristic so children never outlive their
    /// parent.
    pub(super) async fn end_subagents_of(&mut self, parent_local_id: &str) {
        let orphans: Vec<String> = self
            .subagents
            .iter()
            .filter(|(_, st)| st.parent_local_id == parent_local_id)
            .map(|(id, _)| id.clone())
            .collect();
        for agent_id in orphans {
            self.subagents.remove(&agent_id);
            self.ended_subagents.insert(agent_id.clone());
            self.emit(AdapterEvent::SessionEnded {
                local_id: agent_id,
                reason: EndReason::Completed,
            })
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[tokio::test]
    async fn subagent_discovered_as_nested_session() {
        let (mut d, mut rx) = driver();
        // Pre-create the subagent transcript so the first poll finds it.
        write_subagent(
            &d,
            "abcd1234",
            "a8412884de5cc5396",
            &[
                r#"{"type":"assistant","isSidechain":true,"agentId":"a8412884de5cc5396","message":{"content":[{"type":"text","text":"sub work"}]}}"#,
            ],
        );
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;

        let mut started_parent = false;
        let mut started_child = false;
        let mut child_text = false;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { local_id, meta } => {
                    if local_id == "a8412884de5cc5396" {
                        started_child = true;
                        assert_eq!(
                            meta.parent_local_id.as_deref(),
                            Some("abcd1234-uuid"),
                            "subagent must link to its parent session id"
                        );
                        assert_eq!(meta.working_dir.as_deref(), Some("/tmp"));
                    } else {
                        started_parent = true;
                    }
                }
                AdapterEvent::Message { local_id, .. } if local_id == "a8412884de5cc5396" => {
                    child_text = true;
                }
                _ => {}
            }
        }
        assert!(started_parent, "parent SessionStarted expected");
        assert!(started_child, "subagent SessionStarted expected");
        assert!(child_text, "subagent transcript should stream through");
    }

    #[tokio::test]
    async fn workflow_subagent_carries_workflow_meta() {
        // a Workflow-tool agent under subagents/workflows/<runId>/ is
        // discovered and its SessionStarted meta.extra carries workflow context.
        use std::io::Write;
        let (mut d, mut rx) = driver();
        let sess = "abcd1234-uuid";
        let parent_path = transcript::transcript_path(&d.cfg.projects_root, "/tmp", sess);
        let run_dir = transcript::subagents_dir(&parent_path).join("workflows").join("wf_test123");
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut f = std::fs::File::create(run_dir.join("agent-wfa.jsonl")).unwrap();
        f.write_all(br#"{"type":"assistant","isSidechain":true,"agentId":"wfa","message":{"content":[{"type":"text","text":"wf work"}]}}"#).unwrap();
        f.write_all(b"\n").unwrap();
        std::fs::write(
            run_dir.join("agent-wfa.meta.json"),
            br#"{"agentType":"workflow-subagent"}"#,
        )
        .unwrap();
        // Run-state with name (sibling: <session>/workflows/<runId>.json).
        let wf_state = parent_path.with_extension("").join("workflows");
        std::fs::create_dir_all(&wf_state).unwrap();
        std::fs::write(wf_state.join("wf_test123.json"), br#"{"name":"deep-research"}"#).unwrap();

        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;

        let mut extra = None;
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::SessionStarted { local_id, meta } = evt
                && local_id == "wfa"
            {
                extra = Some(meta.extra);
            }
        }
        let extra = extra.expect("workflow subagent SessionStarted expected");
        assert_eq!(extra.get("subagent").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(extra.get("workflow_run_id").and_then(|v| v.as_str()), Some("wf_test123"));
        assert_eq!(extra.get("workflow_name").and_then(|v| v.as_str()), Some("deep-research"));
        assert_eq!(extra.get("agent_type").and_then(|v| v.as_str()), Some("workflow-subagent"));
    }

    #[test]
    fn subagent_extra_carries_every_sidecar_field() {
        let meta = transcript::SubagentMeta {
            agent_type: Some("general-purpose".to_owned()),
            description: Some("Global competitors research".to_owned()),
            tool_use_id: Some("toolu_018vjjY9mx2Dfmjwv1mQs6nX".to_owned()),
            parent_agent_id: Some("af547156cb073af1e".to_owned()),
            spawn_depth: Some(2),
        };
        let extra = Driver::subagent_extra("a1e5bd", None, Some(&meta));
        assert_eq!(extra.get("subagent").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(extra.get("agent_id").and_then(|v| v.as_str()), Some("a1e5bd"));
        assert_eq!(extra.get("agent_type").and_then(|v| v.as_str()), Some("general-purpose"));
        assert_eq!(
            extra.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_018vjjY9mx2Dfmjwv1mQs6nX")
        );
        assert_eq!(
            extra.get("parent_agent_id").and_then(|v| v.as_str()),
            Some("af547156cb073af1e")
        );
        assert_eq!(extra.get("spawn_depth").and_then(serde_json::Value::as_u64), Some(2));
        // `description` rides the Status name, never `extra`.
        assert_eq!(extra.get("description"), None);
        assert_eq!(extra.get("workflow_run_id"), None);
    }

    #[test]
    fn subagent_extra_without_a_sidecar_is_the_bare_marks() {
        let extra = Driver::subagent_extra("f00dca", None, None);
        assert_eq!(extra.get("subagent").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(extra.get("agent_id").and_then(|v| v.as_str()), Some("f00dca"));
        for absent in ["agent_type", "tool_use_id", "parent_agent_id", "spawn_depth"] {
            assert_eq!(extra.get(absent), None, "{absent} must be absent");
        }
    }

    #[test]
    fn subagent_extra_defaults_the_type_for_a_sidecarless_workflow_agent() {
        let wf = transcript::WorkflowContext {
            run_id: "wf_test123".to_owned(),
            name: Some("deep-research".to_owned()),
        };
        let extra = Driver::subagent_extra("wfa", Some(&wf), None);
        assert_eq!(extra.get("workflow_run_id").and_then(|v| v.as_str()), Some("wf_test123"));
        assert_eq!(extra.get("workflow_name").and_then(|v| v.as_str()), Some("deep-research"));
        assert_eq!(extra.get("agent_type").and_then(|v| v.as_str()), Some("workflow-subagent"));

        // A sidecar on a workflow agent wins over that default.
        let meta = transcript::SubagentMeta {
            agent_type: Some("Explore".to_owned()),
            ..transcript::SubagentMeta::default()
        };
        let typed = Driver::subagent_extra("wfa", Some(&wf), Some(&meta));
        assert_eq!(typed.get("agent_type").and_then(|v| v.as_str()), Some("Explore"));

        // An unnamed run omits the name rather than emitting null.
        let anon = transcript::WorkflowContext { run_id: "wf_x".to_owned(), name: None };
        assert_eq!(Driver::subagent_extra("wfa", Some(&anon), None).get("workflow_name"), None);
    }

    #[test]
    fn subagent_name_status_sets_only_the_name() {
        let evt = Driver::subagent_name_status("a1e5bd", "Global competitors research".to_owned());
        let AdapterEvent::Status { local_id, name, tempo, state, activity, model, effort, .. } =
            evt
        else {
            panic!("expected a Status event");
        };
        assert_eq!(local_id, "a1e5bd");
        assert_eq!(name.as_deref(), Some("Global competitors research"));
        // Nothing else may be asserted by the rename — a Status carrying a
        // tempo or state here would move the subagent between buckets.
        assert!(tempo.is_none() && state.is_none() && activity.is_none());
        assert!(model.is_none() && effort.is_none());
    }

    #[tokio::test]
    async fn flat_task_subagent_is_named_from_its_sidecar() {
        // A Task-tool subagent used to reach the UI as a bare 6-char id hash.
        // Its sidecar names it, and the name rides the ordinary Status path.
        let (mut d, mut rx) = driver();
        write_subagent(
            &d,
            "abcd1234",
            "a1e5bd0f2c3d4e5f6",
            &[
                r#"{"type":"assistant","isSidechain":true,"agentId":"a1e5bd0f2c3d4e5f6","message":{"content":[{"type":"text","text":"sub work"}]}}"#,
            ],
        );
        let parent_path =
            transcript::transcript_path(&d.cfg.projects_root, "/tmp", "abcd1234-uuid");
        std::fs::write(
            transcript::subagents_dir(&parent_path).join("agent-a1e5bd0f2c3d4e5f6.meta.json"),
            br#"{"agentType":"general-purpose","description":"Global competitors research",
                 "toolUseId":"toolu_018vjjY9mx2Dfmjwv1mQs6nX","spawnDepth":1}"#,
        )
        .unwrap();
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;

        let mut extra = None;
        let mut name = None;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { local_id, meta }
                    if local_id == "a1e5bd0f2c3d4e5f6" =>
                {
                    extra = Some(meta.extra);
                }
                AdapterEvent::Status { local_id, name: n, .. }
                    if local_id == "a1e5bd0f2c3d4e5f6" =>
                {
                    name = n;
                }
                _ => {}
            }
        }
        assert_eq!(name.as_deref(), Some("Global competitors research"));
        let extra = extra.expect("task subagent SessionStarted expected");
        assert_eq!(extra.get("subagent").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(extra.get("agent_type").and_then(|v| v.as_str()), Some("general-purpose"));
        assert_eq!(
            extra.get("tool_use_id").and_then(|v| v.as_str()),
            Some("toolu_018vjjY9mx2Dfmjwv1mQs6nX")
        );
        assert_eq!(extra.get("spawn_depth").and_then(serde_json::Value::as_u64), Some(1));
        // No workflow run: this is the flat Task layout.
        assert_eq!(extra.get("workflow_run_id"), None);
    }

    #[tokio::test]
    async fn a_sidecarless_subagent_is_announced_unnamed() {
        // The pre-CCT-941 behaviour, preserved: no sidecar means no name and
        // no agent_type, and the UI falls back to the 6-char id.
        let (mut d, mut rx) = driver();
        write_subagent(
            &d,
            "abcd1234",
            "f00dcafe12345678a",
            &[
                r#"{"type":"assistant","isSidechain":true,"agentId":"f00dcafe12345678a","message":{"content":[{"type":"text","text":"anon"}]}}"#,
            ],
        );
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;

        let mut extra = None;
        let mut named = false;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { local_id, meta }
                    if local_id == "f00dcafe12345678a" =>
                {
                    extra = Some(meta.extra);
                }
                AdapterEvent::Status { local_id, name, .. } if local_id == "f00dcafe12345678a" => {
                    named |= name.is_some();
                }
                _ => {}
            }
        }
        let extra = extra.expect("subagent SessionStarted expected");
        assert!(!named, "a sidecarless subagent must not be given a name");
        assert_eq!(extra.get("subagent").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(extra.get("agent_type"), None);
        assert_eq!(extra.get("tool_use_id"), None);
    }

    #[tokio::test]
    async fn subagent_announced_once_then_ends_on_quiescence() {
        let (mut d, mut rx) = driver();
        write_subagent(
            &d,
            "abcd1234",
            "deadbeefcafe00001",
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#],
        );
        // First poll: discover + announce.
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        // Drain.
        while rx.try_recv().is_ok() {}

        // Subsequent idle polls must NOT re-announce, and after the idle
        // threshold the subagent ends exactly once.
        let mut started_again = 0;
        let mut ended = 0;
        for _ in 0..(SUBAGENT_IDLE_TICKS_TO_END + 2) {
            d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
            while let Ok(evt) = rx.try_recv() {
                match evt {
                    AdapterEvent::SessionStarted { local_id, .. }
                        if local_id == "deadbeefcafe00001" =>
                    {
                        started_again += 1;
                    }
                    AdapterEvent::SessionEnded { local_id, .. }
                        if local_id == "deadbeefcafe00001" =>
                    {
                        ended += 1;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(started_again, 0, "quiescent subagent must not be re-announced");
        assert_eq!(ended, 1, "subagent should end exactly once on quiescence");
    }

    #[tokio::test]
    async fn subagent_ends_when_parent_leaves_roster() {
        let (mut d, mut rx) = driver();
        write_subagent(
            &d,
            "abcd1234",
            "facefeed00001111a",
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#],
        );
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        while rx.try_recv().is_ok() {}
        // Parent disappears → its subagent must end too.
        d.apply_snapshot(vec![]).await;
        let mut child_ended = false;
        while let Ok(evt) = rx.try_recv() {
            if matches!(&evt, AdapterEvent::SessionEnded { local_id, .. } if local_id == "facefeed00001111a")
            {
                child_ended = true;
            }
        }
        assert!(child_ended, "subagent should end when its parent leaves the roster");
    }
}
