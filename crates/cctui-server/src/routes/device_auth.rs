//! Device-authorization login.
//!
//! A headless client (`cctui login` over SSH) asks for a code, prints it, and
//! polls. A signed-in browser user approves that code, and the next poll mints
//! the key — once. `start` and `poll` are unauthenticated by necessity: the
//! device has no credential yet, which is the entire point. What protects them
//! is that a request is worthless until a real user approves it, the device
//! code is a 256-bit secret stored only as a hash, codes expire in
//! [`TTL_MINUTES`], and polling faster than [`INTERVAL_SECS`] is refused.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub use cctui_proto::api::device_auth::{
    DeviceAuthDecision, DeviceAuthPoll, DeviceAuthPollRequest, DeviceAuthRequestInfo,
    DeviceAuthStart, DeviceAuthStartRequest, DeviceAuthStatus,
};

use crate::auth::{self, AuthContext};
use crate::error::AppError;
use crate::state::AppState;

pub const TTL_MINUTES: i64 = 10;
pub const INTERVAL_SECS: i64 = 5;
/// Polls closer together than this are refused. Below `INTERVAL_SECS` so a
/// client that honours the advertised interval never trips it on jitter.
const MIN_POLL_GAP_SECS: i64 = 2;

/// A device key is a full-ceiling credential handed to whatever asked for it,
/// so it expires on its own rather than living until someone remembers it.
pub const DEVICE_KEY_DAYS: i64 = 90;

/// `start` is unauthenticated, so the only thing bounding it is this: per
/// client address per minute, and a ceiling on how many requests may be alive
/// at once across the deployment.
pub const START_PER_MIN: usize = 10;
pub const POLL_PER_MIN: usize = 60;
pub const MAX_PENDING: i64 = 500;

/// No vowels (so no code spells a word), and no glyph pair a terminal font
/// renders alike: the user is reading this off one screen and typing it into
/// another.
const CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ23456789";

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: Uuid,
    user_code: String,
    client_name: Option<String>,
    client_ip: Option<String>,
    user_agent: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    last_polled_at: Option<DateTime<Utc>>,
    approved_at: Option<DateTime<Utc>>,
    approved_by: Option<Uuid>,
    denied_at: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
}

impl RequestRow {
    fn status(&self, now: DateTime<Utc>) -> DeviceAuthStatus {
        if self.denied_at.is_some() {
            DeviceAuthStatus::Denied
        } else if self.expires_at <= now || self.claimed_at.is_some() {
            DeviceAuthStatus::Expired
        } else if self.approved_at.is_some() {
            DeviceAuthStatus::Approved
        } else {
            DeviceAuthStatus::Pending
        }
    }

    fn expires_in_secs(&self, now: DateTime<Utc>) -> u32 {
        u32::try_from((self.expires_at - now).num_seconds().max(0)).unwrap_or(0)
    }
}

const SELECT_BY_DEVICE_CODE: &str = "SELECT id, user_code, client_name, client_ip, user_agent, \
     created_at, expires_at, last_polled_at, approved_at, approved_by, denied_at, claimed_at \
     FROM device_auth_requests WHERE device_code_hash = $1";

const SELECT_BY_USER_CODE: &str = "SELECT id, user_code, client_name, client_ip, user_agent, \
     created_at, expires_at, last_polled_at, approved_at, approved_by, denied_at, claimed_at \
     FROM device_auth_requests WHERE user_code = $1";

#[must_use]
pub fn generate_user_code() -> String {
    let raw = *Uuid::new_v4().as_bytes();
    let picked: String = raw
        .iter()
        .take(8)
        .map(|b| char::from(CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()]))
        .collect();
    format!("{}-{}", &picked[..4], &picked[4..])
}

/// Accept what the user typed: case and the separator are cosmetic.
#[must_use]
pub fn normalize_user_code(raw: &str) -> String {
    let stripped: String =
        raw.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect();
    if stripped.len() == 8 { format!("{}-{}", &stripped[..4], &stripped[4..]) } else { stripped }
}

