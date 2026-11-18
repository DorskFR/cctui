use cctui_proto::api::{StorageDropRule, StorageLifecycleRules, StorageTrimRule};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::job::Walk;
use super::lifecycle::{
    MARK, MIN_ABOVE_BYTES, Rule, TRIM_NOTE, active, apply, dropped_text, text_len, validate,
};

const TRIM: Rule = Rule::Trim { after_days: 7, above_bytes: 4096 };
const DROP: Rule = Rule::Drop { after_days: 7 };

/// Hex of a hash chain: pglz finds nothing to compress in it.
fn noise(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 64);
    let mut seed = Sha256::digest(bytes.to_le_bytes()).to_vec();
    while out.len() < bytes {
        seed = Sha256::digest(&seed).to_vec();
        out.push_str(&hex::encode(&seed));
    }
    out.truncate(bytes);
    out
}

fn result(id: &str, content: &Value) -> Value {
    json!({ "kind": "tool_result", "tool_use_id": id, "content": content, "is_error": false })
}

#[test]
fn trimming_twice_is_trimming_once_and_leaves_a_marker() {
    let payload = result("t1", &json!(noise(50_000)));
    let once = apply(TRIM, "tool_use", &payload).expect("over the limit");
    let content = once["content"].as_str().unwrap();
    assert!(content.len() <= 4096, "{}", content.len());
    assert!(content.ends_with(TRIM_NOTE));
    assert!(content.starts_with(&noise(50_000)[..1000]));
    assert_eq!(once[MARK], json!({ "action": "trim", "original_bytes": 50_000 }));
    assert_eq!(once["tool_use_id"], "t1");
    assert_eq!(apply(TRIM, "tool_use", &once), None, "a second pass changes nothing");
    assert_eq!(apply(TRIM, "tool_use", &result("t2", &json!("ok"))), None);
}

#[test]
fn trimming_cuts_on_a_char_boundary() {
    let payload = result("t1", &json!("é".repeat(10_000)));
    let once = apply(TRIM, "tool_use", &payload).expect("over the limit");
    let content = once["content"].as_str().unwrap();
    assert!(content.len() <= 4096);
    assert!(content.trim_end_matches(TRIM_NOTE).chars().all(|c| c == 'é'));
}

#[test]
fn trimming_block_content_keeps_image_references() {
    let image = json!({ "type": "image", "source": { "type": "cctui-blob", "blob_id": "abc" } });
    let text = json!({ "type": "text", "text": noise(3000) });
    let payload = result("t1", &json!([text, image, text]));
    let once = apply(TRIM, "tool_use", &payload).expect("over the limit");
    let blocks = once["content"].as_array().unwrap();
    assert!(blocks.contains(&image));
    assert!(text_len(&once["content"]) <= 4096);
    assert_eq!(blocks.last().unwrap()["text"], TRIM_NOTE.trim_start());
    assert_eq!(apply(TRIM, "tool_use", &once), None);
}

#[test]
fn opencode_and_codex_outputs_are_trimmed_in_their_own_fields() {
    let big = noise(20_000);
    let opencode = json!({ "type": "tool_result", "kind": "tool_result", "content": &big,
        "output_summary": &big, "tool_use_id": "c1" });
    let once = apply(TRIM, "tool_use", &opencode).unwrap();
    assert!(text_len(&once["content"]) <= 4096 && text_len(&once["output_summary"]) <= 4096);

    let codex = json!({ "type": "response_item", "payload": {
        "type": "function_call_output", "call_id": "c1", "output": &big } });
    let once = apply(TRIM, "message", &codex).unwrap();
    assert!(text_len(&once["payload"]["output"]) <= 4096);
    assert_eq!(once["payload"]["call_id"], "c1");
}

#[test]
fn conversation_text_and_tool_calls_have_nothing_to_trim_or_drop() {
    let long = noise(100_000);
    let rows = [
        ("message", json!({ "role": "user", "text": &long })),
        ("message", json!({ "role": "assistant", "text": &long })),
        (
            "message",
            json!({ "type": "response_item", "payload": { "type": "message",
            "content": [{ "type": "output_text", "text": &long }] } }),
        ),
        ("tool_use", json!({ "tool": "Write", "input": { "content": &long }, "id": "t1" })),
        ("message", result("t1", &json!(&long))),
    ];
    for (event_type, payload) in &rows {
        for rule in [TRIM, DROP] {
            assert_eq!(apply(rule, event_type, payload), None, "{event_type} {payload}");
        }
    }
}

#[test]
fn dropping_replaces_only_the_output_and_keeps_the_trimmed_size() {
    let payload = result("t1", &json!(noise(50_000)));
    let trimmed = apply(TRIM, "tool_use", &payload).unwrap();
    let dropped = apply(DROP, "tool_use", &trimmed).unwrap();
    assert_eq!(dropped["content"], dropped_text(50_000));
    assert_eq!(dropped[MARK], json!({ "action": "drop", "original_bytes": 50_000 }));
    assert_eq!(dropped["tool_use_id"], "t1");
    assert_eq!(dropped["is_error"], false);
    assert_eq!(apply(DROP, "tool_use", &dropped), None);
    assert_eq!(apply(TRIM, "tool_use", &dropped), None);
}

