//! Shared completion classification for result polling.
use crate::store::sessions::SessionRowStatus;
use cctui_proto::adapter::SessionChild;
use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, classify};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Classify a session from its persisted signals.
///
/// `children`
/// are the session's linked-PR references (`sessions.children`); the `Review`
/// bucket arises when one of them has an OPEN PR awaiting attention per the
/// `pr_cache` the GitHub connector maintains (empty cache degrades to no Review).
fn bucket_from_signals(
    tempo: Option<&str>,
    agent_state: Option<&str>,
    activity: Option<&str>,
    soft_limit_blocked: Option<&str>,
    children: &[SessionChild],
    pr_cache: &std::collections::HashMap<String, PrStatus<'_>>,
) -> Bucket {
    let input = ClassifyInput {
        tempo,
        state: agent_state,
        activity,
        children,
        q: None,
        soft_limit_blocked,
    };
    classify(&input, pr_cache)
}

#[derive(sqlx::FromRow)]
pub struct ResultRow {
    pub adapter_id: Option<String>,
    pub end_reason: Option<String>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: String,
    pub tempo: Option<String>,
    pub agent_state: Option<String>,
    pub activity: Option<String>,
    pub soft_limit_reason: Option<String>,
}

impl ResultRow {
    /// Ended, archived, or idle after a finished turn: the bucket the list
    /// files as `Done`, never while the adapter still reports the turn busy.
    #[must_use]
    pub fn done(&self) -> bool {
        if self.ended_at.is_some() || self.status == SessionRowStatus::Archived.as_str() {
            return true;
        }
        if matches!(self.adapter_id.as_deref(), Some("codex" | "opencode")) {
            return self.idle_after_turn();
        }
        let busy = matches!(self.tempo.as_deref(), Some("active" | "blocked"))
            || matches!(self.agent_state.as_deref(), Some("working" | "running"));
        let bucket = bucket_from_signals(
            self.tempo.as_deref(),
            self.agent_state.as_deref(),
            self.activity.as_deref(),
            self.soft_limit_reason.as_deref(),
            &[],
            &HashMap::new(),
        );
        bucket == Bucket::Done && !busy
    }

    /// codex keeps `tempo` at its last busy value after the turn goes idle,
    /// so only `agent_state` is current for these adapters.
    fn idle_after_turn(&self) -> bool {
        self.soft_limit_reason.is_none()
            && matches!(self.agent_state.as_deref(), Some("idle" | "done"))
    }
}

/// A bounded final-message projection; the full host reads its transcript instead.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct CapturedResult {
    pub message_id: Option<String>,
    pub truncated: bool,
    open: bool,
    parts: Vec<(String, String)>,
}
impl CapturedResult {
    pub fn observe(&mut self, value: &serde_json::Value) {
        let field = |key| value.get(key).and_then(serde_json::Value::as_str);
        let id = field("message_id").map(str::to_owned);
        let candidate = field("type") == Some("text")
            && field("kind").is_none()
            && value.get("meta").and_then(serde_json::Value::as_bool) != Some(true)
            && (id.is_some() || field("role") == Some("Assistant"));
        if !candidate {
            if id.is_none() || id != self.message_id {
                self.open = false;
            }
            return;
        }
        if !self.open || id != self.message_id {
            *self = Self { message_id: id, open: true, ..Self::default() };
        }
        let hash = crate::auth::sha256_hex(&value.to_string());
        if self.parts.iter().any(|(digest, _)| digest == &hash) {
            return;
        }
        let text = field("content").unwrap_or_default();
        let used: usize = self.parts.iter().map(|(_, part)| part.len() + 2).sum();
        let remaining = (64 * 1024_usize).saturating_sub(used);
        if self.parts.len() >= 200 || remaining == 0 {
            self.truncated = true;
            return;
        }
        let mut end = text.len().min(remaining);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.truncated |= end < text.len();
        self.parts.push((hash, text[..end].to_owned()));
    }
    #[must_use]
    pub fn text(&self) -> Option<String> {
        (!self.parts.is_empty()).then(|| {
            self.parts.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join("\n\n")
        })
    }
}

