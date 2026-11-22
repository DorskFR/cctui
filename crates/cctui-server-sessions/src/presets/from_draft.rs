use axum::Json;
use axum::http::StatusCode;
use cctui_proto::api::SpawnRequest;
use cctui_proto::api::presets::{
    LaunchPreset, LaunchPresetRequest, RepeatDraftRequest, SpawnTemplate,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;
use crate::routes::spawn::{DRAFT_SEALED_ENV, DRAFT_SEALED_PROMPT};
use crate::store::sessions::SessionRowStatus;

const DEFAULT_NAME: &str = "Scheduled draft";

struct Draft {
    payload: SpawnRequest,
    sealed_prompt: Option<String>,
    sealed_env: Option<String>,
}

async fn read(pool: &PgPool, draft_id: &str) -> Result<Draft, AppError> {
    let row: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT status, metadata FROM sessions WHERE id = $1")
            .bind(draft_id)
            .fetch_optional(pool)
            .await?;
    let Some((status, metadata)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    };
    if SessionRowStatus::parse(&status) != Some(SessionRowStatus::Draft) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session is not a draft"));
    }
    let payload = metadata
        .get("draft")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "corrupt draft payload"))?;
    let text = |key: &str| metadata.get(key).and_then(serde_json::Value::as_str).map(str::to_owned);
    Ok(Draft {
        payload,
        sealed_prompt: text(DRAFT_SEALED_PROMPT),
        sealed_env: text(DRAFT_SEALED_ENV),
    })
}

fn named(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|n| !n.is_empty())
}

async fn account_pick(
    pool: &PgPool,
    owner: Uuid,
    payload: &SpawnRequest,
) -> Result<(Option<Uuid>, Option<Uuid>, bool), AppError> {
    if payload.no_account {
        return Ok((None, None, true));
    }
    if let Some(name) = named(payload.account.as_deref()) {
        let id: Option<Uuid> = sqlx::query_scalar(
            "SELECT a.id FROM accounts a WHERE a.name = $1 \
               AND (a.user_id = $2 OR EXISTS ( \
                   SELECT 1 FROM resource_shares s \
                    WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                      AND s.grantee_id = $2 AND s.revoked_at IS NULL)) \
             ORDER BY a.user_id = $2 DESC LIMIT 1",
        )
        .bind(name)
        .bind(owner)
        .fetch_optional(pool)
        .await?;
        return id.map(|id| (Some(id), None, false)).ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                format!("the draft's account {name:?} is not one a schedule of yours can use"),
            )
        });
    }
    if let Some(name) = named(payload.pool.as_deref()) {
        let id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM account_pools WHERE name = $1 AND user_id = $2")
                .bind(name)
                .bind(owner)
                .fetch_optional(pool)
                .await?;
        return id.map(|id| (None, Some(id), false)).ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                format!("the draft's account pool {name:?} is not one of yours"),
            )
        });
    }
    Ok((None, None, false))
}

fn template_of(payload: &SpawnRequest, card_labels: Vec<String>) -> SpawnTemplate {
    let mut label_ids = payload.label_ids.clone();
    for id in card_labels {
        if !label_ids.contains(&id) {
            label_ids.push(id);
        }
    }
    SpawnTemplate {
        effort: payload.effort.clone(),
        service_tier: payload.service_tier.clone(),
        label_ids,
        context: payload.context.clone(),
        profile_id: payload.profile_id,
        auto_archive: payload.auto_archive,
        attachment_names: Vec::new(),
    }
}

fn request_of(
    draft: &Draft,
    machine: Uuid,
    account: (Option<Uuid>, Option<Uuid>, bool),
    card_labels: Vec<String>,
    body: &RepeatDraftRequest,
) -> LaunchPresetRequest {
    let p = &draft.payload;
    let name = body.name.as_deref().or(p.name.as_deref()).map(str::trim).filter(|n| !n.is_empty());
    LaunchPresetRequest {
        name: name.unwrap_or(DEFAULT_NAME).to_owned(),
        prompt: p.prompt.clone().unwrap_or_default(),
        machine_id: Some(machine),
        working_dir: Some(p.working_dir.clone()),
        adapter_id: p.adapter_id.clone(),
        model: p.model.clone(),
        permission_mode: p.permission_mode,
        account_id: account.0,
        pool_id: account.1,
        no_account: account.2,
        cron: Some(body.cron.clone()),
        timezone: body.timezone.clone(),
        enabled: body.enabled,
        overlap: body.overlap,
        template: Some(template_of(p, card_labels)),
        ..LaunchPresetRequest::default()
    }
}

/// The schedule belongs to the draft machine's owner, and replaces the draft.
pub async fn repeat<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    draft_id: &str,
    body: &RepeatDraftRequest,
) -> Result<LaunchPreset, AppError> {
    let draft = read(state.pool(), draft_id).await?;
    let (machine, owner) = state
        .resolve_owned_machine(ctx, &draft.payload.machine_id)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let account = account_pick(state.pool(), owner, &draft.payload).await?;
    let card_labels = crate::spawn_labels::draft_label_ids(state.pool(), draft_id).await;
    let req = request_of(&draft, machine, account, card_labels, body);
    let valid = super::validate(&req).map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    let created = super::store::create(state.pool(), owner, &valid)
        .await
        .map_err(crate::routes::presets::write_error)?;
    let from = super::store::FromDraft {
        draft_id,
        sealed_prompt: draft.sealed_prompt.as_deref(),
        env_keys: &draft.payload.env_keys,
        sealed_env: draft.sealed_env.as_deref(),
    };
    if let Err(e) = super::store::adopt_draft(state.pool(), created.id, &from).await {
        let _ = super::store::delete(state.pool(), Some(owner), created.id).await;
        return Err(e.into());
    }
    sqlx::query("DELETE FROM sessions WHERE id = $1 AND status = 'draft'")
        .bind(draft_id)
        .execute(state.pool())
        .await?;
    tracing::info!(draft = %draft_id, preset = %created.id, "draft turned into a schedule");
    super::store::get(state.pool(), Some(owner), created.id)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "the new schedule is gone"))
}