#[test]
fn the_default_rule_set_enables_nothing() {
    assert!(active(&StorageLifecycleRules::default()).is_empty());
    let off = StorageLifecycleRules {
        trim: Some(StorageTrimRule { enabled: false, after_days: 7, above_bytes: 4096 }),
        drop: Some(StorageDropRule { enabled: false, after_days: 30 }),
    };
    assert!(active(&off).is_empty());
    let on = StorageLifecycleRules {
        trim: off.trim.map(|t| StorageTrimRule { enabled: true, ..t }),
        drop: off.drop.map(|d| StorageDropRule { enabled: true, ..d }),
    };
    assert_eq!(
        active(&on),
        [Rule::Drop { after_days: 30 }, Rule::Trim { after_days: 7, above_bytes: 4096 }]
    );
}

#[test]
fn rules_out_of_range_are_refused() {
    let trim = |after_days, above_bytes| StorageLifecycleRules {
        trim: Some(StorageTrimRule { enabled: true, after_days, above_bytes }),
        drop: None,
    };
    assert!(validate(trim(7, MIN_ABOVE_BYTES)).is_ok());
    assert!(validate(trim(0, 4096)).is_err());
    assert!(validate(trim(7, MIN_ABOVE_BYTES - 1)).is_err());
    let drop = StorageLifecycleRules {
        trim: None,
        drop: Some(StorageDropRule { enabled: true, after_days: 100_000 }),
    };
    assert!(validate(drop).is_err());
}

struct Seeded {
    pool: PgPool,
    uid: Uuid,
    mid: Uuid,
    sid: String,
    name: String,
}

impl Seeded {
    async fn new(tag: &str) -> Option<Self> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = (Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{tag}-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .unwrap();
        let sid = format!("{tag}-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, user_id, \
             adapter_id) VALUES ($1, $2, $3, '/w', 'active', $4, 'claude-code')",
        )
        .bind(&sid)
        .bind(mid.to_string())
        .bind(mid)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        let name = format!("test-{}", Uuid::new_v4());
        Some(Self { pool, uid, mid, sid, name })
    }

