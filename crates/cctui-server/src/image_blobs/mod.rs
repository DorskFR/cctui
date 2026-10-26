//! Base64 sources in stream event payloads live once in `daemon_blobs`; the
//! payload keeps the daemon's [`BlobRef`] marker, so `session_blob_links` and
//! the blob route serve them.

pub mod backfill;
pub mod recover;
pub mod validate;

use std::collections::HashMap;

use base64::Engine;
use cctui_proto::adapter::AdapterEvent;
use cctui_proto::blob::{BLOB_SOURCE_TYPE, BlobRef};
use cctui_proto::media::sniff_media_type;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::routes::blobs::MAX_BLOB_BYTES;

pub const MIN_BLOB_BYTES: usize = 1024;

pub const INLINE_IMAGES_MAX_BYTES: usize = 8 * 1024 * 1024;

pub struct Extracted {
    pub hash: String,
    pub bytes: Vec<u8>,
    pub corrupt: Option<&'static str>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub valid: u64,
    pub corrupt: u64,
    pub recovered: u64,
}

impl Tally {
    #[must_use]
    pub fn of(blobs: &[Extracted], recovered: u64) -> Self {
        let corrupt = blobs.iter().filter(|b| b.corrupt.is_some()).count() as u64;
        Self { valid: blobs.len() as u64 - corrupt, corrupt, recovered }
    }

    pub const fn add(&mut self, other: Self) {
        self.valid += other.valid;
        self.corrupt += other.corrupt;
        self.recovered += other.recovered;
    }
}

fn is_candidate(map: &Map<String, Value>) -> bool {
    map.get("type").and_then(Value::as_str) == Some("base64")
        && map
            .get("data")
            .and_then(Value::as_str)
            .is_some_and(|d| d.len() / 4 * 3 >= MIN_BLOB_BYTES)
}

fn contains_candidate(v: &Value) -> bool {
    match v {
        Value::Object(map) => is_candidate(map) || map.values().any(contains_candidate),
        Value::Array(items) => items.iter().any(contains_candidate),
        _ => false,
    }
}

/// A ref with `"encoding": "base64"` names a blob holding the source's base64
/// text verbatim, kept because it is not a valid image (`corrupt_reason` says
/// why): it is never served as one.
const OPAQUE_ENCODING: &str = "base64";

fn ref_map(hash: &str, size: u64, media_type: Option<String>) -> Option<Map<String, Value>> {
    match serde_json::to_value(BlobRef::new(hash, size, media_type)) {
        Ok(Value::Object(r)) => Some(r),
        _ => None,
    }
}

fn mark_corrupt(r: &mut Map<String, Value>, reason: &str) {
    r.insert("encoding".to_owned(), Value::from(OPAQUE_ENCODING));
    r.insert("corrupt".to_owned(), Value::Bool(true));
    r.insert("corrupt_reason".to_owned(), Value::from(reason));
}

fn decode(map: &Map<String, Value>) -> Option<(Extracted, Map<String, Value>)> {
    let data = map.get("data").and_then(Value::as_str)?;
    let declared = map.get("media_type").and_then(Value::as_str);
    let (bytes, media_type, corrupt) = match validate::check(declared, data) {
        Ok(v) => (v.bytes, v.media_type, None),
        Err(reason) => (data.as_bytes().to_vec(), declared.map(str::to_owned), Some(reason)),
    };
    if !(MIN_BLOB_BYTES..=MAX_BLOB_BYTES).contains(&bytes.len()) {
        return None;
    }
    let hash = hex::encode(Sha256::digest(&bytes));
    let mut r = ref_map(&hash, bytes.len() as u64, media_type)?;
    if let Some(reason) = corrupt {
        mark_corrupt(&mut r, reason);
    }
    Some((Extracted { hash, bytes, corrupt }, r))
}

fn rewrite(v: &mut Value, out: &mut Vec<Extracted>) {
    match v {
        Value::Object(map) => {
            if is_candidate(map) {
                if let Some((blob, r)) = decode(map) {
                    if !out.iter().any(|b| b.hash == blob.hash) {
                        out.push(blob);
                    }
                    *map = r;
                }
                return;
            }
            map.values_mut().for_each(|x| rewrite(x, out));
        }
        Value::Array(items) => items.iter_mut().for_each(|x| rewrite(x, out)),
        _ => {}
    }
}

