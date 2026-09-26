use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{AuthContext, mint_secret, sha256_hex, user_token};
use crate::error::AppError;
use crate::state::AppState;

// ---- /api/v1/users/{id}/tokens ----

#[derive(Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MintTokenRequest {
    pub label: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct MintTokenResponse {
    pub token: String,
    pub label: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

/// Only a human credential may mint a human token: a machine or dispatcher key
/// would otherwise escalate itself. Admin may mint for anyone, a user only for
/// itself.
fn authorize_token_mint(ctx: &AuthContext, human: bool, user_id: Uuid) -> Result<(), &'static str> {
    if !human || ctx.machine_id.is_some() {
        return Err("only a user credential can mint user tokens");
    }
    if !ctx.is_admin() && ctx.user_id != user_id {
        return Err("cannot mint tokens for another user");
    }
    Ok(())
}

/// The minted token never carries more than the caller holds.
fn token_grant(
    ctx: &AuthContext,
    ceiling: &std::collections::BTreeSet<crate::auth::Scope>,
) -> std::collections::BTreeSet<crate::auth::Scope> {
    ceiling.intersection(&ctx.scopes).copied().collect()
}

pub async fn mint_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<MintTokenRequest>,
) -> Result<Json<MintTokenResponse>, AppError> {
    let human = crate::auth::is_human_credential(&state.pool, &ctx).await?;
    authorize_token_mint(&ctx, human, user_id)
        .map_err(|msg| AppError::new(StatusCode::FORBIDDEN, msg))?;

    let ceiling = crate::store::acls::user_ceiling(&state.pool, user_id).await?;
    let grant = token_grant(&ctx, &ceiling);
    let token = user_token(&mint_secret());
    let hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, expires_at, token_preview) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(req.label.as_deref())
    .bind(req.expires_at)
    .bind(&preview)
    .execute(&mut *tx)
    .await?;
    crate::auth::register_key(
        &mut *tx,
        crate::auth::NewKey {
            user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: req.label.as_deref(),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
            expires_at: req.expires_at,
            passkey_id: None,
        },
        grant,
    )
    .await?;
    tx.commit().await?;

    Ok(Json(MintTokenResponse { token, label: req.label, expires_at: req.expires_at }))
}

#[cfg(test)]
mod mint_token_tests {
    use std::collections::BTreeSet;

    use super::{AuthContext, Uuid, authorize_token_mint, token_grant};
    use crate::auth::Scope;

    fn ctx(user_id: Uuid, machine: bool, scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: machine.then(Uuid::new_v4),
            scopes: scopes.iter().copied().collect(),
        }
    }

    #[test]
    fn machine_key_cannot_mint_for_its_own_user() {
        let uid = Uuid::new_v4();
        assert!(authorize_token_mint(&ctx(uid, true, &Scope::all()), true, uid).is_err());
        assert!(authorize_token_mint(&ctx(uid, false, &[Scope::Read]), true, uid).is_ok());
    }

    #[test]
    fn dispatcher_key_cannot_mint_for_its_own_user() {
        let uid = Uuid::new_v4();
        let dispatcher = ctx(uid, false, &[Scope::Read, Scope::Dispatch]);
        assert!(authorize_token_mint(&dispatcher, false, uid).is_err());
    }

    #[test]
    fn only_admin_mints_for_another_user() {
        let target = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert!(authorize_token_mint(&ctx(other, false, &[Scope::Read]), true, target).is_err());
        assert!(authorize_token_mint(&ctx(other, false, &Scope::all()), true, target).is_ok());
    }

    #[test]
    fn minted_grant_is_within_the_callers_scopes() {
        let uid = Uuid::new_v4();
        let ceiling: BTreeSet<Scope> = Scope::all().into_iter().collect();
        let caller = ctx(uid, false, &[Scope::Read, Scope::Dispatch]);
        let grant = token_grant(&caller, &ceiling);
        assert!(grant.is_subset(&caller.scopes));
        assert_eq!(grant, [Scope::Read, Scope::Dispatch].into_iter().collect());

        let narrow: BTreeSet<Scope> = std::iter::once(Scope::Read).collect();
        let admin = ctx(Uuid::new_v4(), false, &Scope::all());
        assert_eq!(token_grant(&admin, &narrow), narrow);
    }
}