/// The caller's address as the proxy reports it. Absent in a direct-to-server
/// deployment, where every caller shares one throttle bucket.
fn client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    for name in ["x-forwarded-for", "x-real-ip"] {
        if let Some(raw) = headers.get(name).and_then(|v| v.to_str().ok()) {
            let first = raw.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return Some(first.chars().take(64).collect());
            }
        }
    }
    None
}

fn throttle(bucket: &str, ip: Option<&str>, max: usize) -> Result<(), AppError> {
    let key = format!("device-{bucket}:{}", ip.unwrap_or("unknown"));
    if crate::routes::peer::limiter().admit(&key, max, std::time::Instant::now()) {
        return Ok(());
    }
    Err(AppError::new(StatusCode::TOO_MANY_REQUESTS, "too many device-login requests, slow down"))
}

/// `POST /api/v1/auth/device/start` — unauthenticated.
pub async fn start(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<DeviceAuthStartRequest>,
) -> Result<Json<DeviceAuthStart>, AppError> {
    let ip = client_ip(&headers);
    throttle("start", ip.as_deref(), START_PER_MIN)?;
    reap_expired(&state.pool).await;

    // Even a throttled flood must not be able to grow the table without bound,
    // and the user_code space is small enough that exhausting it 503s honest
    // logins.
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM device_auth_requests \
         WHERE expires_at > now() AND claimed_at IS NULL AND denied_at IS NULL",
    )
    .fetch_one(&state.pool)
    .await?;
    if pending >= MAX_PENDING {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many device logins are pending, try again shortly",
        ));
    }

    let device_code = auth::mint_secret();
    let hash = auth::sha256_hex(&device_code);
    let expires_at = Utc::now() + Duration::minutes(TTL_MINUTES);
    let client_name = req.client_name.as_deref().map(|n| n.chars().take(80).collect::<String>());
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(200).collect::<String>());

    let mut user_code = String::new();
    // A user code is only 8 symbols; a collision with a live request is rare but
    // must not be fatal.
    for attempt in 0..5 {
        let candidate = generate_user_code();
        let inserted = sqlx::query(
            "INSERT INTO device_auth_requests (device_code_hash, user_code, client_name, \
             expires_at, client_ip, user_agent) VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (user_code) DO NOTHING",
        )
        .bind(&hash)
        .bind(&candidate)
        .bind(client_name.as_deref())
        .bind(expires_at)
        .bind(ip.as_deref())
        .bind(user_agent.as_deref())
        .execute(&state.pool)
        .await?
        .rows_affected();
        if inserted == 1 {
            user_code = candidate;
            break;
        }
        tracing::warn!(attempt, "device auth: user code collision");
    }
    if user_code.is_empty() {
        return Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "could not allocate a device code, try again",
        ));
    }

    let base = state.config.external_url.trim_end_matches('/');
    let verification_uri = format!("{base}/device");
    let verification_uri_complete = format!("{verification_uri}?code={user_code}");
    Ok(Json(DeviceAuthStart {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in_secs: u32::try_from(TTL_MINUTES * 60).unwrap_or(600),
        interval_secs: u32::try_from(INTERVAL_SECS).unwrap_or(5),
    }))
}

/// `POST /api/v1/auth/device/poll` — unauthenticated.
pub async fn poll(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<DeviceAuthPollRequest>,
) -> Result<Json<DeviceAuthPoll>, AppError> {
    throttle("poll", client_ip(&headers).as_deref(), POLL_PER_MIN)?;
    let hash = auth::sha256_hex(&req.device_code);
    let row = sqlx::query_as::<_, RequestRow>(SELECT_BY_DEVICE_CODE)
        .bind(&hash)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "unknown device code"))?;

    let now = Utc::now();
    if row.last_polled_at.is_some_and(|at| now - at < Duration::seconds(MIN_POLL_GAP_SECS)) {
        return Err(AppError::new(StatusCode::TOO_MANY_REQUESTS, "slow down"));
    }
    let _ = sqlx::query("UPDATE device_auth_requests SET last_polled_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(&state.pool)
        .await?;

    let status = row.status(now);
    if status != DeviceAuthStatus::Approved {
        return Ok(Json(DeviceAuthPoll { status, token: None }));
    }

    let Some(user_id) = row.approved_by else {
        return Ok(Json(DeviceAuthPoll { status: DeviceAuthStatus::Expired, token: None }));
    };

    // One transaction: a mint that fails must not leave the request claimed,
    // or the approval is spent with no key issued and the user must start over.
    let mut tx = state.pool.begin().await?;
    if !claim(&mut *tx, row.id).await? {
        return Ok(Json(DeviceAuthPoll { status: DeviceAuthStatus::Expired, token: None }));
    }
    let token = mint_device_token(&mut tx, user_id, row.client_name.as_deref()).await?;
    tx.commit().await?;
    tracing::info!(%user_id, user_code = %row.user_code, "device auth claimed");
    Ok(Json(DeviceAuthPoll { status: DeviceAuthStatus::Approved, token: Some(token) }))
}

