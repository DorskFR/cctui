//! Agent voice notes: `CctuiSpeak` text synthesized to opus, stored per
//! session and shown in the conversation as a `voice` text event.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use cctui_proto::ws::ServerEvent;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::server_settings::{speech_client, speech_error};
use crate::state::AppState;

pub const MAX_NOTE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_NOTES_PER_SESSION: i64 = 500;
pub const MAX_NOTE_BYTES_PER_SESSION: i64 = 100 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 4096;

const OPUS_GRANULE_RATE: f64 = 48_000.0;

#[derive(Debug, Deserialize)]
pub struct SpeakRequest {
    pub text: String,
    #[serde(default)]
    pub voice: Option<String>,
}

/// Playback length of an Ogg Opus stream: the last page's granule position
/// minus the `OpusHead` pre-skip, at Opus's fixed 48 kHz granule rate.
#[must_use]
pub fn ogg_opus_duration_ms(bytes: &[u8]) -> Option<u32> {
    let mut pos = 0;
    let mut last_granule = None;
    let mut pre_skip = 0u64;
    let mut first = true;
    while bytes.len() >= pos + 27 && &bytes[pos..pos + 4] == b"OggS" {
        let granule = i64::from_le_bytes(bytes[pos + 6..pos + 14].try_into().ok()?);
        let segments = usize::from(bytes[pos + 26]);
        let table = bytes.get(pos + 27..pos + 27 + segments)?;
        let body_len: usize = table.iter().map(|&b| usize::from(b)).sum();
        let body_start = pos + 27 + segments;
        if first {
            let body = bytes.get(body_start..body_start + body_len)?;
            if body.len() >= 12 && body.starts_with(b"OpusHead") {
                pre_skip = u64::from(u16::from_le_bytes([body[10], body[11]]));
            }
            first = false;
        }
        if granule >= 0 {
            last_granule = Some(granule.cast_unsigned());
        }
        pos = body_start + body_len;
    }
    let samples = last_granule?.saturating_sub(pre_skip);
    u32::try_from((samples as f64 / OPUS_GRANULE_RATE * 1000.0).round() as u64).ok()
}

/// Persist a note and its `voice` stream event for a session owned by
/// `user_id`. Returns the note id, the event and its `seq`.
pub async fn store_note(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    session_id: &str,
    text: &str,
    audio: &[u8],
    duration_ms: Option<u32>,
) -> Result<(Uuid, Value, i64), AppError> {
    if audio.len() > MAX_NOTE_BYTES {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("voice note is {} bytes; per-note cap is {MAX_NOTE_BYTES}", audio.len()),
        ));
    }
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    match owner {
        None => return Err(AppError::new(StatusCode::NOT_FOUND, "session not found")),
        Some(o) if o != user_id => {
            return Err(AppError::new(StatusCode::FORBIDDEN, "not your session"));
        }
        Some(_) => {}
    }
    let (count, total): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM session_voice_notes \
         WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let byte_len = i64::try_from(audio.len()).unwrap_or(i64::MAX);
    if count >= MAX_NOTES_PER_SESSION || total + byte_len > MAX_NOTE_BYTES_PER_SESSION {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "session voice note quota exceeded",
        ));
    }
    let duration = duration_ms.map(i64::from);
    let mut tx = pool.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO session_voice_notes (session_id, media_type, byte_len, duration_ms, text, bytes) \
         VALUES ($1, 'audio/ogg', $2, $3, $4, $5) RETURNING id",
    )
    .bind(session_id)
    .bind(byte_len)
    .bind(duration)
    .bind(text)
    .bind(audio)
    .fetch_one(&mut *tx)
    .await?;
    let payload = json!({
        "role": "assistant_voice",
        "text": text,
        "message_id": id.to_string(),
        "duration_ms": duration,
    });
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         VALUES ($1, 'message', $2) RETURNING id",
    )
    .bind(session_id)
    .bind(&payload)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((id, payload, seq))
}

