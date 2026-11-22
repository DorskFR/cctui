use base64::Engine;
use serde_json::json;
use sha2::Digest;

use super::*;
use cctui_server_sessions::routes::daemon::test_support::{
    drop_machines, seed_machine, seed_owned_session,
};

#[tokio::test]
async fn ingest_inserts_without_search_text_and_the_row_is_found_through_its_chunks() {
    for sql in [INSERT_ONE_SQL, INSERT_MANY_SQL] {
        assert!(!sql.contains("search_text"), "{sql}");
    }
    let Some(url) = cctui_server_kernel::test_support::db_url("ingest_chunks_only") else {
        return;
    };
    let pool = crate::db::connect(&url).await.expect("connect test db");
    let (uid, mid) = seed_machine(&pool, "ingest_chunks_only").await;
    let sid = seed_owned_session(&pool, uid, mid).await;
    let payload = |text: &str| json!({ "role": "assistant", "text": text });
    let row = |text: &str| NewEvent::unchunked(&sid, "message", payload(text), None);
    let batched = insert_events(&pool, mid, uid, vec![row("a batched quokka line")])
        .await
        .expect("batched insert");
    let single =
        insert_event(&pool, mid, uid, row("a single quokka line")).await.expect("single insert");
    let ids: Vec<i64> = batched.into_iter().chain([single]).map(|s| s.expect("new row")).collect();
    let chunked: Vec<cctui_server_kernel::search_types::Chunked> = ids
        .iter()
        .zip(["a batched quokka line", "a single quokka line"])
        .map(|(&event_id, text)| cctui_server_kernel::search_types::Chunked {
            event_id,
            session_id: sid.clone(),
            bodies: cctui_server_search::chunk_event("claude-code", "message", payload(text)),
        })
        .collect();
    cctui_server_search::insert_chunks(&pool, &chunked).await.expect("chunks");
    let found: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT event_id FROM search_chunks \
         WHERE session_id = $1 AND body ILIKE '%quokka%' ORDER BY event_id",
    )
    .bind(&sid)
    .fetch_all(&pool)
    .await
    .expect("search");
    assert_eq!(found, ids);

    sqlx::query("DELETE FROM stream_events WHERE session_id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .ok();
    drop_machines(&pool, &[sid], &[(uid, mid)]).await;
}

#[tokio::test]
async fn an_image_result_is_stored_once_by_reference_and_served_to_each_session() {
    let Some(url) = cctui_server_kernel::test_support::db_url("image_ingest") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (uid, mid) = seed_machine(&pool, "image_ingest").await;
    let a = seed_owned_session(&pool, uid, mid).await;
    let b = seed_owned_session(&pool, uid, mid).await;
    let png = cctui_server_blobs::image_blobs::test_support::screenshot(
        Uuid::new_v4().as_u128(),
        450_000,
    );
    let hash = hex::encode(sha2::Sha256::digest(&png));
    let data_head = base64::engine::general_purpose::STANDARD.encode(&png[..96]);

    let mut rows = Vec::new();
    for sid in [&a, &b] {
        let mut payload =
            cctui_server_blobs::image_blobs::test_support::read_result("toolu_shot", &png);
        assert!(
            cctui_server_blobs::image_blobs::externalize(&pool, sid, "tool_use", &mut payload)
                .await
        );
        rows.push(NewEvent {
            local_id: sid.clone(),
            event_type: "tool_use",
            payload,
            turn_id: None,
        });
    }
    let seqs = insert_events(&pool, mid, uid, rows).await.expect("insert");
    assert!(seqs.iter().all(Option::is_some));

    for (sid, seq) in [(&a, seqs[0]), (&b, seqs[1])] {
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM stream_events WHERE id = $1")
                .bind(seq)
                .fetch_one(&pool)
                .await
                .expect("stored row");
        assert_eq!(payload["content"][0]["source"]["type"], "cctui-blob");
        assert_eq!(payload["content"][0]["source"]["blob_id"], hash);
        assert!(!payload.to_string().contains(&data_head), "no base64 in the payload");
        let bodies = cctui_server_search::chunk_event("claude-code", "tool_use", payload);
        assert!(bodies.iter().all(|b| !b.contains(&data_head)), "no base64 in the chunks");
        let served = cctui_server_blobs::routes::fetch_session_blob(&pool, sid, &hash)
            .await
            .expect("fetch")
            .map(|(_, bytes)| bytes);
        assert_eq!(served.as_deref(), Some(png.as_slice()), "each session serves it");
    }
    let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM daemon_blobs WHERE hash = $1")
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .expect("count blobs");
    assert_eq!(blobs, 1, "one blob for the screenshot of both sessions");

    sqlx::query("DELETE FROM stream_events WHERE session_id = ANY($1)")
        .bind(vec![a.clone(), b.clone()])
        .execute(&pool)
        .await
        .ok();
    drop_machines(&pool, &[a, b], &[(uid, mid)]).await;
    sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1").bind(&hash).execute(&pool).await.ok();
}
