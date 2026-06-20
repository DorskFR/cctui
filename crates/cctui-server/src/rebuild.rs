//! Server-side transcript rebuild from stored `stream_events` (CCT-363).
//!
//! Ephemeral k8s workers and any host without the sync daemon (CCT-362) never
//! upload their on-disk transcripts, and the historical `/archive` PVC can't be
//! re-fetched. But we persist `stream_events` (the normalized-ish adapter
//! payloads) plus session metadata, so we can reconstruct an *approximate*
//! transcript and drop it into the same [`ArchiveStore`] the synced files live
//! in — flagged `source = rebuilt` so consumers (export, CCT-364) can prefer a
//! byte-exact synced file when both exist.
//!
//! Fidelity gap: this is **lossy**. We store canonicalized payloads, not the
//! verbatim file, so the rebuilt `.jsonl` approximates — not reproduces — the
//! coach's on-disk schema. It is the fallback, not the primary path.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::archive_store::{ArchiveStore, Stats};

/// Encode a working directory into the `~/.claude/projects/<encoded>` path
/// segment Claude uses: replace every `/` and `.` with `-`, dropping a trailing
/// slash first. Mirrors the daemon's `transcript::encode_cwd` (kept in sync by
/// hand — the two crates don't share this helper).
fn encode_cwd(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    #[error("session not found")]
    NotFound,
    #[error("session machine_id is not a uuid")]
    BadMachineId,
    #[error("session has no stream_events to rebuild from")]
    Empty,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Archive(#[from] crate::archive_store::ArchiveError),
}

/// Reconstruct a transcript for `session_id` from `stream_events`, write it into
/// the archive store, and upsert `archive_index` with `source = 'rebuilt'`.
/// Returns the written file's [`Stats`] plus its resolved `(machine_id,
/// project_dir)` so the caller can broadcast / report.
pub async fn rebuild_session(
    pool: &sqlx::PgPool,
    archive: &Arc<ArchiveStore>,
    session_id: &str,
) -> Result<(Uuid, String, Stats), RebuildError> {
    let session: Option<(String, String, String, Value)> = sqlx::query_as(
        "SELECT machine_id, working_dir, adapter_id, metadata FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    let (machine_id_text, working_dir, adapter_id, metadata) =
        session.ok_or(RebuildError::NotFound)?;

    let machine_id = Uuid::parse_str(&machine_id_text).map_err(|_| RebuildError::BadMachineId)?;
    let project_dir = encode_cwd(&working_dir);

    let rows: Vec<(String, Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT event_type, payload, created_at FROM stream_events \
         WHERE session_id = $1 ORDER BY created_at ASC, id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(RebuildError::Empty);
    }

    let model = metadata.get("model").and_then(Value::as_str).unwrap_or("unknown").to_owned();
    let bytes = if adapter_id == "codex" {
        render_codex(session_id, &working_dir, &rows)
    } else {
        render_claude(session_id, &working_dir, &model, &rows)
    };

    // Reuse the same hashing/atomic-rename writer the upload path uses.
    let stats = archive.write(machine_id, &project_dir, session_id, bytes.as_slice()).await?;

    sqlx::query(
        "INSERT INTO archive_index \
         (machine_id, project_dir, session_id, sha256, size_bytes, line_count, source) \
         VALUES ($1,$2,$3,$4,$5,$6,'rebuilt') \
         ON CONFLICT (machine_id, session_id) DO UPDATE SET \
             sha256 = EXCLUDED.sha256, size_bytes = EXCLUDED.size_bytes, \
             line_count = EXCLUDED.line_count, uploaded_at = now(), \
             project_dir = EXCLUDED.project_dir, source = 'rebuilt'",
    )
    .bind(machine_id)
    .bind(&project_dir)
    .bind(session_id)
    .bind(&stats.sha256)
    .bind(i64::try_from(stats.size_bytes).unwrap_or(i64::MAX))
    .bind(i32::try_from(stats.line_count).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;

    Ok((machine_id, project_dir, stats))
}

/// Emit lines approximating the Claude on-disk schema the coach parses:
/// `{type, uuid, parentUuid, sessionId, cwd, timestamp, isSidechain, message}`.
fn render_claude(
    session_id: &str,
    cwd: &str,
    model: &str,
    rows: &[(String, Value, DateTime<Utc>)],
) -> Vec<u8> {
    let mut out = String::new();
    let mut parent: Option<String> = None;

    for (event_type, payload, created_at) in rows {
        let ts = created_at.to_rfc3339();
        let line = match event_type.as_str() {
            "message" => {
                let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
                match role {
                    "user" => Some(claude_envelope(
                        "user",
                        &mut parent,
                        session_id,
                        cwd,
                        &ts,
                        json!({ "role": "user", "content": [{ "type": "text", "text": text }] }),
                    )),
                    "assistant" | "assistant_thinking" => Some(claude_envelope(
                        "assistant",
                        &mut parent,
                        session_id,
                        cwd,
                        &ts,
                        json!({
                            "role": "assistant",
                            "model": model,
                            "content": [{ "type": "text", "text": text }],
                        }),
                    )),
                    _ => None,
                }
            }
            "tool_use" => {
                if payload.get("kind").and_then(Value::as_str) == Some("tool_result") {
                    let summary = payload
                        .get("content")
                        .and_then(|v| v.as_str().map(str::to_owned).or_else(|| Some(v.to_string())))
                        .unwrap_or_default();
                    Some(claude_envelope(
                        "user",
                        &mut parent,
                        session_id,
                        cwd,
                        &ts,
                        json!({
                            "role": "user",
                            "content": [{ "type": "tool_result", "content": summary }],
                        }),
                    ))
                } else {
                    let tool = payload.get("tool").and_then(Value::as_str).unwrap_or_default();
                    let input = payload.get("input").cloned().unwrap_or(Value::Null);
                    Some(claude_envelope(
                        "assistant",
                        &mut parent,
                        session_id,
                        cwd,
                        &ts,
                        json!({
                            "role": "assistant",
                            "model": model,
                            "content": [{
                                "type": "tool_use",
                                "id": Uuid::new_v4().to_string(),
                                "name": tool,
                                "input": input,
                            }],
                        }),
                    ))
                }
            }
            _ => None,
        };
        if let Some(v) = line {
            out.push_str(&v.to_string());
            out.push('\n');
        }
    }
    out.into_bytes()
}

// `message` is moved into the `json!` envelope below; the pedantic
// pass-by-value lint misfires on the macro expansion.
#[allow(clippy::needless_pass_by_value)]
fn claude_envelope(
    kind: &str,
    parent: &mut Option<String>,
    session_id: &str,
    cwd: &str,
    ts: &str,
    message: Value,
) -> Value {
    let uuid = Uuid::new_v4().to_string();
    let v = json!({
        "type": kind,
        "uuid": uuid,
        "parentUuid": parent.clone(),
        "sessionId": session_id,
        "cwd": cwd,
        "timestamp": ts,
        "isSidechain": false,
        "message": message,
    });
    *parent = Some(uuid);
    v
}

/// Emit a codex-shaped rollout: a leading `session_meta`, then each stored
/// payload wrapped as a `response_item`. Lossy but ingestable.
fn render_codex(session_id: &str, cwd: &str, rows: &[(String, Value, DateTime<Utc>)]) -> Vec<u8> {
    let mut out = String::new();
    out.push_str(
        &json!({
            "type": "session_meta",
            "payload": { "id": session_id, "cwd": cwd },
        })
        .to_string(),
    );
    out.push('\n');
    for (_event_type, payload, created_at) in rows {
        out.push_str(
            &json!({
                "type": "response_item",
                "timestamp": created_at.to_rfc3339(),
                "payload": payload,
            })
            .to_string(),
        );
        out.push('\n');
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-20T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn claude_rebuild_chains_parent_uuids_and_shapes_messages() {
        let rows = vec![
            ("message".to_owned(), json!({ "role": "user", "text": "hi" }), ts()),
            ("message".to_owned(), json!({ "role": "assistant", "text": "hello" }), ts()),
        ];
        let bytes = render_claude("sess-1", "/home/u/p", "claude-opus", &rows);
        let lines: Vec<Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["type"], "user");
        assert_eq!(lines[0]["parentUuid"], Value::Null);
        assert_eq!(lines[0]["message"]["content"][0]["text"], "hi");
        assert_eq!(lines[1]["type"], "assistant");
        assert_eq!(lines[1]["message"]["model"], "claude-opus");
        // assistant's parent is the user's uuid
        assert_eq!(lines[1]["parentUuid"], lines[0]["uuid"]);
    }

    #[test]
    fn codex_rebuild_emits_session_meta_then_items() {
        let rows =
            vec![("message".to_owned(), json!({ "type": "agentMessage", "text": "x" }), ts())];
        let bytes = render_codex("sess-2", "/tmp", &rows);
        let lines: Vec<Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["type"], "session_meta");
        assert_eq!(lines[1]["type"], "response_item");
    }
}