/// The one-time claim: the conditional UPDATE is the whole guard, so two polls
/// racing cannot both mint. `true` when this caller won it.
async fn claim(exec: impl sqlx::PgExecutor<'_>, id: Uuid) -> Result<bool, sqlx::Error> {
    let claimed: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE device_auth_requests SET claimed_at = now() \
         WHERE id = $1 AND claimed_at IS NULL AND approved_at IS NOT NULL AND expires_at > now() \
         RETURNING id",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;
    Ok(claimed.is_some())
}

/// Record the browser user's decision. Rows affected is 0 when the code is
/// unknown, expired or already decided.
async fn set_decision(
    pool: &PgPool,
    code: &str,
    user_id: Uuid,
    approve: bool,
) -> Result<u64, sqlx::Error> {
    let result = if approve {
        sqlx::query(
            "UPDATE device_auth_requests SET approved_at = now(), approved_by = $2 \
             WHERE user_code = $1 AND approved_at IS NULL AND denied_at IS NULL \
             AND claimed_at IS NULL AND expires_at > now()",
        )
        .bind(code)
        .bind(user_id)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            "UPDATE device_auth_requests SET denied_at = now() \
             WHERE user_code = $1 AND approved_at IS NULL AND denied_at IS NULL \
             AND expires_at > now()",
        )
        .bind(code)
        .execute(pool)
        .await?
    };
    Ok(result.rows_affected())
}

/// The same credential shape `POST /users/{id}/tokens` mints, with the
/// approving user's own ceiling: a device login grants nothing extra.
async fn mint_device_token(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    client_name: Option<&str>,
) -> Result<String, AppError> {
    let token = auth::user_token(&auth::mint_secret());
    let hash = auth::sha256_hex(&token);
    let preview = auth::token_preview(&token);
    let scopes = crate::store::acls::user_ceiling(&mut *conn, user_id).await?;
    let label = format!("device: {}", client_name.unwrap_or("cctui"));
    let expires_at = Utc::now() + Duration::days(DEVICE_KEY_DAYS);
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, token_preview, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(&label)
    .bind(&preview)
    .bind(expires_at)
    .execute(&mut *conn)
    .await?;
    auth::register_key(
        &mut *conn,
        auth::NewKey {
            user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: Some(&label),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
            expires_at: Some(expires_at),
            passkey_id: None,
        },
        scopes,
    )
    .await?;
    Ok(token)
}

/// `GET /api/v1/auth/device/{user_code}` — what the approval page shows.
pub async fn info(
    State(state): State<AppState>,
    Path(user_code): Path<String>,
) -> Result<Json<DeviceAuthRequestInfo>, AppError> {
    let code = normalize_user_code(&user_code);
    let row = sqlx::query_as::<_, RequestRow>(SELECT_BY_USER_CODE)
        .bind(&code)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "unknown code"))?;
    let now = Utc::now();
    Ok(Json(DeviceAuthRequestInfo {
        user_code: row.user_code.clone(),
        client_name: row.client_name.clone(),
        expires_in_secs: row.expires_in_secs(now),
        status: row.status(now),
        client_ip: row.client_ip.clone(),
        user_agent: row.user_agent.clone(),
        created_at: row.created_at,
    }))
}

