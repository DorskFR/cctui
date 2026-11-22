//! Workflow completion evidence belongs to the interactive workspace.
use crate::store::dispatch_records::{self as core, Listing};
use cctui_proto::api::FailedDispatch;
use uuid::Uuid;

/// Preserve full-server failure cards after a workflow guard completes.
pub async fn list(
    pool: &sqlx::PgPool,
    listing: Listing,
) -> Result<Vec<FailedDispatch>, sqlx::Error> {
    let completed: Vec<String> = sqlx::query_scalar(
        "SELECT g.session_id FROM guard_steps g JOIN dispatch_records r ON r.session_id=g.session_id \
         WHERE ($1::uuid IS NULL OR r.user_id=$1) AND r.created_at > now() - interval '7 days' \
         AND (g.state->>'step')::bigint < 0",
    ).bind(listing.owner).fetch_all(pool).await?;
    core::list_with_completed(pool, listing, &completed).await
}

/// Dismiss the same workspace-aware list the caller sees.
pub async fn dismiss_all(
    pool: &sqlx::PgPool,
    viewer: Uuid,
    owner: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    let shown =
        list(pool, Listing { owner, viewer: Some(viewer), include_dismissed: false }).await?;
    let ids: Vec<String> = shown.into_iter().map(|record| record.session_id).collect();
    core::dismiss_many(pool, viewer, &ids).await
}
