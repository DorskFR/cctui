use std::io::Cursor;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::backfill::Walk;

#[must_use]
pub fn noise(seed: u128, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 8);
    let mut x = (u64::try_from(seed % u128::from(u64::MAX)).unwrap_or(1) << 1) | 1;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

#[must_use]
pub fn encode(seed: u128, len: usize, format: image::ImageFormat) -> Vec<u8> {
    let side = u32::try_from((len / 3).isqrt().max(1)).unwrap();
    let raw = noise(seed, usize::try_from(side * side * 3).unwrap());
    let img = image::RgbImage::from_raw(side, side, raw).unwrap();
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, format).unwrap();
    out.into_inner()
}

/// A decodable PNG of noise, about `len` bytes like a real screenshot.
#[must_use]
pub fn screenshot(seed: u128, len: usize) -> Vec<u8> {
    encode(seed, len, image::ImageFormat::Png)
}

#[must_use]
pub fn photo(seed: u128, len: usize) -> Vec<u8> {
    encode(seed, len, image::ImageFormat::Jpeg)
}

#[must_use]
pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[must_use]
pub fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A `Read` of a PNG as the claude-code adapter stores it.
#[must_use]
pub fn read_result(tool_use_id: &str, bytes: &[u8]) -> Value {
    json!({
        "kind": "tool_result",
        "tool_use_id": tool_use_id,
        "is_error": false,
        "content": [{"type": "image", "source": {
            "type": "base64", "media_type": "image/png", "data": b64(bytes),
        }}],
    })
}

pub struct Seeded {
    pub url: String,
    pub pool: PgPool,
    pub uid: Uuid,
    pub machine: Uuid,
    pub sessions: Vec<String>,
}

pub async fn seeded(name: &str, sessions: usize) -> Option<Seeded> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{name}-{uid}"))
        .bind(format!("h-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{machine}"))
        .execute(&pool)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for i in 0..sessions {
        let sid = format!("{name}-{i}-{uid}");
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        ids.push(sid);
    }
    Some(Seeded { url, pool, uid, machine, sessions: ids })
}

impl Seeded {
    pub async fn insert(&self, session: usize, payload: &Value) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             VALUES ($1, 'tool_use', $2) RETURNING id",
        )
        .bind(&self.sessions[session])
        .bind(payload)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    pub async fn payload(&self, id: i64) -> Value {
        sqlx::query_scalar("SELECT payload FROM stream_events WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn payload_has(&self, id: i64, needle: &str) -> bool {
        self.payload(id).await.to_string().contains(needle)
    }

    pub async fn blob_rows(&self, hash: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM daemon_blobs WHERE hash = $1")
            .bind(hash)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn served(&self, session: usize, hash: &str) -> Option<Vec<u8>> {
        crate::routes::fetch_session_blob(&self.pool, &self.sessions[session], hash)
            .await
            .unwrap()
            .map(|(_, bytes)| bytes)
    }

    pub async fn cleanup(self, hashes: &[String], walk: Option<&str>) {
        sqlx::query("DELETE FROM sessions WHERE id = ANY($1)")
            .bind(&self.sessions)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM machines WHERE id = $1")
            .bind(self.machine)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(self.uid)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = ANY($1)")
            .bind(hashes)
            .execute(&self.pool)
            .await
            .unwrap();
        if let Some(name) = walk {
            sqlx::query("DELETE FROM image_blob_backfill WHERE name = $1")
                .bind(name)
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }
}

#[must_use]
pub fn walk(name: &str, rows: usize, scope: &[String]) -> Walk {
    Walk {
        name: name.to_owned(),
        scan: 500,
        rows,
        concurrency: 1,
        lag: Duration::ZERO,
        scope: Some(scope.to_vec()),
    }
}

pub async fn set_cursor(pool: &PgPool, name: &str, id: i64) {
    sqlx::query(
        "INSERT INTO image_blob_backfill (name, last_event_id) VALUES ($1, $2) \
         ON CONFLICT (name) DO UPDATE SET last_event_id = EXCLUDED.last_event_id",
    )
    .bind(name)
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}