/// `POST /api/v1/auth/device/{user_code}/decision` — approve or deny, as the
/// signed-in user whose key the device will receive.
pub async fn decide(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_code): Path<String>,
    Json(req): Json<DeviceAuthDecision>,
) -> Result<StatusCode, AppError> {
    if !auth::is_human_credential(&state.pool, &ctx).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "only a user credential can approve a device login",
        ));
    }
    let code = normalize_user_code(&user_code);
    if set_decision(&state.pool, &code, ctx.user_id, req.approve).await? == 0 {
        return Err(AppError::new(StatusCode::CONFLICT, "code is unknown, expired or decided"));
    }
    tracing::info!(user_id = %ctx.user_id, %code, approve = req.approve, "device auth decided");
    Ok(StatusCode::NO_CONTENT)
}

/// An expired request is already unusable, so it is dropped as soon as the
/// grace period that keeps a racing poll's `Expired` answer honest has passed.
async fn reap_expired(pool: &PgPool) {
    let _ = sqlx::query(
        "DELETE FROM device_auth_requests WHERE expires_at < now() - interval '5 minutes'",
    )
    .execute(pool)
    .await;
}

#[cfg(test)]
mod tests {
    use super::{
        CODE_ALPHABET, DeviceAuthStatus, RequestRow, Utc, generate_user_code, normalize_user_code,
    };
    use axum::http::StatusCode;
    use chrono::Duration;
    use uuid::Uuid;

    fn row() -> RequestRow {
        RequestRow {
            id: Uuid::new_v4(),
            user_code: "BCDF-2345".to_owned(),
            client_name: None,
            client_ip: None,
            user_agent: None,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::minutes(5),
            last_polled_at: None,
            approved_at: None,
            approved_by: None,
            denied_at: None,
            claimed_at: None,
        }
    }

    #[test]
    fn a_user_code_is_readable_off_a_screen() {
        let code = generate_user_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(
            code.bytes().filter(|b| *b != b'-').all(|b| CODE_ALPHABET.contains(&b)),
            "{code} left the alphabet"
        );
    }

    #[test]
    fn typing_is_forgiven_but_a_wrong_length_is_not_reformatted() {
        assert_eq!(normalize_user_code("bcdf 2345"), "BCDF-2345");
        assert_eq!(normalize_user_code("BCDF-2345"), "BCDF-2345");
        assert_eq!(normalize_user_code("bcdf23"), "BCDF23");
    }

    /// `start` is unauthenticated, so without a per-caller bound anyone who can
    /// reach the server can fill the table and exhaust the `user_code` space.
    #[test]
    fn start_is_bounded_per_caller_address() {
        let ip = format!("198.51.100.{}", Uuid::new_v4().as_u128() % 250);
        for i in 0..super::START_PER_MIN {
            assert!(
                super::throttle("start", Some(&ip), super::START_PER_MIN).is_ok(),
                "call {i} is within the window"
            );
        }
        let refused = super::throttle("start", Some(&ip), super::START_PER_MIN)
            .expect_err("the next call is refused");
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);

