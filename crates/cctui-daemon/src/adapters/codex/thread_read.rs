//! Structured codex history via `thread/read` + `thread/turns/list`.
//!
//! The primary transcript source for a thread cctui did not drive itself.
//! [`super::log_tail`]'s permissive JSONL scrape of `~/.codex/sessions/**` is
//! the fallback for threads the server will not serve (deleted rollout, older
//! server, probe failure) — the app-server hands back the same `ThreadItem`
//! objects the live `item/completed` notifications carry, so history and live
//! events normalize through one code path instead of two.
//!
//! `thread/items/list` is deliberately not used: it answers `-32601 not
//! supported yet` at the pinned floor.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::adapter::AdapterEvent;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;

use super::app_server::AppServerConfig;
use super::thread_list::{OptionStdioExt as _, initialize_req, read_response_for, write_line};

/// Threads whose transcript came back from `thread/turns/list`. Shared with
/// [`super::log_tail`] so the JSONL scrape does not re-ingest them.
pub type ServedIds = Arc<tokio::sync::Mutex<HashSet<String>>>;

/// Upper bound on turn pages followed for one thread.
const MAX_PAGES: usize = 50;

/// Turn page size requested from `thread/turns/list`.
const TURN_PAGE: u32 = 100;

/// Wall-clock budget for one thread's history read.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// `thread/read` with `includeTurns: false`. Full-history hydration through
/// this call is deprecated for paginated threads; the turns come from
/// `thread/turns/list` instead.
#[must_use]
pub fn thread_read_req(id: i64, thread_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "thread/read",
        "params": {"threadId": thread_id, "includeTurns": false},
    })
}

/// `thread/turns/list`.
///
/// `itemsView: "full"` is required — the default `summary` view omits the item
/// bodies the transcript is made of. Ascending order so the emitted events are
/// already in conversation order.
#[must_use]
pub fn turns_list_req(id: i64, thread_id: &str, cursor: Option<&str>) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_owned(), json!(thread_id));
    params.insert("limit".to_owned(), json!(TURN_PAGE));
    params.insert("itemsView".to_owned(), json!("full"));
    params.insert("sortDirection".to_owned(), json!("ascending"));
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), json!(cursor));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "thread/turns/list", "params": params})
}

#[must_use]
fn next_cursor(result: &Value) -> Option<String> {
    result.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned)
}

/// Flatten `thread/turns/list` pages into the thread's `ThreadItem`s, in the
/// order the server returned them.
#[must_use]
pub fn items_from_turns(pages: &[Value]) -> Vec<Value> {
    pages
        .iter()
        .filter_map(|p| p.get("data").and_then(Value::as_array))
        .flatten()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect()
}

/// Project structured history items onto adapter events, using the same
/// item-type split as the live `item/completed` path so a replayed transcript
/// and a live turn render identically.
#[must_use]
pub fn history_events(local_id: &str, items: &[Value]) -> Vec<AdapterEvent> {
    items.iter().map(|item| super::app_server::item_event(local_id, item)).collect()
}

/// Read one thread's structured history. `Ok(vec![])` means the server served
/// the thread but it has no items; `Err` means the caller should fall back to
/// the JSONL tail.
pub async fn read_history(
    app: &AppServerConfig,
    thread_id: &str,
) -> anyhow::Result<(Value, Vec<Value>)> {
    let mut cmd = Command::new(&app.bin);
    cmd.arg("app-server")
        .arg("-c")
        .arg(format!("sandbox_mode=\"{}\"", app.sandbox_mode))
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().context_stdin()?;
    let stdout = child.stdout.take().context_stdout()?;

    let out = tokio::time::timeout(READ_TIMEOUT, async {
        let mut lines = BufReader::new(stdout).lines();
        write_line(&mut stdin, &initialize_req()).await?;

        write_line(&mut stdin, &thread_read_req(2, thread_id)).await?;
        let meta = read_response_for(&mut lines, 2, "thread/read").await?;

        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let req_id = 3 + i64::try_from(page).unwrap_or(i64::MAX);
            write_line(&mut stdin, &turns_list_req(req_id, thread_id, cursor.as_deref())).await?;
            let result = read_response_for(&mut lines, req_id, "thread/turns/list").await?;
            let next = next_cursor(&result);
            pages.push(result);
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        anyhow::Ok((meta, items_from_turns(&pages)))
    })
    .await;

    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;

    out.map_err(|_| anyhow::anyhow!("thread/read timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shapes_match_the_pinned_schema() {
        let read = thread_read_req(2, "t1");
        assert_eq!(read["method"], "thread/read");
        assert_eq!(read["params"]["threadId"], "t1");
        assert_eq!(read["params"]["includeTurns"], false);

        let turns = turns_list_req(3, "t1", None);
        assert_eq!(turns["method"], "thread/turns/list");
        assert_eq!(turns["params"]["itemsView"], "full");
        assert_eq!(turns["params"]["sortDirection"], "ascending");
        assert!(turns["params"].get("cursor").is_none());

        let paged = turns_list_req(4, "t1", Some("c2"));
        assert_eq!(paged["params"]["cursor"], "c2");
    }

    #[test]
    fn turns_flatten_in_order_across_pages() {
        let pages = vec![
            json!({"data": [
                {"id": "u1", "status": "completed", "items": [
                    {"type": "userMessage", "id": "i1"},
                    {"type": "agentMessage", "id": "i2", "text": "a"},
                ]},
            ], "nextCursor": "c"}),
            json!({"data": [
                {"id": "u2", "status": "completed", "items": [
                    {"type": "commandExecution", "id": "i3", "command": "ls"},
                ]},
            ], "nextCursor": null}),
        ];
        let items = items_from_turns(&pages);
        let ids: Vec<&str> =
            items.iter().filter_map(|i| i.get("id").and_then(Value::as_str)).collect();
        assert_eq!(ids, ["i1", "i2", "i3"]);
    }

    #[test]
    fn empty_and_malformed_pages_flatten_to_nothing() {
        assert!(items_from_turns(&[json!({"data": []})]).is_empty());
        assert!(items_from_turns(&[json!({})]).is_empty());
        assert!(items_from_turns(&[json!({"data": [{"id": "u1"}]})]).is_empty());
    }

    #[test]
    fn history_items_split_into_tool_use_and_message_like_the_live_path() {
        let items = vec![
            json!({"type": "agentMessage", "id": "i1", "text": "hi"}),
            json!({"type": "commandExecution", "id": "i2", "command": "ls"}),
        ];
        let events = history_events("t1", &items);
        assert!(matches!(events[0], AdapterEvent::Message { .. }));
        match &events[1] {
            AdapterEvent::ToolUse { local_id, .. } => assert_eq!(local_id, "t1"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn cursor_stops_on_null_or_empty() {
        assert_eq!(next_cursor(&json!({"nextCursor": "c"})), Some("c".to_owned()));
        assert_eq!(next_cursor(&json!({"nextCursor": null})), None);
        assert_eq!(next_cursor(&json!({"nextCursor": ""})), None);
        assert_eq!(next_cursor(&json!({})), None);
    }
}
