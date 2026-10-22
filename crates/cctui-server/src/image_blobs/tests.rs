use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use super::backfill::{Config, Outcome, Walk, rewrite_event};
use super::*;

/// Incompressible bytes behind a PNG signature, like a real screenshot.
#[must_use]
pub fn screenshot(seed: u128, len: usize) -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut x = u64::try_from(seed % u128::from(u64::MAX)).unwrap_or(1) | 1;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn sha(bytes: &[u8]) -> String {
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

/// Secret redaction rewrote a token-shaped run inside the base64 on prod.
fn redacted_result(tool_use_id: &str, bytes: &[u8]) -> (Value, String) {
    let mut data = b64(bytes);
    data.replace_range(240..304, "[REDACTED:square_token]");
    let payload = json!({
        "kind": "tool_result",
        "tool_use_id": tool_use_id,
        "is_error": false,
        "content": [{"type": "image", "source": {
            "type": "base64", "media_type": "image/png", "data": data,
        }}],
    });
    (payload, data)
}

fn captioned_result(tool_use_id: &str, bytes: &[u8]) -> Value {
    json!({
        "kind": "tool_result",
        "tool_use_id": tool_use_id,
        "is_error": false,
        "content": [
            {"type": "text", "text": "Screenshot saved to /tmp/shot.jpg"},
            {"type": "image", "source": {
                "type": "base64", "media_type": "image/jpeg", "data": b64(bytes),
            }},
        ],
    })
}

fn source(payload: &Value, i: usize) -> &Value {
    &payload["content"][i]["source"]
}

#[test]
fn extract_moves_a_prod_shaped_read_result() {
    let png = screenshot(1, 300_000);
    let payload = read_result("toolu_01", &png);
    let (out, blobs) = extract(&payload).expect("an image to move");
    assert_eq!(blobs.len(), 1);
    assert_eq!(blobs[0].hash, sha(&png));
    assert_eq!(blobs[0].bytes, png);
    let expected = json!({
        "type": "cctui-blob", "blob_id": sha(&png), "size": 300_000, "media_type": "image/png",
    });
    assert_eq!(source(&out, 0), &expected);
    assert_eq!(out["content"][0]["type"], "image");
    assert_eq!(out["tool_use_id"], "toolu_01");
    assert_eq!(out["kind"], "tool_result");
    assert!(!out.to_string().contains(&b64(&png)[..64]), "no base64 left");
}

#[test]
fn extract_keeps_text_and_dedups_a_repeated_source() {
    let jpg = screenshot(2, 50_000);
    let mut payload = captioned_result("toolu_02", &jpg);
    payload["content"].as_array_mut().unwrap().push(json!({"type": "image", "source": {
        "type": "base64", "media_type": "image/jpeg", "data": b64(&jpg),
    }}));
    let (out, blobs) = extract(&payload).expect("images to move");
    assert_eq!(blobs.len(), 1, "one blob for the same bytes twice");
    assert_eq!(out["content"][0]["text"], "Screenshot saved to /tmp/shot.jpg");
    assert_eq!(source(&out, 1)["blob_id"], sha(&jpg));
    assert_eq!(source(&out, 2)["blob_id"], sha(&jpg));
}

#[test]
fn extract_moves_redacted_data_verbatim() {
    let (payload, data) = redacted_result("toolu_r", &screenshot(7, 60_000));
    let (out, blobs) = extract(&payload).expect("moved even though it does not decode");
    assert_eq!(blobs[0].bytes, data.as_bytes());
    assert_eq!(blobs[0].hash, sha(data.as_bytes()));
    assert_eq!(source(&out, 0)["encoding"], "base64");
    assert_eq!(source(&out, 0)["size"], data.len());
    assert!(!out.to_string().contains("REDACTED"));
}

#[test]
fn extract_leaves_small_and_moved_sources_alone() {
    assert!(extract(&read_result("t", &screenshot(3, 200))).is_none(), "below the minimum");
    let (moved, _) = extract(&read_result("t", &screenshot(4, 5000))).unwrap();
    assert!(extract(&moved).is_none(), "a reference is never moved again");
    assert!(extract(&json!({"kind": "tool_result", "content": "plain"})).is_none());
}

#[test]
fn readable_result_labels_images_and_keeps_text() {
    let jpg = screenshot(5, 4096);
    let inline = captioned_result("t", &jpg)["content"].to_string();
    assert_eq!(
        readable_result(&inline).as_deref(),
        Some("Screenshot saved to /tmp/shot.jpg\n\n[image image/jpeg 4 KB]")
    );
    let (moved, _) = extract(&read_result("t", &screenshot(6, 300_000))).unwrap();
    assert_eq!(
        readable_result(&moved["content"].to_string()).as_deref(),
        Some("[image image/png 293 KB]")
    );
    assert!(readable_result("plain output").is_none());
    assert!(readable_result(r#"[{"type":"text","text":"no image"}]"#).is_none());
    assert!(readable_result(r#"[{"type":"tool_reference"},{"type":"image"}]"#).is_none());
}

#[test]
fn backfill_config_is_off_unless_enabled() {
    assert_eq!(Config::from_lookup(|_| None), None);
    for v in ["0", "off", "false", ""] {
        let get = |k: &str| (k == "CCTUI_IMAGE_BACKFILL").then(|| v.to_owned());
        assert_eq!(Config::from_lookup(get), None, "{v:?} keeps it off");
    }
    let on =
        Config::from_lookup(|k| (k == "CCTUI_IMAGE_BACKFILL").then(|| "1".into())).expect("on");
    assert_eq!((on.scan, on.rows, on.pause), (500, 20, Duration::from_secs(1)));
    assert_eq!(on.budget_bytes, 512 * 1024 * 1024);
    let tuned = Config::from_lookup(|k| match k {
        "CCTUI_IMAGE_BACKFILL" => Some("true".into()),
        "CCTUI_IMAGE_BACKFILL_SCAN" => Some("100".into()),
        "CCTUI_IMAGE_BACKFILL_ROWS" => Some("5".into()),
        "CCTUI_IMAGE_BACKFILL_PAUSE_MS" => Some("2000".into()),
        "CCTUI_IMAGE_BACKFILL_BUDGET_MB" => Some("64".into()),
        _ => None,
    })
    .expect("on");
    assert_eq!((tuned.scan, tuned.rows, tuned.pause), (100, 5, Duration::from_secs(2)));
    assert_eq!(tuned.budget_bytes, 64 * 1024 * 1024);
}

struct Seeded {
    url: String,
    pool: PgPool,
    uid: Uuid,
    machine: Uuid,
    sessions: Vec<String>,
}

async fn seeded(name: &str, sessions: usize) -> Option<Seeded> {
    let url = crate::routes::gateway::test_db_url(name)?;
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
    async fn insert(&self, session: usize, payload: &Value) -> i64 {
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

    async fn payload(&self, id: i64) -> Value {
        sqlx::query_scalar("SELECT payload FROM stream_events WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn search_text_has(&self, id: i64, needle: &str) -> bool {
        sqlx::query_scalar("SELECT strpos(search_text, $2) > 0 FROM stream_events WHERE id = $1")
            .bind(id)
            .bind(needle)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn blob_rows(&self, hash: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM daemon_blobs WHERE hash = $1")
            .bind(hash)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn served(&self, session: usize, hash: &str) -> Option<Vec<u8>> {
        crate::routes::blobs::fetch_session_blob(&self.pool, &self.sessions[session], hash)
            .await
            .unwrap()
            .map(|(_, bytes)| bytes)
    }

    async fn cleanup(self, hashes: &[String], walk: Option<&str>) {
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

fn walk(name: &str, rows: usize, scope: &[String]) -> Walk {
    Walk {
        name: name.to_owned(),
        scan: 500,
        rows,
        lag: Duration::ZERO,
        scope: Some(scope.to_vec()),
    }
}

async fn set_cursor(pool: &PgPool, name: &str, id: i64) {
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

async fn cursor(pool: &PgPool, name: &str) -> (i64, i64) {
    sqlx::query_as("SELECT last_event_id, rows_rewritten FROM image_blob_backfill WHERE name = $1")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn drain(walk: &Walk, pool: &PgPool) -> usize {
    let mut rewritten = 0;
    for _ in 0..1000 {
        let step = walk.step(pool).await.unwrap();
        rewritten += step.rewritten;
        if !step.more {
            return rewritten;
        }
    }
    panic!("image backfill never caught up");
}

#[tokio::test]
async fn ingest_stores_a_repeated_screenshot_once() {
    let Some(db) = seeded("image_ingest_dedup", 0).await else { return };
    let png = screenshot(Uuid::new_v4().as_u128(), 400_000);
    let mut first = read_result("toolu_a", &png);
    let mut second = read_result("toolu_b", &png);
    assert!(externalize(&db.pool, "s", "tool_use", &mut first).await);
    assert!(externalize(&db.pool, "s", "tool_use", &mut second).await);
    assert_eq!(source(&first, 0)["blob_id"], sha(&png));
    assert_eq!(source(&second, 0)["blob_id"], sha(&png));
    assert_eq!(db.blob_rows(&sha(&png)).await, 1, "one row per content hash");
    let stored: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM daemon_blobs WHERE hash = $1")
        .bind(sha(&png))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(stored, png);
    db.cleanup(&[sha(&png)], None).await;
}

#[tokio::test]
async fn a_replay_of_an_inline_row_stays_inline_so_it_dedups() {
    let Some(db) = seeded("image_ingest_replay", 1).await else { return };
    let png = screenshot(Uuid::new_v4().as_u128(), 30_000);
    let stored = read_result("toolu_replayed", &png);
    db.insert(0, &stored).await;
    let mut replay = stored.clone();
    assert!(!externalize(&db.pool, &db.sessions[0], "tool_use", &mut replay).await);
    assert_eq!(replay, stored, "kept inline, so the dedup index suppresses it");
    assert_eq!(db.blob_rows(&sha(&png)).await, 0);
    let mut fresh = read_result("toolu_new", &png);
    assert!(externalize(&db.pool, &db.sessions[0], "tool_use", &mut fresh).await);
    db.cleanup(&[sha(&png)], None).await;
}

#[tokio::test]
async fn backfill_rewrites_prod_shaped_rows_resumably_and_idempotently() {
    let name = "image_backfill_prod_shaped";
    let Some(db) = seeded(name, 2).await else { return };
    let walk_name = format!("test-{}", db.uid);
    let seed = Uuid::new_v4().as_u128();
    let png = screenshot(seed, 420_000);
    let mut jpg = screenshot(seed.wrapping_add(1), 250_000);
    jpg[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    let tiny = screenshot(seed.wrapping_add(2), 200);

    let read = db.insert(0, &read_result("toolu_read", &png)).await;
    let captioned = db.insert(0, &captioned_result("toolu_shot", &jpg)).await;
    let bash = json!({"kind": "tool_result", "tool_use_id": "toolu_bash", "is_error": false,
        "content": "drwxr-xr-x 2 user user 4096 a\n".repeat(2000)});
    let bash_id = db.insert(0, &bash).await;
    let small = read_result("toolu_small", &tiny);
    let small_id = db.insert(0, &small).await;
    let again = db.insert(1, &read_result("toolu_again", &png)).await;
    let (redacted_payload, redacted) = redacted_result("toolu_redacted", &jpg);
    let redacted_id = db.insert(0, &redacted_payload).await;
    set_cursor(&db.pool, &walk_name, read - 1).await;

    let first = walk(&walk_name, 1, &db.sessions).step(&db.pool).await.unwrap();
    assert_eq!((first.rewritten, first.more), (1, true));
    assert_eq!(cursor(&db.pool, &walk_name).await, (read, 1), "the cursor stops at the batch");
    assert_eq!(source(&db.payload(read).await, 0)["type"], "cctui-blob");
    assert_eq!(source(&db.payload(captioned).await, 1)["type"], "base64", "not reached yet");

    let resumed = walk(&walk_name, 20, &db.sessions);
    assert_eq!(drain(&resumed, &db.pool).await, 3, "a new walk resumes at the cursor");
    let opaque = source(&db.payload(redacted_id).await, 0).clone();
    assert_eq!(opaque["encoding"], "base64");
    assert_eq!(db.served(0, &sha(redacted.as_bytes())).await.as_deref(), Some(redacted.as_bytes()));

    for (id, i, bytes) in [(read, 0, &png), (captioned, 1, &jpg), (again, 0, &png)] {
        let p = db.payload(id).await;
        assert_eq!(source(&p, i)["type"], "cctui-blob");
        assert_eq!(source(&p, i)["blob_id"], sha(bytes));
        assert_eq!(source(&p, i)["size"], bytes.len());
        assert!(!db.search_text_has(id, &b64(bytes)[..64]).await, "search_text drops the base64");
    }
    let caption = &db.payload(captioned).await["content"][0]["text"];
    assert_eq!(caption, "Screenshot saved to /tmp/shot.jpg");
    assert_eq!(db.payload(bash_id).await, bash, "text rows are untouched");
    assert_eq!(db.payload(small_id).await, small, "small images stay inline");

    assert_eq!(db.blob_rows(&sha(&png)).await, 1, "one blob for both sessions' screenshot");
    assert_eq!(db.served(0, &sha(&png)).await.as_deref(), Some(png.as_slice()));
    assert_eq!(db.served(1, &sha(&png)).await.as_deref(), Some(png.as_slice()));
    assert_eq!(db.served(0, &sha(&jpg)).await.as_deref(), Some(jpg.as_slice()));
    assert!(db.served(1, &sha(&jpg)).await.is_none(), "only the referencing session serves it");

    let snapshot = [db.payload(read).await, db.payload(captioned).await, db.payload(again).await];
    set_cursor(&db.pool, &walk_name, read - 1).await;
    assert_eq!(drain(&resumed, &db.pool).await, 0, "a re-run rewrites nothing");
    let after = [db.payload(read).await, db.payload(captioned).await, db.payload(again).await];
    assert_eq!(after, snapshot);
    assert_eq!(cursor(&db.pool, &walk_name).await.1, 4);

    db.cleanup(&[sha(&png), sha(&jpg), sha(redacted.as_bytes())], Some(&walk_name)).await;
}

#[tokio::test]
async fn a_blob_that_fails_its_hash_check_leaves_the_row_untouched() {
    let Some(db) = seeded("image_backfill_bad_blob", 1).await else { return };
    let png = screenshot(Uuid::new_v4().as_u128(), 30_000);
    sqlx::query(
        "INSERT INTO daemon_blobs (hash, media_type, byte_len, bytes) \
         VALUES ($1, 'image/png', $2, $3)",
    )
    .bind(sha(&png))
    .bind(i64::try_from(png.len()).unwrap())
    .bind(b"not the screenshot".as_slice())
    .execute(&db.pool)
    .await
    .unwrap();
    let payload = read_result("toolu_bad", &png);
    let id = db.insert(0, &payload).await;

    let mut conn = db.pool.acquire().await.unwrap();
    assert_eq!(rewrite_event(&mut conn, id).await.unwrap(), Outcome::BadBlob);
    drop(conn);
    assert_eq!(db.payload(id).await, payload);
    assert!(db.served(0, &sha(&png)).await.is_none(), "no link to the bad blob");
    db.cleanup(&[sha(&png)], None).await;
}

#[tokio::test]
async fn a_failed_blob_write_leaves_the_row_untouched() {
    let Some(db) = seeded("image_backfill_failed_write", 1).await else { return };
    let png = screenshot(Uuid::new_v4().as_u128(), 30_000);
    let payload = read_result("toolu_fail", &png);
    let id = db.insert(0, &payload).await;

    let mut conn = sqlx::PgConnection::connect(&db.url).await.unwrap();
    sqlx::raw_sql(
        "CREATE TEMP TABLE daemon_blobs (LIKE public.daemon_blobs INCLUDING ALL); \
         ALTER TABLE pg_temp.daemon_blobs ADD CONSTRAINT refuse_writes CHECK (false);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    assert!(rewrite_event(&mut conn, id).await.is_err(), "the blob insert fails");
    conn.close().await.unwrap();

    assert_eq!(db.payload(id).await, payload, "the row keeps its inline image");
    assert_eq!(db.blob_rows(&sha(&png)).await, 0);
    db.cleanup(&[sha(&png)], None).await;
}

#[tokio::test]
async fn a_rewrite_that_duplicates_a_stored_row_is_left_inline() {
    let Some(db) = seeded("image_backfill_duplicate", 1).await else { return };
    let png = screenshot(Uuid::new_v4().as_u128(), 30_000);
    let inline = read_result("toolu_dup", &png);
    let (moved, _) = extract(&inline).unwrap();
    db.insert(0, &moved).await;
    let id = db.insert(0, &inline).await;

    let mut conn = db.pool.acquire().await.unwrap();
    assert_eq!(rewrite_event(&mut conn, id).await.unwrap(), Outcome::Duplicate);
    drop(conn);
    assert_eq!(db.payload(id).await, inline);
    db.cleanup(&[sha(&png)], None).await;
}

#[tokio::test]
async fn inline_on_demand_follows_links_and_the_size_limit() {
    let Some(db) = seeded("image_inline_on_demand", 2).await else { return };
    let png = screenshot(Uuid::new_v4().as_u128(), 60_000);
    let mut payload = read_result("toolu_inline", &png);
    assert!(externalize(&db.pool, &db.sessions[0], "tool_use", &mut payload).await);
    sqlx::query("INSERT INTO session_blob_links (session_id, hash) VALUES ($1, $2)")
        .bind(&db.sessions[0])
        .bind(sha(&png))
        .execute(&db.pool)
        .await
        .unwrap();
    let client = crate::normalize::for_client("claude-code", "tool_use", payload).unwrap();
    let inlined = |session: usize, budget: usize| {
        let (pool, sid, row) = (db.pool.clone(), db.sessions[session].clone(), client.clone());
        async move {
            let mut rows = vec![row];
            inline_client_rows(&pool, &sid, &mut rows, budget).await.unwrap();
            rows.remove(0)["output_summary"].as_str().unwrap().to_owned()
        }
    };

    let full = inlined(0, INLINE_IMAGES_MAX_BYTES).await;
    let blocks: Value = serde_json::from_str(&full).unwrap();
    assert_eq!(
        blocks[0]["source"],
        json!({"type": "base64", "media_type": "image/png", "data": b64(&png)})
    );
    assert!(inlined(0, png.len() - 1).await.contains("cctui-blob"), "past the limit: a ref");
    assert!(inlined(1, INLINE_IMAGES_MAX_BYTES).await.contains("cctui-blob"), "unlinked: a ref");

    let (mut redacted, data) = redacted_result("toolu_redacted", &png);
    let original = redacted["content"].clone();
    assert!(externalize(&db.pool, &db.sessions[0], "tool_use", &mut redacted).await);
    sqlx::query("INSERT INTO session_blob_links (session_id, hash) VALUES ($1, $2)")
        .bind(&db.sessions[0])
        .bind(sha(data.as_bytes()))
        .execute(&db.pool)
        .await
        .unwrap();
    let mut rows = vec![crate::normalize::for_client("claude-code", "tool_use", redacted).unwrap()];
    inline_client_rows(&db.pool, &db.sessions[0], &mut rows, INLINE_IMAGES_MAX_BYTES)
        .await
        .unwrap();
    let summary = rows[0]["output_summary"].as_str().unwrap();
    let restored: Value = serde_json::from_str(summary).unwrap();
    assert_eq!(restored, original, "redacted data comes back byte for byte");
    db.cleanup(&[sha(&png), sha(data.as_bytes())], None).await;
}