        // A different caller has its own window.
        let other = format!("198.51.100.{}", (Uuid::new_v4().as_u128() % 250) + 1000);
        assert!(super::throttle("start", Some(&other), super::START_PER_MIN).is_ok());
        // And polling is counted separately from starting.
        assert!(super::throttle("poll", Some(&ip), super::POLL_PER_MIN).is_ok());
    }

    #[test]
    fn the_caller_address_comes_from_the_proxy_headers() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(super::client_ip(&headers), None);
        headers.insert("x-real-ip", "203.0.113.9".parse().unwrap());
        assert_eq!(super::client_ip(&headers).as_deref(), Some("203.0.113.9"));
        // The first hop is the client; the rest are proxies.
        headers.insert("x-forwarded-for", "203.0.113.5, 10.0.0.1".parse().unwrap());
        assert_eq!(super::client_ip(&headers).as_deref(), Some("203.0.113.5"));
    }

    async fn seed(pool: &sqlx::PgPool, code: &str, ttl: Duration) -> (Uuid, Uuid) {
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("device-auth-{user_id}"))
            .bind(format!("h-{user_id}"))
            .execute(pool)
            .await
            .unwrap();
        for scope in ["read", "dispatch"] {
            sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, $2)")
                .bind(user_id)
                .bind(scope)
                .execute(pool)
                .await
                .unwrap();
        }
        let id: (Uuid,) = sqlx::query_as(
            "INSERT INTO device_auth_requests (device_code_hash, user_code, client_name, \
             expires_at) VALUES ($1, $2, 'cctui test', now() + $3::interval) RETURNING id",
        )
        .bind(format!("hash-{}", Uuid::new_v4()))
        .bind(code)
        .bind(format!("{} seconds", ttl.num_seconds()))
        .fetch_one(pool)
        .await
        .unwrap();
        (id.0, user_id)
    }

    async fn test_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    #[tokio::test]
    async fn an_approved_code_can_be_claimed_exactly_once() {
        let name = "an_approved_code_can_be_claimed_exactly_once";
        let Some(pool) = test_pool(name).await else { return };
        let code = super::generate_user_code();
        let (id, user_id) = seed(&pool, &code, Duration::minutes(10)).await;

        assert_eq!(super::set_decision(&pool, &code, user_id, true).await.unwrap(), 1);
        assert_eq!(
            super::set_decision(&pool, &code, user_id, true).await.unwrap(),
            0,
            "a decided request cannot be decided again"
        );
        assert!(super::claim(&pool, id).await.unwrap());
        assert!(!super::claim(&pool, id).await.unwrap(), "the claim is one-time");

        let mut conn = pool.acquire().await.unwrap();
        let token = super::mint_device_token(&mut conn, user_id, Some("cctui test")).await.unwrap();
        drop(conn);
        assert!(token.starts_with("cctui_u_"));
        let scopes: Vec<(String,)> = sqlx::query_as(
            "SELECT a.scope FROM key_acls a JOIN auth_keys k ON k.id = a.key_id \
             WHERE k.key_hash = $1 ORDER BY a.scope",
        )
        .bind(crate::auth::sha256_hex(&token))
        .fetch_all(&pool)
        .await
        .unwrap();
        let scopes: Vec<String> = scopes.into_iter().map(|(s,)| s).collect();
        assert_eq!(scopes, ["dispatch", "read"], "a device key gets the user's ceiling, no more");
    }

    /// A mint that fails must not spend the approval: claim and mint share one
    /// transaction, so rolling it back leaves the request claimable again.
    #[tokio::test]
    async fn a_failed_mint_leaves_the_approval_claimable() {
        let name = "a_failed_mint_leaves_the_approval_claimable";
        let Some(pool) = test_pool(name).await else { return };
        let code = super::generate_user_code();
        let (id, user_id) = seed(&pool, &code, Duration::minutes(10)).await;
        assert_eq!(super::set_decision(&pool, &code, user_id, true).await.unwrap(), 1);

        // Claiming on the pool, as the code did before the mint joined the
        // transaction: the UPDATE commits on its own and a rollback around it
        // changes nothing, which is how an approval got spent with no key.
        let tx = pool.begin().await.unwrap();
        assert!(super::claim(&pool, id).await.unwrap());
        tx.rollback().await.unwrap();
        let leaked: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT claimed_at FROM device_auth_requests WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(leaked.is_some(), "a pool-level claim is not undone by the caller's rollback");
        sqlx::query("UPDATE device_auth_requests SET claimed_at = NULL WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        // Claiming on the transaction the mint shares: a rollback undoes it.
        let mut tx = pool.begin().await.unwrap();
        assert!(super::claim(&mut *tx, id).await.unwrap());
        tx.rollback().await.unwrap();

        let claimed_at: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT claimed_at FROM device_auth_requests WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(claimed_at.is_none(), "a rolled-back mint must not leave the request claimed");

        // And the device's next poll can still complete the login.
        let mut tx = pool.begin().await.unwrap();
        assert!(super::claim(&mut *tx, id).await.unwrap(), "the approval survived");
        let token = super::mint_device_token(&mut tx, user_id, Some("cctui test")).await.unwrap();
        tx.commit().await.unwrap();
        assert!(token.starts_with("cctui_u_"));
    }

    /// A device key is full-ceiling, so it must not be immortal.
    #[tokio::test]
    async fn a_device_key_is_minted_with_an_expiry() {
        let name = "a_device_key_is_minted_with_an_expiry";
        let Some(pool) = test_pool(name).await else { return };
        let code = super::generate_user_code();
        let (_id, user_id) = seed(&pool, &code, Duration::minutes(10)).await;

        let mut conn = pool.acquire().await.unwrap();
        let token = super::mint_device_token(&mut conn, user_id, Some("cctui test")).await.unwrap();
        drop(conn);
        let hash = crate::auth::sha256_hex(&token);

        let key_expiry: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT expires_at FROM auth_keys WHERE key_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        let token_expiry: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT expires_at FROM user_tokens WHERE token_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        for (what, at) in [("auth_keys", key_expiry), ("user_tokens", token_expiry)] {
            let at = at.unwrap_or_else(|| panic!("{what} expiry must be set"));
            let days = (at - Utc::now()).num_days();
            assert!(
                (super::DEVICE_KEY_DAYS - 2..=super::DEVICE_KEY_DAYS).contains(&days),
                "{what} expiry is {days} days out, expected about {}",
                super::DEVICE_KEY_DAYS
            );
        }
    }

    /// The approval page can only warn about an unexpected device if the server
    /// kept something the requester did not choose.
    #[tokio::test]
    async fn the_requester_address_and_agent_reach_the_approval_page() {
        let name = "the_requester_address_and_agent_reach_the_approval_page";
        let Some(pool) = test_pool(name).await else { return };
        let code = super::generate_user_code();
        sqlx::query(
            "INSERT INTO device_auth_requests (device_code_hash, user_code, client_name, \
             expires_at, client_ip, user_agent) \
             VALUES ($1, $2, 'cctui on your-laptop', now() + interval '10 minutes', \
             '203.0.113.7', 'cctui/0.23')",
        )
        .bind(format!("hash-{}", Uuid::new_v4()))
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();

        let row = sqlx::query_as::<_, RequestRow>(super::SELECT_BY_USER_CODE)
            .bind(&code)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.client_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(row.user_agent.as_deref(), Some("cctui/0.23"));
        assert!(row.created_at <= Utc::now());
    }

    #[tokio::test]
    async fn an_expired_code_is_neither_approvable_nor_claimable() {
        let name = "an_expired_code_is_neither_approvable_nor_claimable";
        let Some(pool) = test_pool(name).await else { return };
        let code = super::generate_user_code();
        let (id, user_id) = seed(&pool, &code, Duration::seconds(-1)).await;

        assert_eq!(super::set_decision(&pool, &code, user_id, true).await.unwrap(), 0);
        let _ = sqlx::query(
            "UPDATE device_auth_requests SET approved_at = now(), approved_by = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!super::claim(&pool, id).await.unwrap(), "expiry outranks approval");
    }

    #[tokio::test]
    async fn a_denied_code_stays_denied() {
        let name = "a_denied_code_stays_denied";
        let Some(pool) = test_pool(name).await else { return };
        let code = super::generate_user_code();
        let (id, user_id) = seed(&pool, &code, Duration::minutes(10)).await;

        assert_eq!(super::set_decision(&pool, &code, user_id, false).await.unwrap(), 1);
        assert_eq!(super::set_decision(&pool, &code, user_id, true).await.unwrap(), 0);
        assert!(!super::claim(&pool, id).await.unwrap());
    }

    #[test]
    fn a_claimed_or_expired_request_is_dead_whatever_else_it_is() {
        let now = Utc::now();
        let mut r = row();
        assert_eq!(r.status(now), DeviceAuthStatus::Pending);
        r.approved_at = Some(now);
        assert_eq!(r.status(now), DeviceAuthStatus::Approved);
        r.claimed_at = Some(now);
        assert_eq!(r.status(now), DeviceAuthStatus::Expired);
        r.claimed_at = None;
        r.expires_at = now - Duration::seconds(1);
        assert_eq!(r.status(now), DeviceAuthStatus::Expired);
        r.denied_at = Some(now);
        assert_eq!(r.status(now), DeviceAuthStatus::Denied);
    }
}