/// Store only the owned run's bounded final assistant message.
pub async fn capture_message(
    pool: &sqlx::PgPool,
    machine: uuid::Uuid,
    user: uuid::Uuid,
    session: &str,
    adapter: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let Some(value) = crate::normalize::for_client(adapter, "message", payload) else {
        return Ok(());
    };
    let mut tx = pool.begin().await?;
    let metadata: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM sessions WHERE id=$1 AND machine_uuid=$2 AND user_id=$3 FOR UPDATE",
    )
    .bind(session)
    .bind(machine)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(metadata) = metadata else {
        return Ok(());
    };
    let mut result: CapturedResult = metadata
        .get("cctui_execution_result")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    result.observe(&value);
    sqlx::query("UPDATE sessions SET metadata=COALESCE(metadata,'{}'::jsonb) || jsonb_build_object('cctui_execution_result',$2::jsonb),last_heartbeat=now() WHERE id=$1")
        .bind(session).bind(serde_json::to_value(result).expect("capture contains strings and booleans")).execute(&mut *tx).await?;
    tx.commit().await
}

/// Read a durable dispatch before its worker registers a session.
///
/// Ownership is checked on the dispatch record. Queue position follows the
/// existing dispatcher/profile ordering used by the full server.
pub async fn unregistered_result(
    pool: &sqlx::PgPool,
    session_id: &str,
    owner: Option<uuid::Uuid>,
) -> Result<Option<cctui_proto::api::SessionResult>, sqlx::Error> {
    let row: Option<(String, Option<String>, bool, Option<i64>)> = sqlx::query_as(
        "SELECT r.outcome, r.end_reason, r.queued, \
                CASE WHEN r.queued AND r.outcome = 'forwarded' THEN \
                    (SELECT count(*) FROM dispatch_records q \
                     WHERE q.queued AND q.outcome = 'forwarded' \
                       AND q.dispatcher_name = r.dispatcher_name \
                       AND q.profile IS NOT DISTINCT FROM r.profile \
                       AND (q.created_at, q.session_id) <= (r.created_at, r.session_id)) \
                ELSE NULL END \
         FROM dispatch_records r \
         WHERE r.session_id = $1 AND ($2::uuid IS NULL OR r.user_id = $2)",
    )
    .bind(session_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    let Some((outcome, end_reason, queued, position)) = row else { return Ok(None) };
    if !matches!(outcome.as_str(), "pending" | "forwarded" | "refused" | "failed") {
        return Ok(None);
    }
    let done = matches!(outcome.as_str(), "refused" | "failed");
    let queued = !done && outcome == "forwarded" && queued;
    Ok(Some(cctui_proto::api::SessionResult {
        done,
        end_reason: if done {
            Some(end_reason.unwrap_or_else(|| "spawn_failed".into()))
        } else {
            None
        },
        final_assistant_text: None,
        message_id: None,
        token_usage: None,
        queued,
        queue_position: if queued { position.and_then(|p| u32::try_from(p).ok()) } else { None },
    }))
}

#[cfg(test)]
mod tests {
    use super::CapturedResult;
    use serde_json::json;

    #[test]
    fn final_message_joins_visible_parts_and_replay_does_not_duplicate_them() {
        let mut capture = CapturedResult::default();
        for text in ["first", "second", "first", "second"] {
            capture.observe(&json!({"type":"text","content":text,"message_id":"answer"}));
        }
        capture.observe(&json!({"type":"text","content":"private reasoning","kind":"thinking","message_id":"answer"}));
        assert_eq!(capture.text().as_deref(), Some("first\n\nsecond"));
        capture.observe(&json!({"type":"text","content":"next answer","message_id":"next"}));
        assert_eq!(capture.text().as_deref(), Some("next answer"));
    }
    #[test]
    fn oversized_results_are_bounded_at_utf8_boundaries_and_flagged() {
        let mut capture = CapturedResult::default();
        capture.observe(&json!({"type":"text","content":"界".repeat(30_000),"role":"Assistant"}));
        let text = capture.text().unwrap();
        assert!(text.len() <= 64 * 1024);
        assert!(capture.truncated);
        assert!(text.chars().all(|c| c == '界'));
        let restored: CapturedResult =
            serde_json::from_value(serde_json::to_value(&capture).unwrap()).unwrap();
        assert_eq!(restored.text(), capture.text());
        assert!(restored.truncated);
    }
}