#[must_use]
pub fn extract(payload: &Value) -> Option<(Value, Vec<Extracted>)> {
    if !contains_candidate(payload) {
        return None;
    }
    let mut out = payload.clone();
    let mut blobs = Vec::new();
    rewrite(&mut out, &mut blobs);
    (!blobs.is_empty()).then_some((out, blobs))
}

/// [`extract`] fully decodes every image, so it runs off the async workers.
pub async fn extract_blocking(payload: &Value) -> Option<(Value, Vec<Extracted>)> {
    if !contains_candidate(payload) {
        return None;
    }
    let copy = payload.clone();
    tokio::task::spawn_blocking(move || extract(&copy)).await.unwrap_or_else(|err| {
        tracing::warn!(%err, "image extraction failed; payload kept inline");
        None
    })
}

fn retarget(v: &mut Value, from: &str, to: &recover::Intact) {
    match v {
        Value::Object(map) => {
            if blob_ref(map) == Some(from) {
                if let Some(mut r) = ref_map(&to.hash, to.size, to.media_type.clone()) {
                    r.insert("recovered".to_owned(), Value::Bool(true));
                    *map = r;
                }
                return;
            }
            map.values_mut().for_each(|x| retarget(x, from, to));
        }
        Value::Array(items) => items.iter_mut().for_each(|x| retarget(x, from, to)),
        _ => {}
    }
}

fn flag(v: &mut Value, hash: &str, reason: &str) {
    match v {
        Value::Object(map) => {
            if blob_ref(map) == Some(hash) {
                mark_corrupt(map, reason);
                return;
            }
            map.values_mut().for_each(|x| flag(x, hash, reason));
        }
        Value::Array(items) => items.iter_mut().for_each(|x| flag(x, hash, reason)),
        _ => {}
    }
}

#[derive(Debug, Default)]
pub struct Repair {
    /// Blobs the payload now points at instead of a redacted copy.
    pub intact: Vec<String>,
    pub tally: Tally,
    pub changed: bool,
}

/// Points refs to redacted data at the intact blob they were cut from; the
/// redacted copy is then not stored.
pub async fn recover_redacted(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
    payload: &mut Value,
    blobs: &mut Vec<Extracted>,
) -> Result<Vec<String>, sqlx::Error> {
    let mut intact = Vec::new();
    let mut i = 0;
    while i < blobs.len() {
        let found = match (blobs[i].corrupt, std::str::from_utf8(&blobs[i].bytes)) {
            (Some(validate::REDACTED), Ok(text)) => {
                recover::find_intact(&mut *conn, session_id, text).await?
            }
            _ => None,
        };
        if let Some(found) = found {
            retarget(payload, &blobs[i].hash, &found);
            intact.push(found.hash);
            blobs.remove(i);
        } else {
            i += 1;
        }
    }
    Ok(intact)
}

/// Refs stored opaque before sources were validated carry no verdict: each is
/// recovered or flagged corrupt.
pub async fn review_opaque(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
    payload: &mut Value,
) -> Result<Repair, sqlx::Error> {
    let mut refs = Vec::new();
    collect_refs(payload, &mut refs);
    let mut repair = Repair::default();
    for r in refs.into_iter().filter(|r| r.opaque && !r.flagged) {
        let stored: Option<Vec<u8>> =
            crate::routes::blobs::read_blob_in(&mut *conn, &r.hash).await?;
        let Some(text) = stored.and_then(|b| String::from_utf8(b).ok()) else { continue };
        let Err(reason) = validate::check(r.media_type.as_deref(), &text) else { continue };
        let found = if reason == validate::REDACTED {
            recover::find_intact(&mut *conn, session_id, &text).await?
        } else {
            None
        };
        if let Some(found) = found {
            retarget(payload, &r.hash, &found);
            repair.intact.push(found.hash);
            repair.tally.recovered += 1;
        } else {
            flag(payload, &r.hash, reason);
            repair.tally.corrupt += 1;
        }
        repair.changed = true;
    }
    Ok(repair)
}

