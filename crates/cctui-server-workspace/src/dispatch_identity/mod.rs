//! Kube dispatch identity: Job registration, single-use redeem of a pod's
//! service-account token for its machine key, and the one-shot env read.

mod issuers;
pub mod jwt;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use cctui_proto::api::ApiError;
use cctui_proto::dispatch_identity::{EnvResponse, RedeemRequest, RedeemResponse, RegisterRequest};
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;

/// A queued Job may wait long before its pod starts; past this the
/// registration is dropped and a late pod fails its redeem.
const REGISTRATION_TTL_DAYS: i32 = 7;
const K8S_NAME_MAX: usize = 63;

type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

fn api_err(status: StatusCode, error: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { error: error.into() }))
}

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<ApiError>) {
    tracing::error!(error = %e, "dispatch identity store failed");
    api_err(StatusCode::INTERNAL_SERVER_ERROR, "dispatch identity store unavailable")
}

static ISSUERS: LazyLock<Result<issuers::Issuers, String>> = LazyLock::new(|| {
    let parsed = issuers::Issuers::from_env();
    if let Err(e) = &parsed {
        tracing::error!(error = %e, "dispatch redeem is disabled until the issuer config is fixed");
    }
    parsed
});

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
}

fn is_k8s_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= K8S_NAME_MAX
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn is_env_name(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn check_register(req: &RegisterRequest) -> Result<(), String> {
    if !is_k8s_name(&req.namespace) {
        return Err(format!("namespace `{}` is not a Kubernetes name", req.namespace));
    }
    if !is_k8s_name(&req.job_name) {
        return Err(format!("job_name `{}` is not a Kubernetes name", req.job_name));
    }
    if req.session_id.trim().is_empty() || req.machine_key.is_empty() {
        return Err("session_id and machine_key are required".to_owned());
    }
    if let Some(bad) = req.env.keys().find(|k| !is_env_name(k)) {
        return Err(format!("env name `{bad}` is not a valid variable name"));
    }
    Ok(())
}

async fn dispatcher_of(pool: &PgPool, key: &str) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT d.id, d.user_id FROM dispatchers d \
         JOIN users u ON u.id = d.user_id \
         WHERE d.key_hash = $1 AND d.revoked_at IS NULL AND d.deleted_at IS NULL \
         AND u.revoked_at IS NULL AND u.disabled_at IS NULL",
    )
    .bind(cctui_server_core::auth::sha256_hex(key))
    .fetch_optional(pool)
    .await
}

struct Registration<'a> {
    req: &'a RegisterRequest,
    dispatcher_id: Uuid,
    user_id: Uuid,
    encrypted_machine_key: String,
    encrypted_env: String,
}

/// `false` when the dispatch id is already registered.
async fn insert(pool: &PgPool, r: &Registration<'_>) -> Result<bool, sqlx::Error> {
    sqlx::query("DELETE FROM dispatch_identities WHERE expires_at < now()").execute(pool).await?;
    let done = sqlx::query(
        "INSERT INTO dispatch_identities \
         (dispatch_id, dispatcher_id, user_id, session_id, namespace, job_name, \
          machine_key_hash, encrypted_machine_key, encrypted_env, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now() + make_interval(days => $10)) \
         ON CONFLICT (dispatch_id) DO NOTHING",
    )
    .bind(r.req.dispatch_id)
    .bind(r.dispatcher_id)
    .bind(r.user_id)
    .bind(&r.req.session_id)
    .bind(&r.req.namespace)
    .bind(&r.req.job_name)
    .bind(cctui_server_core::auth::sha256_hex(&r.req.machine_key))
    .bind(&r.encrypted_machine_key)
    .bind(&r.encrypted_env)
    .bind(REGISTRATION_TTL_DAYS)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    namespace: String,
    job_name: String,
    session_id: String,
    redeemed: bool,
    expired: bool,
}

async fn binding(pool: &PgPool, dispatch_id: Uuid) -> Result<Option<Binding>, sqlx::Error> {
    let row: Option<(String, String, String, bool, bool)> = sqlx::query_as(
        "SELECT namespace, job_name, session_id, redeemed_at IS NOT NULL, expires_at <= now() \
         FROM dispatch_identities WHERE dispatch_id = $1",
    )
    .bind(dispatch_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(namespace, job_name, session_id, redeemed, expired)| Binding {
        namespace,
        job_name,
        session_id,
        redeemed,
        expired,
    }))
}

/// Single use: the first redeem takes the encrypted key and clears it; any
/// later one finds nothing.
async fn claim(
    pool: &PgPool,
    dispatch_id: Uuid,
    pod: &jwt::PodIdentity,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE dispatch_identities d SET redeemed_at = now(), issuer = $2, pod_name = $3, \
         pod_uid = $4, encrypted_machine_key = NULL \
         FROM (SELECT dispatch_id, encrypted_machine_key FROM dispatch_identities \
               WHERE dispatch_id = $1 FOR UPDATE) old \
         WHERE d.dispatch_id = old.dispatch_id AND d.redeemed_at IS NULL \
         AND d.expires_at > now() AND old.encrypted_machine_key IS NOT NULL \
         RETURNING old.encrypted_machine_key",
    )
    .bind(dispatch_id)
    .bind(&pod.issuer)
    .bind(&pod.pod_name)
    .bind(&pod.pod_uid)
    .fetch_optional(pool)
    .await
}

