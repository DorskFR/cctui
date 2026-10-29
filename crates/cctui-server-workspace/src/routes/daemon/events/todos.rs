use cctui_proto::api::TodoEntry;

/// A `Message` that normalized to a non-meta user turn (`▷ User:` prefix, shared
/// by every adapter's user text) starts a new turn, so the per-turn tool count
/// resets.
pub(super) fn is_user_turn(event: Option<&cctui_proto::ws::AgentEvent>) -> bool {
    matches!(
        event,
        Some(cctui_proto::ws::AgentEvent::Text { content, meta: false, .. })
            if content.starts_with("▷ User:")
    )
}

/// Normalize a task-list tool call onto [`TodoEntry`], or `None` for any other
/// tool. Claude's `TodoWrite` carries `{todos: [{content, status, activeForm}]}`;
/// codex's synthetic `update_plan` (see `normalize::codex`) carries
/// `{plan: [{step, status}]}`. An empty array is a real value — the agent
/// clearing its list — so it is written, not skipped.
pub(super) fn extract_todos(tool: &str, input: &serde_json::Value) -> Option<Vec<TodoEntry>> {
    let items = match tool {
        "TodoWrite" => input.get("todos")?.as_array()?,
        "update_plan" => input.get("plan")?.as_array()?,
        _ => return None,
    };
    Some(
        items
            .iter()
            .map(|item| TodoEntry {
                content: item
                    .get("content")
                    .or_else(|| item.get("step"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                status: match item.get("status").and_then(serde_json::Value::as_str) {
                    Some("completed") => "completed",
                    Some("in_progress") => "in_progress",
                    _ => "pending",
                }
                .to_owned(),
                active_form: item
                    .get("activeForm")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                ..TodoEntry::default()
            })
            .collect(),
    )
}

/// One step of Claude Code's incremental task list, read off the live stream.
///
/// `TaskCreate`'s input has no id, so a creation is read from its result
/// (`Task #N created successfully: <subject>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TaskOp {
    Created { id: String, subject: String },
    Updated { id: String, input: serde_json::Value },
}

pub(super) fn task_op(event: &cctui_proto::ws::AgentEvent) -> Option<TaskOp> {
    match event {
        cctui_proto::ws::AgentEvent::ToolCall { tool, input, .. } if tool == "TaskUpdate" => {
            let id = match input.get("taskId")? {
                serde_json::Value::String(s) => s.trim().to_owned(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            (!id.is_empty()).then(|| TaskOp::Updated { id, input: input.clone() })
        }
        cctui_proto::ws::AgentEvent::ToolResult { output_summary, error: false, .. } => {
            let rest = output_summary.trim_start().strip_prefix("Task #")?;
            let (id, subject) = rest.split_once(" created successfully: ")?;
            (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
                .then(|| TaskOp::Created { id: id.to_owned(), subject: subject.trim().to_owned() })
        }
        _ => None,
    }
}

/// Apply `op` to a stored list, idempotently.
///
/// Entries without an id are a `TodoWrite` / `update_plan` list, superseded
/// once a task lands.
pub(super) fn fold_task(list: &mut Vec<TodoEntry>, op: TaskOp) {
    match op {
        TaskOp::Created { id, subject } => {
            list.retain(|t| t.id.is_some());
            if !list.iter().any(|t| t.id.as_deref() == Some(id.as_str())) {
                list.push(TodoEntry {
                    content: subject,
                    status: "pending".to_owned(),
                    id: Some(id),
                    ..TodoEntry::default()
                });
                list.sort_by_key(|t| task_order(t.id.as_deref()));
            }
        }
        TaskOp::Updated { id, input } => {
            if input.get("status").and_then(serde_json::Value::as_str) == Some("deleted") {
                list.retain(|t| t.id.as_deref() != Some(id.as_str()));
            } else if let Some(entry) =
                list.iter_mut().find(|t| t.id.as_deref() == Some(id.as_str()))
            {
                apply_update(entry, &input);
            }
        }
    }
}

fn apply_update(entry: &mut TodoEntry, input: &serde_json::Value) {
    let field = |key: &str| input.get(key).and_then(serde_json::Value::as_str);
    let non_empty = |key: &str| field(key).map(|s| Some(s.to_owned()).filter(|s| !s.is_empty()));
    if let Some(status) = field("status") {
        task_status(status).clone_into(&mut entry.status);
    }
    if let Some(Some(subject)) = non_empty("subject") {
        entry.content = subject;
    }
    if let Some(description) = non_empty("description") {
        entry.description = description;
    }
    if let Some(active_form) = non_empty("activeForm") {
        entry.active_form = active_form;
    }
    if let Some(owner) = non_empty("owner") {
        entry.owner = owner;
    }
    let added = input.get("addBlockedBy").and_then(serde_json::Value::as_array);
    for blocker in added.into_iter().flatten().filter_map(serde_json::Value::as_str) {
        if !entry.blocked_by.iter().any(|b| b == blocker) {
            entry.blocked_by.push(blocker.to_owned());
        }
    }
}

fn task_status(raw: &str) -> &'static str {
    match raw {
        "completed" => "completed",
        "in_progress" => "in_progress",
        _ => "pending",
    }
}

/// Numeric ids in numeric order, anything else after them.
fn task_order(id: Option<&str>) -> (u64, String) {
    let id = id.unwrap_or_default();
    (id.parse().unwrap_or(u64::MAX), id.to_owned())
}

/// Fold a task-list step into the session's stored list.
pub(super) async fn record_task_op(pool: &sqlx::PgPool, local_id: &str, op: TaskOp) {
    let stored: Option<Option<serde_json::Value>> =
        match sqlx::query_scalar("SELECT todos FROM sessions WHERE id = $1")
            .bind(local_id)
            .fetch_optional(pool)
            .await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(%err, %local_id, "todo read failed");
                return;
            }
        };
    let Some(stored) = stored else { return };
    let mut list: Vec<TodoEntry> =
        stored.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let before = list.clone();
    fold_task(&mut list, op);
    if list != before {
        record_todos(pool, local_id, &list).await;
    }
}

/// Persist the session's task list. **Leaf only, deliberately unlike
/// [`Bumps::flush`]**: each subagent owns its own list, so rolling up the
/// `parent_id` chain would make a child's todos masquerade as the parent's.
pub(super) async fn record_todos(pool: &sqlx::PgPool, local_id: &str, todos: &[TodoEntry]) {
    let payload = match serde_json::to_value(todos) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%err, %local_id, "todo serialization failed");
            return;
        }
    };
    if let Err(err) =
        sqlx::query("UPDATE sessions SET todos = $2, todo_updated_at = now() WHERE id = $1")
            .bind(local_id)
            .bind(payload)
            .execute(pool)
            .await
    {
        tracing::warn!(%err, %local_id, "todo write failed");
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn extract_todos_reads_the_claude_todowrite_shape() {
        let got = extract_todos(
            "TodoWrite",
            &json!({"todos": [
                {"content": "parse it", "status": "completed", "activeForm": "Parsing it"},
                {"content": "wire it", "status": "in_progress", "activeForm": "Wiring the parser"},
                {"content": "ship it", "status": "pending", "activeForm": "Shipping it"},
            ]}),
        )
        .expect("TodoWrite yields a list");
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].content, "wire it");
        assert_eq!(got[1].status, "in_progress");
        assert_eq!(got[1].active_form.as_deref(), Some("Wiring the parser"));
    }

    #[test]
    fn extract_todos_reads_the_codex_update_plan_shape() {
        let got = extract_todos(
            "update_plan",
            &json!({"explanation": "why", "plan": [
                {"step": "read the code", "status": "completed"},
                {"step": "change the code", "status": "in_progress"},
            ]}),
        )
        .expect("update_plan yields a list");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].content, "read the code");
        assert_eq!(got[0].status, "completed");
        assert!(got[1].active_form.is_none());
    }

    #[test]
    fn extract_todos_ignores_other_tools_and_degrades_bad_input() {
        assert!(extract_todos("Read", &json!({"file_path": "/x"})).is_none());
        assert!(extract_todos("TodoWrite", &json!({})).is_none());
        assert!(extract_todos("TodoWrite", &json!({"todos": "nope"})).is_none());
        // An empty list is a real value: the agent cleared its todos.
        assert_eq!(extract_todos("TodoWrite", &json!({"todos": []})).unwrap().len(), 0);
        let got = extract_todos("TodoWrite", &json!({"todos": [{"content": "x"}]})).unwrap();
        assert_eq!(got[0].status, "pending");
        assert_eq!(got[0].content, "x");
    }

    #[test]
    fn a_todowrite_entry_serializes_exactly_as_before() {
        let got = extract_todos(
            "TodoWrite",
            &json!({"todos": [{"content": "a", "status": "in_progress", "activeForm": "A-ing"}]}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            json!([{"content": "a", "status": "in_progress", "active_form": "A-ing"}])
        );
    }

    fn tool_call(tool: &str, input: serde_json::Value) -> cctui_proto::ws::AgentEvent {
        cctui_proto::ws::AgentEvent::ToolCall {
            tool: tool.to_owned(),
            input,
            kind: None,
            tool_use_id: None,
            ts: 0,
            seq: None,
        }
    }

    fn tool_result(summary: &str, error: bool) -> cctui_proto::ws::AgentEvent {
        cctui_proto::ws::AgentEvent::ToolResult {
            tool: String::new(),
            output_summary: summary.to_owned(),
            kind: None,
            tool_use_id: None,
            error,
            ts: 0,
            seq: None,
        }
    }

    fn task(id: &str, content: &str, status: &str) -> TodoEntry {
        TodoEntry {
            content: content.to_owned(),
            status: status.to_owned(),
            id: Some(id.to_owned()),
            ..TodoEntry::default()
        }
    }

    #[test]
    fn task_op_reads_creations_from_results_and_updates_from_calls() {
        assert_eq!(
            task_op(&tool_result("Task #12 created successfully: ship: it", false)),
            Some(TaskOp::Created { id: "12".into(), subject: "ship: it".into() })
        );
        assert_eq!(task_op(&tool_result("Task #12 created successfully: x", true)), None);
        assert_eq!(task_op(&tool_result("Task #x created successfully: x", false)), None);
        assert_eq!(task_op(&tool_result("ok", false)), None);
        let input = json!({"taskId": "3", "status": "completed"});
        assert_eq!(
            task_op(&tool_call("TaskUpdate", input.clone())),
            Some(TaskOp::Updated { id: "3".into(), input })
        );
        assert!(matches!(
            task_op(&tool_call("TaskUpdate", json!({"taskId": 4}))),
            Some(TaskOp::Updated { id, .. }) if id == "4"
        ));
        assert_eq!(task_op(&tool_call("TaskUpdate", json!({"status": "completed"}))), None);
        assert_eq!(task_op(&tool_call("TaskCreate", json!({"subject": "s"}))), None);
        assert_eq!(task_op(&tool_call("TodoWrite", json!({"todos": []}))), None);
    }

    #[test]
    fn creations_fold_in_numeric_order_and_replace_a_todowrite_list() {
        let mut list = extract_todos("TodoWrite", &json!({"todos": [{"content": "old"}]})).unwrap();
        for (id, subject) in [("10", "ten"), ("9", "nine"), ("10", "ten again")] {
            fold_task(&mut list, TaskOp::Created { id: id.into(), subject: subject.into() });
        }
        assert_eq!(list, vec![task("9", "nine", "pending"), task("10", "ten", "pending")]);
    }

    #[test]
    fn updates_patch_the_task_and_deleted_removes_it() {
        let mut list = vec![task("1", "one", "pending"), task("2", "two", "pending")];
        fold_task(
            &mut list,
            TaskOp::Updated {
                id: "2".into(),
                input: json!({
                    "taskId": "2", "status": "in_progress", "subject": "two!",
                    "description": "why", "activeForm": "Doing two", "owner": "agent-a",
                    "addBlockedBy": ["1", "1"],
                }),
            },
        );
        let two = &list[1];
        assert_eq!(two.content, "two!");
        assert_eq!(two.status, "in_progress");
        assert_eq!(two.description.as_deref(), Some("why"));
        assert_eq!(two.active_form.as_deref(), Some("Doing two"));
        assert_eq!(two.owner.as_deref(), Some("agent-a"));
        assert_eq!(two.blocked_by, vec!["1".to_owned()]);
        let snapshot = list.clone();
        fold_task(
            &mut list,
            TaskOp::Updated { id: "2".into(), input: json!({"status": "in_progress"}) },
        );
        assert_eq!(list, snapshot, "an update the list already carries changes nothing");
        fold_task(
            &mut list,
            TaskOp::Updated { id: "1".into(), input: json!({"status": "deleted"}) },
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.as_deref(), Some("2"));
    }

    #[test]
    fn an_update_for_an_unknown_task_leaves_a_todowrite_list_alone() {
        let mut list =
            extract_todos("TodoWrite", &json!({"todos": [{"content": "keep"}]})).unwrap();
        let before = list.clone();
        fold_task(
            &mut list,
            TaskOp::Updated { id: "7".into(), input: json!({"status": "completed"}) },
        );
        assert_eq!(list, before);
    }

    #[test]
    fn a_todos_frame_is_scoped_to_its_session() {
        let frame = cctui_proto::ws::DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: cctui_proto::adapter::AdapterEvent::Todos {
                local_id: "sess-1".into(),
                todos: vec![task("1", "one", "pending")],
            },
        };
        assert_eq!(crate::routes::daemon::connection::session_scope(&frame), Some("sess-1"));
    }

    /// DB-gated: unlike the activity bump, a todos
    /// write must NOT roll up the `parent_id` chain. A subagent writing its own
    /// list must leave the parent's list untouched, or every parent row shows
    /// whatever its newest child happened to be doing.
    #[tokio::test]
    async fn subagent_todos_do_not_overwrite_the_parent_list() {
        let Some(url) = crate::routes::gateway::test_db_url("subagent_todos_stay_on_the_leaf")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'todo-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");

        let parent_id = Uuid::new_v4().to_string();
        let child_id = Uuid::new_v4().to_string();
        for (id, parent) in [(&parent_id, None::<&str>), (&child_id, Some(parent_id.as_str()))] {
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code')",
            )
            .bind(id)
            .bind(parent)
            .bind(machine.to_string())
            .bind(uid)
            .bind(machine)
            .execute(&pool)
            .await
            .expect("seed session");
        }

        let parent_todos =
            extract_todos("TodoWrite", &json!({"todos": [{"content": "parent task"}]})).unwrap();
        let child_todos = extract_todos(
            "TodoWrite",
            &json!({"todos": [{"content": "child task a"}, {"content": "child task b"}]}),
        )
        .unwrap();
        record_todos(&pool, &parent_id, &parent_todos).await;
        record_todos(&pool, &child_id, &child_todos).await;

        let read = |id: String| {
            let pool = pool.clone();
            async move {
                let (todos, at): (Option<serde_json::Value>, Option<chrono::DateTime<Utc>>) =
                    sqlx::query_as("SELECT todos, todo_updated_at FROM sessions WHERE id = $1")
                        .bind(&id)
                        .fetch_one(&pool)
                        .await
                        .expect("read todos");
                (
                    serde_json::from_value::<Vec<TodoEntry>>(todos.expect("todos written"))
                        .expect("todos decode"),
                    at,
                )
            }
        };

        let (parent_got, parent_at) = read(parent_id.clone()).await;
        let (child_got, child_at) = read(child_id.clone()).await;
        assert_eq!(parent_got, parent_todos, "child's write must not touch the parent's list");
        assert_eq!(child_got, child_todos);
        assert!(parent_at.is_some() && child_at.is_some());

        sqlx::query("DELETE FROM sessions WHERE id = ANY($1)")
            .bind(vec![parent_id, child_id])
            .execute(&pool)
            .await
            .expect("cleanup sessions");
        sqlx::query("DELETE FROM machines WHERE id = $1")
            .bind(machine)
            .execute(&pool)
            .await
            .expect("cleanup machine");
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .expect("cleanup user");
    }
}
