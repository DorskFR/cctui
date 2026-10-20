//! `/accounts/{id}/service-tokens` — long-lived gateway credentials an account
//! owner mints for non-agent services. The secret is returned once at mint;
//! only its hash is stored.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::service_tokens::{
    MintServiceTokenRequest, MintServiceTokenResponse, ServiceTokenRow,
};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;
use crate::store::tokens;

const MAX_LABEL: usize = 80;

async fn owned(state: &AppState, ctx: &AuthContext, id: Uuid) -> Result<(), AppError> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await?;
    found.map(|_| ()).ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))
}

fn label_of(raw: &str) -> Result<&str, AppError> {
    let label = raw.trim();
    if label.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "label required"));
    }
    if label.chars().count() > MAX_LABEL {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "label too long"));
    }
    Ok(label)
}

pub async fn list_service_tokens(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ServiceTokenRow>>, AppError> {
    owned(&state, &ctx, id).await?;
    Ok(Json(tokens::list_service(&state.pool, id).await?))
}

pub async fn mint_service_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<MintServiceTokenRequest>,
) -> Result<(StatusCode, Json<MintServiceTokenResponse>), AppError> {
    owned(&state, &ctx, id).await?;
    let label = label_of(&req.label)?;
    let provider: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM account_providers WHERE id = $1 AND account_id = $2")
            .bind(req.provider_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(provider) = provider else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such provider on this account"));
    };
    let (token, row) = tokens::mint_service(&state.pool, ctx.user_id, provider, label).await?;
    Ok((StatusCode::CREATED, Json(MintServiceTokenResponse { token, row })))
}

pub async fn revoke_service_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    owned(&state, &ctx, id).await?;
    if tokens::revoke_service(&state.pool, id, token_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::new(StatusCode::NOT_FOUND, "no such service token"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::{Extension, Json};
    use cctui_proto::api::service_tokens::MintServiceTokenRequest;
    use uuid::Uuid;

    use crate::auth::AuthContext;
    use crate::state::AppState;

    async fn test_state(tag: &str) -> Option<AppState> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        Some(AppState::for_test(pool))
    }

    fn ctx(user_id: Uuid) -> AuthContext {
        AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes: BTreeSet::new() }
    }

    /// A user with one account and one anthropic provider.
    async fn tenant(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid) {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("svc-{uid}"))
            .bind(format!("hsvc-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        let account: Uuid =
            sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
                .bind(uid)
                .bind(format!("svc-account-{uid}"))
                .fetch_one(pool)
                .await
                .unwrap();
        let provider: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider) \
             VALUES ($1, $2, 'anthropic') RETURNING id",
        )
        .bind(uid)
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap();
        (uid, account, provider)
    }

    fn status(e: crate::error::AppError) -> StatusCode {
        axum::response::IntoResponse::into_response(e).status()
    }

    #[tokio::test]
    async fn owner_mints_lists_and_revokes() {
        let Some(state) = test_state("owner_mints_lists_and_revokes").await else { return };
        let (uid, account, provider) = tenant(&state.pool).await;

        let (code, Json(minted)) = super::mint_service_token(
            State(state.clone()),
            Extension(ctx(uid)),
            Path(account),
            Json(MintServiceTokenRequest { label: "  translator ".into(), provider_id: provider }),
        )
        .await
        .unwrap();
        assert_eq!(code, StatusCode::CREATED);
        assert!(minted.token.starts_with("cctui_s_"));
        assert_eq!(minted.row.label, "translator");

        let Json(listed) =
            super::list_service_tokens(State(state.clone()), Extension(ctx(uid)), Path(account))
                .await
                .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, minted.row.id);

        let code = super::revoke_service_token(
            State(state.clone()),
            Extension(ctx(uid)),
            Path((account, minted.row.id)),
        )
        .await
        .unwrap();
        assert_eq!(code, StatusCode::NO_CONTENT);
        let Json(listed) =
            super::list_service_tokens(State(state), Extension(ctx(uid)), Path(account))
                .await
                .unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn another_user_is_refused_on_every_verb() {
        let Some(state) = test_state("another_user_is_refused_on_every_verb").await else {
            return;
        };
        let (owner, account, provider) = tenant(&state.pool).await;
        let (stranger, _, _) = tenant(&state.pool).await;
        let (_, Json(minted)) = super::mint_service_token(
            State(state.clone()),
            Extension(ctx(owner)),
            Path(account),
            Json(MintServiceTokenRequest { label: "svc".into(), provider_id: provider }),
        )
        .await
        .unwrap();

        let err = super::mint_service_token(
            State(state.clone()),
            Extension(ctx(stranger)),
            Path(account),
            Json(MintServiceTokenRequest { label: "svc".into(), provider_id: provider }),
        )
        .await
        .err()
        .expect("stranger cannot mint");
        assert_eq!(status(err), StatusCode::NOT_FOUND);

        let err = super::list_service_tokens(
            State(state.clone()),
            Extension(ctx(stranger)),
            Path(account),
        )
        .await
        .err()
        .expect("stranger cannot list");
        assert_eq!(status(err), StatusCode::NOT_FOUND);

        let err = super::revoke_service_token(
            State(state.clone()),
            Extension(ctx(stranger)),
            Path((account, minted.row.id)),
        )
        .await
        .err()
        .expect("stranger cannot revoke");
        assert_eq!(status(err), StatusCode::NOT_FOUND);
        assert_eq!(
            crate::store::tokens::list_service(&state.pool, account).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn mint_rejects_a_foreign_provider_and_a_blank_label() {
        let Some(state) = test_state("mint_rejects_a_foreign_provider_and_a_blank_label").await
        else {
            return;
        };
        let (uid, account, provider) = tenant(&state.pool).await;
        let (_, _, other_provider) = tenant(&state.pool).await;

        let err = super::mint_service_token(
            State(state.clone()),
            Extension(ctx(uid)),
            Path(account),
            Json(MintServiceTokenRequest { label: "svc".into(), provider_id: other_provider }),
        )
        .await
        .err()
        .expect("a provider of another account is refused");
        assert_eq!(status(err), StatusCode::NOT_FOUND);

        let err = super::mint_service_token(
            State(state),
            Extension(ctx(uid)),
            Path(account),
            Json(MintServiceTokenRequest { label: "   ".into(), provider_id: provider }),
        )
        .await
        .err()
        .expect("a blank label is refused");
        assert_eq!(status(err), StatusCode::BAD_REQUEST);
    }
}
