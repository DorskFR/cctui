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
            })
            .collect(),
    )
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
