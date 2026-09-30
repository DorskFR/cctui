//! `/api/v1/drafts` — the per-user draft store.
//!
//! Every statement filters on `user_id = ctx.user_id`, so a key another user
//! owns reads as absent rather than as someone else's text; there is no
//! per-object guard to get wrong.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::drafts::{DRAFT_KEY_MAX, DRAFT_TEXT_MAX, Draft, DraftList, PutDraftRequest};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

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

pub async fn list_drafts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<DraftList>, AppError> {
    let rows = sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT key, text, updated_at FROM user_drafts WHERE user_id = $1 \
         ORDER BY updated_at DESC",
    )
    .bind(ctx.user_id)
    .fetch_all(&state.pool)
    .await?;

    let drafts =
        rows.into_iter().map(|(key, text, updated_at)| Draft { key, text, updated_at }).collect();
    Ok(Json(DraftList { drafts }))
}

pub async fn get_draft(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(key): Path<String>,
) -> Result<Json<Draft>, AppError> {
    validate(&key, "")?;
    let row = sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>)>(
        "SELECT text, updated_at FROM user_drafts WHERE user_id = $1 AND key = $2",
    )
    .bind(ctx.user_id)
    .bind(&key)
    .fetch_optional(&state.pool)
    .await?;

    let (text, updated_at) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such draft"))?;
    Ok(Json(Draft { key, text, updated_at }))
}

/// Upsert a draft, or delete it when the body is empty so a cleared composer
/// leaves no row behind.
pub async fn put_draft(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(key): Path<String>,
    Json(body): Json<PutDraftRequest>,
) -> Result<StatusCode, AppError> {
    validate(&key, &body.text)?;
    if body.text.is_empty() {
        return delete_draft(State(state), Extension(ctx), Path(key)).await;
    }
    sqlx::query(
        "INSERT INTO user_drafts (user_id, key, text, updated_at) VALUES ($1, $2, $3, now()) \
         ON CONFLICT (user_id, key) DO UPDATE SET text = EXCLUDED.text, updated_at = now()",
    )
    .bind(ctx.user_id)
    .bind(&key)
    .bind(&body.text)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_draft(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM user_drafts WHERE user_id = $1 AND key = $2")
        .bind(ctx.user_id)
        .bind(&key)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use uuid::Uuid;

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

    fn ctx(user_id: Uuid) -> AuthContext {
        AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes: BTreeSet::new() }
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn insert_user(pool: &sqlx::PgPool, tag: &str) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{tag}-{uid}"))
            .bind(format!("h-{tag}-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        uid
    }

    #[tokio::test]
    async fn a_draft_round_trips_and_an_empty_put_deletes_it() {
        let Some(pool) = test_pool("a_draft_round_trips_and_an_empty_put_deletes_it").await else {
            return;
        };
        let uid = insert_user(&pool, "draft-rt").await;
        let state = AppState::for_test(pool);
        let key = format!("cctui_draft_{uid}");

        let put = put_draft(
            State(state.clone()),
            Extension(ctx(uid)),
            Path(key.clone()),
            Json(PutDraftRequest { text: "half a sentence".to_owned() }),
        )
        .await
        .unwrap();
        assert_eq!(put, StatusCode::NO_CONTENT);

        let got =
            get_draft(State(state.clone()), Extension(ctx(uid)), Path(key.clone())).await.unwrap();
        assert_eq!(got.0.text, "half a sentence");
        assert_eq!(got.0.key, key);

        let listed = list_drafts(State(state.clone()), Extension(ctx(uid))).await.unwrap();
        assert!(listed.0.drafts.iter().any(|d| d.key == key));

        put_draft(
            State(state.clone()),
            Extension(ctx(uid)),
            Path(key.clone()),
            Json(PutDraftRequest { text: String::new() }),
        )
        .await
        .unwrap();
        let gone = get_draft(State(state), Extension(ctx(uid)), Path(key)).await;
        assert_eq!(gone.unwrap_err().status(), StatusCode::NOT_FOUND);
    }

    /// The whole point of moving drafts server-side is that they are per user:
    /// knowing another user's key must not read, overwrite or delete their text.
    #[tokio::test]
    async fn a_draft_is_invisible_to_another_user_who_knows_its_key() {
        let Some(pool) = test_pool("a_draft_is_invisible_to_another_user_who_knows_its_key").await
        else {
            return;
        };
        let owner = insert_user(&pool, "draft-owner").await;
        let other = insert_user(&pool, "draft-other").await;
        let state = AppState::for_test(pool);
        let key = format!("cctui_draft_shared_{owner}");

        put_draft(
            State(state.clone()),
            Extension(ctx(owner)),
            Path(key.clone()),
            Json(PutDraftRequest { text: "owner's secret".to_owned() }),
        )
        .await
        .unwrap();

        let peek = get_draft(State(state.clone()), Extension(ctx(other)), Path(key.clone())).await;
        assert_eq!(peek.unwrap_err().status(), StatusCode::NOT_FOUND);

        let listed = list_drafts(State(state.clone()), Extension(ctx(other))).await.unwrap();
        assert!(listed.0.drafts.iter().all(|d| d.key != key));

        delete_draft(State(state.clone()), Extension(ctx(other)), Path(key.clone())).await.unwrap();
        put_draft(
            State(state.clone()),
            Extension(ctx(other)),
            Path(key.clone()),
            Json(PutDraftRequest { text: "trampled".to_owned() }),
        )
        .await
        .unwrap();

        let still = get_draft(State(state), Extension(ctx(owner)), Path(key)).await.unwrap();
        assert_eq!(still.0.text, "owner's secret", "another user's PUT wrote its own row");
    }
}
