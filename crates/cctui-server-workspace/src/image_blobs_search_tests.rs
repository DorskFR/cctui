use uuid::Uuid;

use crate::image_blobs::extract;
use crate::image_blobs::test_support::{
    Seeded, b64, read_result, screenshot, seeded, set_cursor, sha, walk,
};

async fn chunks_have(db: &Seeded, id: i64, needle: &str) -> bool {
    let payload = db.payload(id).await;
    let bodies = crate::search_index::chunk_event("claude-code", "tool_use", payload);
    bodies.iter().any(|b| b.contains(needle))
}

#[tokio::test]
async fn backfill_leaves_a_duplicate_row_inline_without_base64_in_its_chunks() {
    let Some(db) = seeded("image_backfill_left_inline", 1).await else { return };
    let walk_name = format!("test-{}", db.uid);
    let png = screenshot(Uuid::new_v4().as_u128(), 30_000);
    let inline = read_result("toolu_inline_dup", &png);
    let (moved, _) = extract(&inline).unwrap();
    let kept = db.insert(0, &moved).await;
    let id = db.insert(0, &inline).await;
    let needle = &b64(&png)[..64];

    let w = walk(&walk_name, 20, &db.sessions);
    set_cursor(&db.pool, &walk_name, kept).await;
    let first = w.step(&db.pool).await.unwrap();
    assert_eq!((first.rewritten, first.left_inline), (0, 1));
    assert_eq!(db.payload(id).await, inline, "a duplicate keeps its inline payload");
    assert!(!chunks_have(&db, id, needle).await, "its chunks carry no base64");

    set_cursor(&db.pool, &walk_name, kept).await;
    let again = w.step(&db.pool).await.unwrap();
    assert_eq!((again.rewritten, again.left_inline), (0, 1), "a re-run writes nothing");
    db.cleanup(&[sha(&png)], Some(&walk_name)).await;
}
