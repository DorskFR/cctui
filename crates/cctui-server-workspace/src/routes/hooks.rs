use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{MethodRouter, post};
use axum::{Extension, Json};
use serde_json::Value;
use uuid::Uuid;

use cctui_proto::api::webhooks::{
    Webhook, WebhookDelivery, WebhookOutcome, WebhookReceipt, WebhookRequest, WebhookSecret,
};

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::hooks::store::{self, Claim, Receiver, WriteError};
use crate::hooks::{self, ValidHook};
use crate::inbox::Source;
use crate::inbox::store::{self as inbox, NewEvent};
use crate::routes::inbox::DEFAULT_TOPIC;
use crate::state::AppState;

type ApiResult<T> = Result<T, AppError>;

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no webhook with that id")
}

fn write_error(e: WriteError) -> AppError {
    match e {
        WriteError::Invalid(msg) => AppError::new(StatusCode::BAD_REQUEST, msg),
        WriteError::NameTaken => {
            AppError::new(StatusCode::CONFLICT, "you already have a webhook with that name")
        }
        WriteError::Db(e) => e.into(),
    }
}

fn located(state: &AppState, hook: Webhook) -> Webhook {
    Webhook { url: hooks::url(&state.config.external_url, hook.id), ..hook }
}

fn issued(state: &AppState, webhook: Webhook, secret: String) -> WebhookSecret {
    WebhookSecret { webhook: located(state, webhook), secret }
}

fn may_write(ctx: &AuthContext) -> ApiResult<()> {
    ctx.requires(Scope::Dispatch)
        .map_err(|code| AppError::new(code, "changing a webhook needs the dispatch scope"))
}