/// Single read, by the holder of the redeemed key only.
async fn take_env(
    pool: &PgPool,
    dispatch_id: Uuid,
    machine_key: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE dispatch_identities d SET env_read_at = now(), encrypted_env = NULL \
         FROM (SELECT dispatch_id, encrypted_env FROM dispatch_identities \
               WHERE dispatch_id = $1 FOR UPDATE) old \
         WHERE d.dispatch_id = old.dispatch_id AND d.machine_key_hash = $2 \
         AND d.redeemed_at IS NOT NULL AND d.env_read_at IS NULL \
         AND old.encrypted_env IS NOT NULL \
         RETURNING old.encrypted_env",
    )
    .bind(dispatch_id)
    .bind(cctui_server_core::auth::sha256_hex(machine_key))
    .fetch_optional(pool)
    .await
}

/// `POST /api/v1/dispatch/register`: a dispatcher records the Job it is about
/// to create, before creating it.
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<StatusCode> {
    let key = bearer(&headers)
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "dispatcher key required"))?;
    let (dispatcher_id, user_id) =
        dispatcher_of(&state.pool, key)
            .await
            .map_err(|e| db_err(&e))?
            .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "invalid dispatcher key"))?;
    check_register(&req).map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    let vault = crate::crypto::vault_key();
    let env = serde_json::to_string(&req.env)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, format!("env: {e}")))?;
    let registration = Registration {
        req: &req,
        dispatcher_id,
        user_id,
        encrypted_machine_key: crate::crypto::encrypt(&req.machine_key, &vault),
        encrypted_env: crate::crypto::encrypt(&env, &vault),
    };
    if !insert(&state.pool, &registration).await.map_err(|e| db_err(&e))? {
        return Err(api_err(StatusCode::CONFLICT, "dispatch is already registered"));
    }
    tracing::info!(
        dispatch = %req.dispatch_id, %dispatcher_id, namespace = %req.namespace,
        job = %req.job_name, "dispatch registered"
    );
    Ok(StatusCode::CREATED)
}

fn rejected(dispatch_id: Uuid, why: &jwt::Rejection) -> (StatusCode, Json<ApiError>) {
    tracing::warn!(dispatch = %dispatch_id, reason = %why, "dispatch redeem rejected");
    let status = match why {
        jwt::Rejection::Keys { .. } => StatusCode::BAD_GATEWAY,
        jwt::Rejection::WrongNamespace { .. } | jwt::Rejection::WrongPod { .. } => {
            StatusCode::FORBIDDEN
        }
        _ => StatusCode::UNAUTHORIZED,
    };
    api_err(status, format!("dispatch identity rejected: {why}"))
}

/// `POST /api/v1/dispatch/redeem`: the guard-proxy trades its pod's projected
/// service-account token for the dispatch's machine key, once.
pub async fn redeem(
    State(state): State<AppState>,
    Json(req): Json<RedeemRequest>,
) -> ApiResult<Json<RedeemResponse>> {
    let issuers = ISSUERS.as_ref().map_err(|e| {
        api_err(StatusCode::SERVICE_UNAVAILABLE, format!("dispatch redeem is misconfigured: {e}"))
    })?;
    let id = req.dispatch_id;
    let token = jwt::parse(&req.token).map_err(|e| rejected(id, &e))?;
    let jwks = issuers
        .keys(&state.http_client, token.issuer(), token.kid())
        .await
        .map_err(|e| rejected(id, &e))?;
    let pod =
        jwt::verify(token, &jwks, chrono::Utc::now().timestamp()).map_err(|e| rejected(id, &e))?;
    let found = binding(&state.pool, id).await.map_err(|e| db_err(&e))?;
    let bound = found.ok_or_else(|| api_err(StatusCode::NOT_FOUND, "unknown dispatch"))?;
    if bound.expired {
        return Err(api_err(StatusCode::GONE, "dispatch registration expired"));
    }
    jwt::check_binding(&pod, &bound.namespace, &bound.job_name).map_err(|e| rejected(id, &e))?;
    if bound.redeemed {
        return Err(api_err(StatusCode::CONFLICT, "dispatch identity was already redeemed"));
    }
    let sealed = claim(&state.pool, id, &pod)
        .await
        .map_err(|e| db_err(&e))?
        .ok_or_else(|| api_err(StatusCode::CONFLICT, "dispatch identity was already redeemed"))?;
    let machine_key =
        crate::crypto::decrypt(&sealed, &crate::crypto::vault_key()).ok_or_else(|| {
            api_err(StatusCode::INTERNAL_SERVER_ERROR, "stored machine key is unreadable")
        })?;
    tracing::info!(
        dispatch = %id, session = %bound.session_id, pod = %pod.pod_name, issuer = %pod.issuer,
        service_account = ?pod.service_account, "dispatch identity redeemed"
    );
    Ok(Json(RedeemResponse { machine_key }))
}

/// `GET /api/v1/dispatch/{id}/env`: the payload env, once, to the redeemed key.
pub async fn env(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<EnvResponse>> {
    let key = bearer(&headers)
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "machine key required"))?;
    let sealed =
        take_env(&state.pool, id, key).await.map_err(|e| db_err(&e))?.ok_or_else(|| {
            api_err(StatusCode::NOT_FOUND, "no env for this key (not redeemed, or already read)")
        })?;
    let env: BTreeMap<String, String> =
        crate::crypto::decrypt(&sealed, &crate::crypto::vault_key())
            .and_then(|json| serde_json::from_str(&json).ok())
            .ok_or_else(|| {
                api_err(StatusCode::INTERNAL_SERVER_ERROR, "stored env is unreadable")
            })?;
    Ok(Json(EnvResponse { env }))
}

#[cfg(test)]
mod tests;