    async fn event(&self, event_type: &str, payload: &Value, days_old: i32) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO stream_events (session_id, event_type, payload, created_at) \
             VALUES ($1, $2, $3, now() - make_interval(days => $4)) RETURNING id",
        )
        .bind(&self.sid)
        .bind(event_type)
        .bind(payload)
        .bind(days_old)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn payloads(&self) -> Vec<(i64, Value)> {
        sqlx::query_as("SELECT id, payload FROM stream_events WHERE session_id = $1 ORDER BY id")
            .bind(&self.sid)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    async fn stored(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(sum(pg_column_size(payload)), 0)::int8 FROM stream_events \
             WHERE session_id = $1",
        )
        .bind(&self.sid)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    fn walk(&self, rules: Vec<Rule>) -> Walk {
        Walk { scope: Some(vec![self.sid.clone()]), ..Walk::new(&self.name, rules) }
    }

    async fn run(&self, rules: Vec<Rule>) -> (u64, u64) {
        let walk = self.walk(rules);
        let (mut trimmed, mut dropped) = (0, 0);
        for _ in 0..50 {
            let step = walk.step(&self.pool).await.expect("step");
            trimmed += step.trimmed;
            dropped += step.dropped;
            if !step.more {
                return (trimmed, dropped);
            }
        }
        panic!("the walk did not finish");
    }

    async fn forget_cursors(&self) {
        sqlx::query("DELETE FROM storage_lifecycle_cursors WHERE name = $1")
            .bind(&self.name)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn runs(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM storage_lifecycle_runs WHERE name = $1")
            .bind(&self.name)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        for table in ["storage_lifecycle_cursors", "storage_lifecycle_runs"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE name = $1")))
                .bind(&self.name)
                .execute(&self.pool)
                .await
                .ok();
        }
        let pool = &self.pool;
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&self.sid).execute(pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(self.mid).execute(pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(self.uid).execute(pool).await.ok();
    }
}

fn trim_rules() -> StorageLifecycleRules {
    StorageLifecycleRules {
        trim: Some(StorageTrimRule { enabled: true, after_days: 7, above_bytes: 4096 }),
        drop: None,
    }
}

async fn conversation(s: &Seeded) {
    let long = noise(30_000);
    for (event_type, payload) in [
        ("message", json!({ "role": "user", "text": &long })),
        ("message", json!({ "role": "assistant", "text": &long })),
        ("tool_use", json!({ "tool": "Bash", "input": { "command": &long }, "id": "t1" })),
        ("tool_use", result("t2", &json!("ok"))),
    ] {
        s.event(event_type, &payload, 30).await;
    }
}

#[tokio::test]
async fn a_trim_run_twice_gives_the_same_rows_and_spares_the_conversation() {
    let Some(s) = Seeded::new("storage_trim_twice").await else { return };
    conversation(&s).await;
    let old = s.event("tool_use", &result("t1", &json!(noise(60_000))), 30).await;
    s.event("tool_use", &result("t3", &json!(noise(60_000))), 1).await;
    let before = s.payloads().await;

    assert_eq!(s.run(vec![TRIM]).await, (1, 0));
    let once = s.payloads().await;
    for (id, payload) in &once {
        let original = &before.iter().find(|(i, _)| i == id).unwrap().1;
        if *id == old {
            let content = payload["content"].as_str().unwrap();
            assert!(content.len() <= 4096 && content.ends_with(TRIM_NOTE));
            assert_eq!(payload[MARK]["action"], "trim");
        } else {
            assert_eq!(payload, original, "row {id} untouched");
        }
    }

    s.forget_cursors().await;
    assert_eq!(s.run(vec![TRIM]).await, (0, 0), "a full second walk changes nothing");
    assert_eq!(s.payloads().await, once);

    let info = super::info(&s.pool, &s.name).await.unwrap();
    let last = info.last_run.expect("a run is recorded");
    assert!(last.finished_at.is_some());
    assert_eq!(last.rows_trimmed, 0, "the second run is the last one");
    assert!(info.bytes_freed_total > 10_000, "{}", info.bytes_freed_total);
    s.cleanup().await;
}

#[tokio::test]
async fn a_drop_run_removes_only_tool_output() {
    let Some(s) = Seeded::new("storage_drop_only_output").await else { return };
    conversation(&s).await;
    s.event("tool_use", &result("t1", &json!(noise(20_000))), 30).await;
    let before = s.payloads().await;

    assert_eq!(s.run(vec![DROP]).await, (0, 2), "the big and the small result");
    let after = s.payloads().await;
    for (id, payload) in &after {
        let original = &before.iter().find(|(i, _)| i == id).unwrap().1;
        let is_result = original.get("kind").is_some();
        if is_result {
            assert_eq!(payload[MARK]["action"], "drop");
            assert!(payload["content"].as_str().unwrap().starts_with("[tool output removed"));
            assert_eq!(payload["tool_use_id"], original["tool_use_id"]);
        } else {
            assert_eq!(payload, original, "row {id} untouched");
        }
    }
    s.forget_cursors().await;
    assert_eq!(s.run(vec![DROP]).await, (0, 0));
    assert_eq!(s.payloads().await, after);
    s.cleanup().await;
}

#[tokio::test]
async fn without_an_enabled_rule_the_job_touches_nothing() {
    let Some(s) = Seeded::new("storage_default_off").await else { return };
    conversation(&s).await;
    s.event("tool_use", &result("t1", &json!(noise(60_000))), 30).await;
    let before = s.payloads().await;
    let rules = active(&StorageLifecycleRules {
        trim: trim_rules().trim.map(|t| StorageTrimRule { enabled: false, ..t }),
        drop: None,
    });
    let step = s.walk(rules).step(&s.pool).await.unwrap();
    assert_eq!(step, super::job::Step::default());
    assert_eq!(s.payloads().await, before);
    assert_eq!(s.runs().await, 0, "no run is recorded");
    s.cleanup().await;
}

#[tokio::test]
async fn the_preview_counts_what_the_job_then_changes() {
    let Some(s) = Seeded::new("storage_preview_matches").await else { return };
    conversation(&s).await;
    for (i, size) in [60_000, 120_000, 9_000].into_iter().enumerate() {
        s.event("tool_use", &result(&format!("r{i}"), &json!(noise(size))), 30).await;
    }
    s.event("tool_use", &result("young", &json!(noise(60_000))), 1).await;
    let scope = [s.sid.clone()];
    let preview = super::preview(&s.pool, trim_rules(), Some(scope.as_slice())).await.unwrap();
    assert!((preview.percent - 100.0).abs() < f64::EPSILON);
    assert_eq!((preview.rows_trimmed, preview.rows_dropped), (3, 0));

    let stored = s.stored().await;
    assert_eq!(s.run(vec![TRIM]).await, (3, 0));
    let freed = u64::try_from(stored - s.stored().await).unwrap();
    let (low, high) = (freed * 4 / 5, freed * 6 / 5);
    assert!((low..=high).contains(&preview.bytes_freed), "{preview:?} vs measured {freed}");

    let after = super::preview(&s.pool, trim_rules(), Some(scope.as_slice())).await.unwrap();
    assert_eq!((after.rows_trimmed, after.bytes_freed), (0, 0));
    s.cleanup().await;
}