/// `Ok(None)` when a stored row does not hash to its key (a payload must never
/// point at bytes that are not its own); otherwise the bytes newly written.
pub async fn store_verified(
    conn: &mut sqlx::PgConnection,
    blobs: &[Extracted],
) -> Result<Option<u64>, sqlx::Error> {
    let mut written = 0u64;
    for b in blobs {
        let stored = crate::routes::blobs::store_blob_in(
            &mut *conn,
            &b.bytes,
            Some(sniff_media_type("", &b.bytes)),
        )
        .await?;
        if stored.created {
            written += b.bytes.len() as u64;
        }
        let stored_bytes = crate::routes::blobs::read_blob_in(&mut *conn, &b.hash).await?;
        if stored_bytes.as_deref() != Some(b.bytes.as_slice()) {
            tracing::warn!(hash = %b.hash, "stored blob does not match its hash");
            return Ok(None);
        }
    }
    Ok(Some(written))
}

/// A replay of an event stored inline before the backfill reached it: the
/// rewritten payload would slip past the dedup indexes as a second row.
async fn stored_inline(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
    event_type: &str,
    payload: &Value,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM stream_events WHERE session_id = $1 \
         AND event_type = $2 AND content_hash = digest(($3::jsonb)::text, 'sha256'))",
    )
    .bind(session_id)
    .bind(event_type)
    .bind(payload)
    .fetch_one(conn)
    .await
}

/// On any failure, or when the session already holds the event inline, the
/// payload is left as it was.
pub async fn externalize(
    pool: &sqlx::PgPool,
    session_id: &str,
    event_type: &str,
    payload: &mut Value,
) -> bool {
    let Some((mut rewritten, mut blobs)) = extract_blocking(payload).await else {
        return false;
    };
    let original = &*payload;
    let stored: Result<Option<u64>, sqlx::Error> = async {
        let mut conn = pool.acquire().await?;
        if stored_inline(&mut conn, session_id, event_type, original).await? {
            return Ok(None);
        }
        recover_redacted(&mut conn, session_id, &mut rewritten, &mut blobs).await?;
        for b in &blobs {
            if let Some(reason) = b.corrupt {
                tracing::warn!(session_id, hash = %b.hash, reason, "stored a corrupt image source");
            }
        }
        store_verified(&mut conn, &blobs).await
    }
    .await;
    match stored {
        Ok(Some(_)) => {
            *payload = rewritten;
            true
        }
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(%err, "blob store write failed; payload kept inline");
            false
        }
    }
}

pub async fn externalize_event(pool: &sqlx::PgPool, event: &mut AdapterEvent) {
    match event {
        AdapterEvent::Message { local_id, payload, .. } => {
            externalize(pool, local_id, "message", payload).await;
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            externalize(pool, local_id, "tool_use", payload).await;
        }
        _ => {}
    }
}

pub fn set_payload(event: &mut AdapterEvent, rewritten: &Value) {
    if let AdapterEvent::Message { payload, .. } | AdapterEvent::ToolUse { payload, .. } = event {
        payload.clone_from(rewritten);
    }
}

struct RefInfo {
    hash: String,
    size: u64,
    media_type: Option<String>,
    opaque: bool,
    flagged: bool,
}

fn blob_ref(map: &Map<String, Value>) -> Option<&str> {
    if map.get("type").and_then(Value::as_str) != Some(BLOB_SOURCE_TYPE) {
        return None;
    }
    map.get("blob_id").and_then(Value::as_str)
}

fn collect_refs(v: &Value, out: &mut Vec<RefInfo>) {
    match v {
        Value::Object(map) => {
            if let Some(hash) = blob_ref(map) {
                if !out.iter().any(|r| r.hash == hash) {
                    out.push(RefInfo {
                        hash: hash.to_owned(),
                        size: map.get("size").and_then(Value::as_u64).unwrap_or(0),
                        media_type: map
                            .get("media_type")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        opaque: map.get("encoding").and_then(Value::as_str)
                            == Some(OPAQUE_ENCODING),
                        flagged: map.contains_key("corrupt"),
                    });
                }
                return;
            }
            map.values().for_each(|x| collect_refs(x, out));
        }
        Value::Array(items) => items.iter().for_each(|x| collect_refs(x, out)),
        _ => {}
    }
}

