//! `/api/v1/drafts` — the per-user draft store.
//!
//! Every statement filters on `user_id = ctx.user_id`, so a key another user
//! owns reads as absent rather than as someone else's text; there is no
//! per-object guard to get wrong.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::drafts::{
    DRAFT_CAP_PER_USER, DRAFT_KEY_MAX, DRAFT_TEXT_MAX, Draft, DraftList, PutDraftRequest,
};

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;

/// Reject a key/body that is out of bounds before it reaches SQL.
fn validate(key: &str, text: &str) -> Result<(), AppError> {
    if key.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "draft key must not be empty"));
    }
    if key.chars().count() > DRAFT_KEY_MAX {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("draft key longer than {DRAFT_KEY_MAX} characters"),
        ));
    }
    if text.len() > DRAFT_TEXT_MAX {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("draft longer than {DRAFT_TEXT_MAX} bytes"),
        ));
    }
    Ok(())
}

pub async fn list_drafts<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<DraftList>, AppError> {
    let rows = sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT key, text, updated_at FROM user_drafts WHERE user_id = $1 \
         ORDER BY updated_at DESC",
    )
    .bind(ctx.user_id)
    .fetch_all(state.pool())
    .await?;

    let drafts =
        rows.into_iter().map(|(key, text, updated_at)| Draft { key, text, updated_at }).collect();
    Ok(Json(DraftList { drafts }))
}

pub async fn get_draft<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(key): Path<String>,
) -> Result<Json<Draft>, AppError> {
    validate(&key, "")?;
    let row = sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>)>(
        "SELECT text, updated_at FROM user_drafts WHERE user_id = $1 AND key = $2",
    )
    .bind(ctx.user_id)
    .bind(&key)
    .fetch_optional(state.pool())
    .await?;

    let (text, updated_at) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such draft"))?;
    Ok(Json(Draft { key, text, updated_at }))
}

/// Upsert a draft, or delete it when the body is empty so a cleared composer
/// leaves no row behind. `<redacted>` spans are stored masked: drafts and
/// prompt history share this table.
pub async fn put_draft<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(key): Path<String>,
    Json(body): Json<PutDraftRequest>,
) -> Result<StatusCode, AppError> {
    validate(&key, &body.text)?;
    if body.text.is_empty() {
        return delete_draft(State(state), Extension(ctx), Path(key)).await;
    }
    let text = cctui_crypto::redact::mask_user_spans(&body.text);
    let mut tx = state.pool().begin().await?;
    sqlx::query(
        "INSERT INTO user_drafts (user_id, key, text, updated_at) VALUES ($1, $2, $3, now()) \
         ON CONFLICT (user_id, key) DO UPDATE SET text = EXCLUDED.text, updated_at = now()",
    )
    .bind(ctx.user_id)
    .bind(&key)
    .bind(text.as_ref())
    .execute(&mut *tx)
    .await?;
    evict_over_cap(&mut tx, ctx.user_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Drop the least recently updated drafts past [`DRAFT_CAP_PER_USER`].
///
/// Runs in the put's transaction, after the upsert: the row just written is the
/// newest, so a put is never the thing evicted and never fails for being over
/// the cap. `key` breaks ties so concurrent puts in the same statement-timestamp
/// evict deterministically.
async fn evict_over_cap(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: uuid::Uuid,
) -> Result<(), AppError> {
    let over = i64::try_from(DRAFT_CAP_PER_USER).unwrap_or(i64::MAX);
    sqlx::query(
        "DELETE FROM user_drafts WHERE user_id = $1 AND key IN ( \
           SELECT key FROM user_drafts WHERE user_id = $1 \
            ORDER BY updated_at DESC, key ASC OFFSET $2 \
         )",
    )
    .bind(user_id)
    .bind(over)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn delete_draft<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM user_drafts WHERE user_id = $1 AND key = $2")
        .bind(ctx.user_id)
        .bind(&key)
        .execute(state.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn validate_rejects_an_empty_or_overlong_key_and_an_overlong_body() {
        assert_eq!(validate("", "x").unwrap_err().status(), StatusCode::BAD_REQUEST);
        let long_key = "k".repeat(DRAFT_KEY_MAX + 1);
        assert_eq!(validate(&long_key, "x").unwrap_err().status(), StatusCode::BAD_REQUEST);
        let long_text = "t".repeat(DRAFT_TEXT_MAX + 1);
        assert_eq!(validate("k", &long_text).unwrap_err().status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(validate("k", &"t".repeat(DRAFT_TEXT_MAX)).is_ok());
    }
}