fn writable(ctx: &AuthContext, req: &WebhookRequest) -> ApiResult<ValidHook> {
    may_write(ctx)?;
    hooks::validate(req).map_err(|msg| AppError::new(StatusCode::BAD_REQUEST, msg))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<Vec<Webhook>>> {
    let found = store::list(&state.pool, ctx.owner_filter()).await?;
    Ok(Json(found.into_iter().map(|hook| located(&state, hook)).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<WebhookRequest>,
) -> ApiResult<(StatusCode, Json<WebhookSecret>)> {
    let hook = writable(&ctx, &req)?;
    let (webhook, secret) =
        store::create(&state.pool, ctx.user_id, &hook).await.map_err(write_error)?;
    Ok((StatusCode::CREATED, Json(issued(&state, webhook, secret))))
}

pub async fn get_one(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Webhook>> {
    let hook = store::get(&state.pool, ctx.owner_filter(), id).await?.ok_or_else(not_found)?;
    Ok(Json(located(&state, hook)))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<WebhookRequest>,
) -> ApiResult<Json<Webhook>> {
    let hook = writable(&ctx, &req)?;
    let hook = store::update(&state.pool, ctx.owner_filter(), id, &hook)
        .await
        .map_err(write_error)?
        .ok_or_else(not_found)?;
    Ok(Json(located(&state, hook)))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    may_write(&ctx)?;
    if store::delete(&state.pool, ctx.owner_filter(), id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

pub async fn rotate_secret(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WebhookSecret>> {
    may_write(&ctx)?;
    let (webhook, secret) =
        store::rotate(&state.pool, ctx.owner_filter(), id).await?.ok_or_else(not_found)?;
    Ok(Json(issued(&state, webhook, secret)))
}

pub async fn deliveries(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<WebhookDelivery>>> {
    store::deliveries(&state.pool, ctx.owner_filter(), id).await?.map(Json).ok_or_else(not_found)
}

fn header<'a>(headers: &'a HeaderMap, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| headers.get(*name)?.to_str().ok())
}

async fn refuse(
    state: &AppState,
    hook: &Receiver,
    delivery: Option<&str>,
    outcome: WebhookOutcome,
    code: StatusCode,
    detail: &str,
) -> AppError {
    if let Err(e) = store::log_refusal(&state.pool, hook.id, delivery, outcome, detail).await {
        tracing::warn!(hook = %hook.id, "webhook: logging a refusal failed: {e}");
    }
    AppError::new(code, detail)
}

/// What the sessions this hook reached in its window cost, priced as the
/// session list prices them.
async fn spent_usd(state: &AppState, hook: &Receiver) -> sqlx::Result<f64> {
    let sessions = store::budget_sessions(&state.pool, hook).await?;
    if sessions.is_empty() {
        return Ok(0.0);
    }
    let rows: Vec<(String, Option<String>, i64, i64, i64)> = sqlx::query_as(
        "SELECT session_id, NULLIF(model, ''), input_tokens, cache_read_tokens, output_tokens \
         FROM session_usage_totals WHERE session_id = ANY($1)",
    )
    .bind(&sessions)
    .fetch_all(&state.pool)
    .await?;
    let catalogs = crate::routes::sessions::session_catalogs(state, &sessions).await;
    Ok(rows
        .into_iter()
        .map(|(session_id, model, input, cached_input, output)| {
            let usage = crate::cost::TokenUsage { input, cached_input, output };
            crate::cost::tallies_cost_usd(catalogs.get(&session_id), &[(model, usage)])
        })
        .sum())
}

/// `Some(reason)` when the hook has a dollar cap and its runs reached it.
async fn over_budget(state: &AppState, hook: &Receiver) -> sqlx::Result<Option<String>> {
    let Some(cap) = hook.max_usd else { return Ok(None) };
    let spent = spent_usd(state, hook).await?;
    Ok((spent >= cap).then(|| {
        format!(
            "over budget: its runs cost ${spent:.2} of the ${cap:.2} allowed per {} hours",
            hook.budget_window_hours
        )
    }))
}

async fn queue(
    state: &AppState,
    hook: &Receiver,
    delivery: &str,
    event: &str,
    payload: Value,
) -> sqlx::Result<Uuid> {
    let lane_id =
        crate::inbox::preset::lane(&state.pool, hook.user_id, hook.preset_id, DEFAULT_TOPIC)
            .await?;
    let body = hooks::render(&hook.prompt_template, &payload, event, delivery);
    let source_id = format!("{}:{delivery}", hook.id);
    let stored = serde_json::json!({
        "hook_id": hook.id,
        "delivery_id": delivery,
        "event": event,
        "raw": payload,
    });
    let new = NewEvent {
        lane_id,
        user_id: hook.user_id,
        source: Source::Webhook,
        source_id: Some(&source_id),
        body: &body,
        payload: Some(&stored),
        not_before: None,
        expires_at: None,
    };
    let inserted = inbox::insert_event(&state.pool, &new).await?;
    if !inserted.deduplicated {
        crate::inbox::deliver::wake_lane(state, lane_id);
    }
    Ok(inserted.id)
}

/// Authenticated by the body's HMAC alone: the caller is a third party that
/// holds the hook's secret, never a cctui token.
pub async fn receive(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<WebhookReceipt>)> {
    if body.len() > hooks::MAX_BODY_BYTES {
        let detail = format!("the body is larger than {} bytes", hooks::MAX_BODY_BYTES);
        return Err(AppError::new(StatusCode::PAYLOAD_TOO_LARGE, detail));
    }
    let hook = store::receiver(&state.pool, id).await?.ok_or_else(not_found)?;
    let sent_id = hooks::token(header(&headers, &hooks::DELIVERY_HEADERS));
    let sent_id = Some(sent_id.as_str()).filter(|id| !id.is_empty());
    let rejected = WebhookOutcome::Rejected;
    if let Err(why) =
        hooks::verify(&hook.secret, &body, header(&headers, &[hooks::SIGNATURE_HEADER]))
    {
        let code = StatusCode::UNAUTHORIZED;
        return Err(refuse(&state, &hook, sent_id, rejected, code, why).await);
    }
    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(payload) if payload.is_object() => payload,
        _ => {
            let detail = "the body must be a JSON object";
            let code = StatusCode::BAD_REQUEST;
            return Err(refuse(&state, &hook, sent_id, rejected, code, detail).await);
        }
    };
    let digest = hooks::body_digest(&body);
    let delivery = hooks::delivery_id(sent_id, &digest);
    let row = match store::claim(&state.pool, &hook, &delivery, &digest).await? {
        Claim::Granted(row) => row,
        Claim::Duplicate => {
            let detail = "this delivery was already dispatched";
            let duplicate = WebhookOutcome::Duplicate;
            store::log_refusal(&state.pool, hook.id, Some(&delivery), duplicate, detail).await?;
            return Ok((
                StatusCode::OK,
                Json(WebhookReceipt { status: duplicate, event_id: None }),
            ));
        }
        Claim::RateLimited => {
            let detail = format!("more than {} deliveries a minute", hook.rate_per_minute);
            let (outcome, code) = (WebhookOutcome::RateLimited, StatusCode::TOO_MANY_REQUESTS);
            return Err(refuse(&state, &hook, Some(&delivery), outcome, code, &detail).await);
        }
    };
    if let Some(detail) = over_budget(&state, &hook).await? {
        store::unclaim(&state.pool, row).await?;
        let (outcome, code) = (WebhookOutcome::OverBudget, StatusCode::TOO_MANY_REQUESTS);
        return Err(refuse(&state, &hook, Some(&delivery), outcome, code, &detail).await);
    }
    let event = hooks::token(header(&headers, &hooks::EVENT_HEADERS));
    match queue(&state, &hook, &delivery, &event, payload).await {
        Ok(event_id) => {
            store::set_event(&state.pool, row, event_id).await?;
            let status = WebhookOutcome::Dispatched;
            Ok((StatusCode::ACCEPTED, Json(WebhookReceipt { status, event_id: Some(event_id) })))
        }
        Err(e) => {
            store::unclaim(&state.pool, row).await?;
            Err(e.into())
        }
    }
}

/// The body is cut off at the limit while it is read, before the handler runs.
pub fn receive_route() -> MethodRouter<AppState> {
    post(receive).layer(DefaultBodyLimit::max(hooks::MAX_BODY_BYTES))
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