fn inline_refs(v: &mut Value, data: &HashMap<String, (Option<String>, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(hash) = blob_ref(map) {
                if let Some((media_type, b64)) = data.get(hash) {
                    let media_type = map
                        .get("media_type")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| media_type.clone());
                    let mut m = Map::new();
                    m.insert("type".to_owned(), Value::from("base64"));
                    if let Some(mt) = media_type {
                        m.insert("media_type".to_owned(), Value::from(mt));
                    }
                    m.insert("data".to_owned(), Value::from(b64.as_str()));
                    *map = m;
                }
                return;
            }
            map.values_mut().for_each(|x| inline_refs(x, data));
        }
        Value::Array(items) => items.iter_mut().for_each(|x| inline_refs(x, data)),
        _ => {}
    }
}

fn image_label(block: &Map<String, Value>) -> String {
    let source = block.get("source").and_then(Value::as_object);
    let field = |k: &str| source.and_then(|s| s.get(k));
    let media_type = field("media_type").and_then(Value::as_str).unwrap_or("image");
    let size = field("size").and_then(Value::as_u64).or_else(|| {
        field("data").and_then(Value::as_str).map(|d| {
            let pad = d.bytes().rev().take_while(|b| *b == b'=').count();
            (d.len() / 4 * 3).saturating_sub(pad) as u64
        })
    });
    size.map_or_else(
        || format!("[image {media_type}]"),
        |n| format!("[image {media_type} {} KB]", n.div_ceil(1024)),
    )
}

/// The text blocks of a stringified content array that carries images, with
/// a label per image; `None` for any other summary.
#[must_use]
pub fn readable_result(summary: &str) -> Option<String> {
    let head = summary.trim_start();
    if !head.starts_with('[') || !head.contains("\"image\"") {
        return None;
    }
    let Value::Array(blocks) = serde_json::from_str::<Value>(head).ok()? else {
        return None;
    };
    let mut parts = Vec::new();
    let mut images = 0;
    for block in &blocks {
        let block = block.as_object()?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(block.get("text").and_then(Value::as_str)?.to_owned()),
            Some("image") => {
                images += 1;
                parts.push(image_label(block));
            }
            _ => return None,
        }
    }
    (images > 0).then(|| parts.join("\n\n"))
}

fn result_blocks(row: &Value) -> Option<Value> {
    if row.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let summary = row.get("output_summary").and_then(Value::as_str)?;
    if !summary.contains(BLOB_SOURCE_TYPE) {
        return None;
    }
    serde_json::from_str(summary).ok()
}

/// Blobs past `budget` decoded bytes stay references, fetchable from
/// `/sessions/{id}/blobs/{hash}`.
pub async fn inline_client_rows(
    pool: &sqlx::PgPool,
    session_id: &str,
    rows: &mut [Value],
    mut budget: usize,
) -> Result<(), sqlx::Error> {
    let mut data: HashMap<String, (Option<String>, String)> = HashMap::new();
    for row in rows.iter_mut() {
        let Some(mut blocks) = result_blocks(row) else { continue };
        let mut refs = Vec::new();
        collect_refs(&blocks, &mut refs);
        for r in refs {
            if r.opaque
                || data.contains_key(&r.hash)
                || !usize::try_from(r.size).is_ok_and(|s| s <= budget)
            {
                continue;
            }
            let Some((media_type, bytes)) =
                crate::routes::blobs::fetch_session_blob(pool, session_id, &r.hash).await?
            else {
                continue;
            };
            if bytes.len() > budget {
                continue;
            }
            budget -= bytes.len();
            let text = base64::engine::general_purpose::STANDARD.encode(&bytes);
            data.insert(r.hash, (media_type, text));
        }
        inline_refs(&mut blocks, &data);
        if let Some(obj) = row.as_object_mut() {
            obj.insert("output_summary".to_owned(), Value::from(blocks.to_string()));
        }
    }
    Ok(())
}

#[cfg(test)]
pub mod tests;