pub async fn create_voice_note(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<SpeakRequest>,
) -> Result<Json<Value>, AppError> {
    let user_id = crate::routes::images::machine_user(&state, &headers).await?;
    let text = req.text.trim();
    if text.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "text is required"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("text is longer than {MAX_TEXT_CHARS} characters"),
        ));
    }
    let voice = req.voice.as_deref().map(str::trim).filter(|v| !v.is_empty());
    let client = speech_client(&state).await?;
    let audio =
        client.synthesize(text, voice, Some("opus"), None).await.map_err(|e| speech_error(&e))?;
    let duration_ms = ogg_opus_duration_ms(&audio);
    let (id, payload, seq) =
        store_note(&state.pool, user_id, &session_id, text, &audio, duration_ms).await?;
    if let Some(mut data) = crate::normalize::to_agent_event("", "message", &payload) {
        data.set_seq(seq);
        state.bus.publish_server(ServerEvent::Stream { session_id, data });
    }
    Ok(Json(json!({
        "ok": true,
        "note_id": id,
        "duration_s": duration_ms.map(|ms| f64::from(ms) / 1000.0),
    })))
}

pub async fn get_voice_note(
    State(state): State<AppState>,
    Path((session_id, note_id)): Path<(String, Uuid)>,
) -> Result<Response, AppError> {
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT media_type, bytes FROM session_voice_notes WHERE id = $1 AND session_id = $2",
    )
    .bind(note_id)
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await?;
    let (media_type, bytes) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "voice note not found"))?;
    Ok((
        [
            (header::CONTENT_TYPE, media_type),
            (header::CACHE_CONTROL, "private, max-age=31536000, immutable".to_owned()),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(granule: i64, body: &[u8]) -> Vec<u8> {
        let mut p = b"OggS".to_vec();
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&granule.to_le_bytes());
        p.extend_from_slice(&[0; 12]);
        p.push(1);
        p.push(u8::try_from(body.len()).unwrap());
        p.extend_from_slice(body);
        p
    }

    #[test]
    fn duration_is_the_last_granule_minus_pre_skip() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 1]);
        head.extend_from_slice(&312u16.to_le_bytes());
        let mut ogg = page(0, &head);
        ogg.extend(page(0, b"OpusTags"));
        ogg.extend(page(-1, b"partial"));
        ogg.extend(page(48_000 * 3 + 312, b"audio"));
        assert_eq!(ogg_opus_duration_ms(&ogg), Some(3000));
        assert_eq!(ogg_opus_duration_ms(b"ID3 not ogg"), None);
    }

    async fn test_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn seed(pool: &sqlx::PgPool) -> (Uuid, String) {
        let uid = Uuid::new_v4();
        let sid = format!("voice-{}", Uuid::new_v4().simple());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("voice-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id) VALUES ($1, 'm1', '/w', $2)",
        )
        .bind(&sid)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        (uid, sid)
    }

    #[tokio::test]
    async fn a_note_persists_a_voice_event_and_respects_ownership_and_caps() {
        let Some(pool) = test_pool("a_note_persists_a_voice_event").await else { return };
        let (uid, sid) = seed(&pool).await;

        let (id, _, seq) =
            store_note(&pool, uid, &sid, "hello", b"OggS..", Some(1500)).await.unwrap();
        let payload: Value = sqlx::query_scalar("SELECT payload FROM stream_events WHERE id = $1")
            .bind(seq)
            .fetch_one(&pool)
            .await
            .unwrap();
        let client = crate::normalize::for_client("claude-code", "message", payload).unwrap();
        assert_eq!(client["kind"], "voice");
        assert_eq!(client["content"], "hello");
        assert_eq!(client["message_id"], id.to_string());

        let foreign = store_note(&pool, Uuid::new_v4(), &sid, "x", b"OggS", None).await;
        assert_eq!(foreign.unwrap_err().status(), StatusCode::FORBIDDEN);
        let missing = store_note(&pool, uid, "no-such-session", "x", b"OggS", None).await;
        assert_eq!(missing.unwrap_err().status(), StatusCode::NOT_FOUND);
        let big = vec![0u8; MAX_NOTE_BYTES + 1];
        let too_big = store_note(&pool, uid, &sid, "x", &big, None).await;
        assert_eq!(too_big.unwrap_err().status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
