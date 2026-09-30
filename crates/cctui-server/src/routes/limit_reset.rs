//! "Reset my usage limit" on a provider credential: Codex's "Redeem usage limit
//! reset" credits and Claude Code's `/limit-reset`, claimed server-side with the
//! stored OAuth credential so no interactive CLI is needed. Both are undocumented
//! upstream APIs: best-effort, fail soft, never retried in a loop.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

pub use cctui_proto::api::limit_reset::{LimitResetEntry, LimitResetResponse, LimitResetStatus};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::routes::gateway::{self, Account};
use crate::state::AppState;

/// The endpoint that spends a Codex reset credit, overridable on its own: the
/// `wham` default performs the reset but answers in the credits shape, while
/// codex's own `/api/codex/rate-limit-reset-credits/consume` answers `{outcome}`.
pub fn openai_consume_url() -> String {
    std::env::var("CCTUI_OPENAI_RESET_CREDITS_CONSUME_URL")
        .unwrap_or_else(|_| format!("{}/consume", gateway::openai_reset_credits_url()))
}

pub fn anthropic_profile_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_PROFILE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com/api/oauth/profile".into())
}

pub fn anthropic_reset_url(organization_uuid: &str) -> String {
    let base = std::env::var("CCTUI_ANTHROPIC_API_BASE")
        .unwrap_or_else(|_| "https://api.anthropic.com".into());
    format!("{base}/api/organizations/{organization_uuid}/reset_rate_limits")
}

fn str_at(v: &serde_json::Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::to_owned)
}

/// The windows a Codex credit refills. An unknown `reset_type` restores nothing
/// we can name: a row displays this as fact, so it must not be a guess.
fn codex_restores(reset_type: Option<&str>) -> Vec<String> {
    match reset_type {
        Some("full") => vec!["five_hour".to_owned(), "seven_day".to_owned()],
        Some("five_hour" | "primary") => vec!["five_hour".to_owned()],
        Some("weekly" | "secondary" | "seven_day") => vec!["seven_day".to_owned()],
        _ => Vec::new(),
    }
}

fn is_expired(iso: Option<&str>) -> bool {
    iso.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|t| t <= chrono::Utc::now())
}

/// Why a `cedar_ember` grant cannot be spent; `None` means it is offered.
fn grant_unusable_reason(ce: &serde_json::Value, grant: &serde_json::Value) -> Option<String> {
    let flag = |k: &str| grant.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
    if !ce.get("eligible").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        return Some(str_at(ce, "ineligible_reason").unwrap_or_else(|| "not_eligible".to_owned()));
    }
    if flag("paused") {
        return Some("paused".to_owned());
    }
    if is_expired(grant.get("ends_at").and_then(|v| v.as_str())) {
        return Some("expired".to_owned());
    }
    if !flag("usable_now") {
        return Some("not_usable_now".to_owned());
    }
    None
}

fn cedar_ember_entries(ce: &serde_json::Value, out: &mut Vec<LimitResetEntry>) {
    let grants = ce.get("grants").and_then(|g| g.as_array()).map(Vec::as_slice).unwrap_or_default();
    for g in grants {
        let Some(id) = str_at(g, "id") else { continue };
        let resets_left = g.get("resets_left").and_then(serde_json::Value::as_i64);
        let ends_at = str_at(g, "ends_at");
        if resets_left == Some(0) || is_expired(ends_at.as_deref()) {
            continue;
        }
        let reason = grant_unusable_reason(ce, g);
        out.push(LimitResetEntry {
            kind: "claude",
            id,
            title: str_at(g, "label"),
            restores: g
                .get("clears")
                .and_then(|c| c.as_array())
                .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
            expires_at: ends_at,
            resets_left,
            requires_limit: Some(
                g.get("use_requires_limit").and_then(serde_json::Value::as_bool).unwrap_or(true),
            ),
            usable: reason.is_none(),
            unusable_reason: reason,
        });
    }
}

/// The at-wall program as a row of its own: it has no grant list, so its
/// "expiry" is `next_available_at`. `restores` stays empty — upstream never names
/// the windows it clears.
fn juniper_tide_entry(jt: &serde_json::Value) -> LimitResetEntry {
    let flag = |k: &str| jt.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
    let usable = flag("available") && flag("eligible");
    LimitResetEntry {
        kind: "claude",
        id: "juniper_tide".to_owned(),
        title: None,
        restores: Vec::new(),
        expires_at: str_at(jt, "next_available_at"),
        resets_left: None,
        requires_limit: Some(true),
        usable,
        unusable_reason: (!usable)
            .then(|| str_at(jt, "ineligible_reason").unwrap_or_else(|| "not_available".to_owned())),
    }
}

/// Every reset the account's cached usage payload currently offers, usable
/// first and then by soonest expiry (an entry with no expiry sorts last). Spent
/// and expired offers are dropped; unusable-but-live ones stay so the UI can say
/// why. Reads the same payload as [`limit_reset_status`] — no upstream calls.
pub fn limit_resets(provider: &str, usage: &serde_json::Value) -> Vec<LimitResetEntry> {
    let mut out: Vec<LimitResetEntry> = Vec::new();
    let block = |k: &str| usage.get(k).filter(|v| !v.is_null());
    match provider {
        "openai" => {
            let credits = block("reset_credits")
                .and_then(|c| c.get("credits"))
                .and_then(|c| c.as_array())
                .map(Vec::as_slice)
                .unwrap_or_default();
            for c in credits {
                if c.get("status").and_then(|s| s.as_str()) != Some("available") {
                    continue;
                }
                let Some(id) = str_at(c, "id") else { continue };
                let expires_at = str_at(c, "expires_at");
                if is_expired(expires_at.as_deref()) {
                    continue;
                }
                out.push(LimitResetEntry {
                    kind: "codex",
                    id,
                    title: str_at(c, "title"),
                    restores: codex_restores(c.get("reset_type").and_then(|v| v.as_str())),
                    expires_at,
                    resets_left: None,
                    requires_limit: None,
                    usable: true,
                    unusable_reason: None,
                });
            }
        }
        "anthropic" => {
            if let Some(ce) = block("cedar_ember") {
                cedar_ember_entries(ce, &mut out);
            }
            if let Some(jt) = block("juniper_tide") {
                out.push(juniper_tide_entry(jt));
            }
        }
        _ => {}
    }
    out.sort_by(|a, b| {
        b.usable.cmp(&a.usable).then_with(|| match (&a.expires_at, &b.expires_at) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
    out
}

/// Derive the reset status from the usage JSON `GET /accounts/{id}/usage`
/// serves. `None` when the provider has no reset mechanism or the payload does
/// not mention one (an Anthropic account outside the experiment, a Codex body
/// with no credits block).
pub fn limit_reset_status(provider: &str, usage: &serde_json::Value) -> Option<LimitResetStatus> {
    match provider {
        "openai" => {
            let credits = usage.get("reset_credits")?;
            let available_count =
                credits.get("available_count").and_then(serde_json::Value::as_i64).unwrap_or(0);
            let first = credits.get("credits").and_then(|c| c.as_array()).and_then(|list| {
                list.iter()
                    .find(|c| c.get("status").and_then(|s| s.as_str()) == Some("available"))
                    .or_else(|| list.first())
            });
            Some(LimitResetStatus {
                kind: "codex",
                available: available_count > 0,
                title: first.and_then(|c| str_at(c, "title")),
                credit_id: first.and_then(|c| str_at(c, "id")),
                ineligible_reason: None,
                next_available_at: first.and_then(|c| str_at(c, "expires_at")),
                weekly_resets_at: None,
                resets_left: None,
                requires_limit: None,
                clears: None,
            })
        }
        "anthropic" => {
            // An explicit `null` block must read as absent: `Value::get` would
            // otherwise yield a status with every flag false, i.e. a dead button.
            let block = |k: &str| usage.get(k).filter(|v| !v.is_null());
            if let Some(ce) = block("cedar_ember") {
                return Some(cedar_ember_status(ce));
            }
            let jt = block("juniper_tide")?;
            let flag = |k: &str| jt.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
            Some(LimitResetStatus {
                kind: "claude",
                available: flag("available") && flag("eligible"),
                title: None,
                credit_id: None,
                ineligible_reason: str_at(jt, "ineligible_reason"),
                next_available_at: str_at(jt, "next_available_at"),
                weekly_resets_at: str_at(jt, "weekly_resets_at"),
                resets_left: None,
                requires_limit: None,
                clears: None,
            })
        }
        _ => None,
    }
}

/// The grant a `cedar_ember` claim would spend: the one `next_grant_id` names.
fn next_grant(ce: &serde_json::Value) -> Option<&serde_json::Value> {
    let id = ce.get("next_grant_id").and_then(|v| v.as_str())?;
    ce.get("grants")?.as_array()?.iter().find(|g| g.get("id").and_then(|v| v.as_str()) == Some(id))
}

fn grant_is_offered(grant: &serde_json::Value) -> bool {
    let flag = |k: &str| grant.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
    let not_expired = grant
        .get("ends_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_none_or(|t| t > chrono::Utc::now());
    let resets_left =
        grant.get("resets_left").and_then(serde_json::Value::as_i64).unwrap_or_default();
    flag("usable_now") && !flag("paused") && not_expired && resets_left > 0
}

/// The `cedar_ember` grant program: promotional resets with a label and an
/// expiry, claimable before hitting a limit when `use_requires_limit` is false.
fn cedar_ember_status(ce: &serde_json::Value) -> LimitResetStatus {
    let eligible = ce.get("eligible").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let grant = next_grant(ce);
    LimitResetStatus {
        kind: "claude",
        available: eligible && grant.is_some_and(grant_is_offered),
        title: grant.and_then(|g| str_at(g, "label")),
        credit_id: grant.and_then(|g| str_at(g, "id")),
        ineligible_reason: str_at(ce, "ineligible_reason"),
        next_available_at: grant.and_then(|g| str_at(g, "ends_at")),
        weekly_resets_at: str_at(ce, "weekly_resets_at"),
        resets_left: grant.and_then(|g| g.get("resets_left").and_then(serde_json::Value::as_i64)),
        requires_limit: grant.map(|g| {
            g.get("use_requires_limit").and_then(serde_json::Value::as_bool).unwrap_or(true)
        }),
        clears: grant.and_then(|g| g.get("clears")).and_then(|c| c.as_array()).map(|list| {
            list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>()
        }),
    }
}

/// The CLI's own id shapes. A malformed id is a bug on our side, not something
/// to hand to the claim endpoint.
pub fn valid_grant_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub fn valid_request_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The `cedar_ember` claim body; `None` when either id is malformed, so nothing
/// is sent.
pub fn cedar_ember_claim_body(grant_id: &str, request_id: &str) -> Option<serde_json::Value> {
    (valid_grant_id(grant_id) && valid_request_id(request_id)).then(|| {
        serde_json::json!({
            "program": "cedar_ember",
            "grant_id": grant_id,
            "request_id": request_id,
        })
    })
}

/// Upstream outcomes arrive `camelCase` from the app-server shape and `snake_case`
/// from the HTTP one; the audit row and the UI see one spelling.
pub fn normalize_outcome(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 4);
    for (i, ch) in raw.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The outcome of a 2xx `consume` response. The `wham` endpoint answers with the
/// credits block instead of the app-server's `{outcome}`, so the claimed credit
/// no longer being available is proof it was spent. `error` means the body said
/// nothing at all; `unconfirmed` means the call went through but the body could
/// not be read, and must not read as an upstream rejection.
pub fn consume_outcome(credit_id: Option<&str>, body: &serde_json::Value) -> String {
    if let Some(raw) = body.get("outcome").and_then(|o| o.as_str()) {
        return normalize_outcome(raw);
    }
    if !body.is_object() {
        return "error".to_owned();
    }
    let Some(credits) = gateway::map_reset_credits(body) else {
        return "unconfirmed".to_owned();
    };
    let list = credits["credits"].as_array().cloned().unwrap_or_default();
    let spent = credit_id.map_or_else(
        || credits["available_count"].as_i64() == Some(0),
        |id| {
            !list.iter().any(|c| {
                c.get("id").and_then(serde_json::Value::as_str) == Some(id)
                    && c.get("status").and_then(serde_json::Value::as_str) == Some("available")
            })
        },
    );
    if spent { "reset".to_owned() } else { "unconfirmed".to_owned() }
}

/// Whether the claim may have moved the account's windows, so the cached usage
/// must be dropped rather than served until the next poll.
pub fn invalidates_usage(outcome: &str) -> bool {
    !matches!(
        outcome,
        "error"
            | "unavailable"
            | "already_redeemed"
            | "already_used"
            | "not_limited"
            | "cooldown"
            | "ineligible"
    )
}

/// What a repeat claim on the same credit does with the prior attempt's row.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimPlan {
    /// The credit was already redeemed by us: answer locally, send nothing.
    AlreadyRedeemed { idempotency_key: String },
    /// Send the consume request under this key (the prior attempt's if it did
    /// not settle, else a fresh one).
    Send { idempotency_key: String, reused: bool },
}

pub fn plan_claim(prior: Option<(String, String)>, fresh_key: String) -> ClaimPlan {
    match prior {
        Some((key, outcome))
            if matches!(outcome.as_str(), "reset" | "already_redeemed" | "already_used") =>
        {
            ClaimPlan::AlreadyRedeemed { idempotency_key: key }
        }
        Some((key, _)) => ClaimPlan::Send { idempotency_key: key, reused: true },
        None => ClaimPlan::Send { idempotency_key: fresh_key, reused: false },
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct LimitResetRequest {
    /// Which reset to spend: a Codex credit id or a `cedar_ember` grant id.
    /// Defaults to the first available one from the cached usage.
    #[serde(default)]
    pub credit_id: Option<String>,
}

/// `POST /api/v1/accounts/{id}/limit-reset` — claim a usage-limit reset on a
/// provider credential. `{id}` is the provider-row id. Ownership as for usage:
/// a user may only act on their own providers; admin on any.
pub async fn limit_reset(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    body: Option<Json<LimitResetRequest>>,
) -> Result<Json<LimitResetResponse>, AppError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let provider: Option<String> = sqlx::query_scalar(
        "SELECT provider FROM account_providers \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await?;
    let Some(provider) = provider else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    };
    let Some(acct) = gateway::reload_account(&state, id).await else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    };
    let access_token = gateway::current_access_token(&state, &acct)
        .await
        .map_err(|s| AppError::new(s, "could not obtain an access token for this account"))?;

    let out = match provider.as_str() {
        "openai" => claim_codex(&state, &acct, &access_token, req.credit_id).await,
        "anthropic" => claim_claude(&state, &acct, &access_token, req.credit_id).await,
        _ => {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "this provider has no limit reset"));
        }
    };
    record(&state, id, &out, ctx.user_id).await;
    if invalidates_usage(&out.outcome) {
        state.account_usage_cache.remove(&id);
        // Net-zero upstream: the eviction above already forced the next reader
        // to fetch; doing it here just leaves the cache warm and pushes once.
        if let Ok((p, usage)) = crate::routes::gateway::fetch_usage_with_provider(&state, id).await
        {
            crate::routes::gateway::record_usage_samples(&state, id, usage.as_ref());
            crate::routes::accounts::store_and_broadcast_usage(&state, id, p, usage).await;
        }
    }
    Ok(Json(LimitResetResponse { account_id: id, provider, ..out }))
}

async fn record(state: &AppState, id: Uuid, out: &LimitResetResponse, requested_by: Uuid) {
    if let Err(e) = sqlx::query(
        "INSERT INTO account_limit_resets \
             (provider_id, idempotency_key, credit_id, outcome, requested_by) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&out.idempotency_key)
    .bind(&out.credit_id)
    .bind(&out.outcome)
    .bind(requested_by)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(account = %id, "limit reset audit row failed: {e}");
    }
}

fn blank(outcome: &str, idempotency_key: String) -> LimitResetResponse {
    LimitResetResponse {
        account_id: Uuid::nil(),
        provider: String::new(),
        outcome: outcome.to_owned(),
        credit_id: None,
        next_available_at: None,
        weekly_resets_at: None,
        idempotency_key,
        reused: false,
    }
}

async fn claim_codex(
    state: &AppState,
    acct: &Account,
    access_token: &str,
    credit_id: Option<String>,
) -> LimitResetResponse {
    let credit_id = credit_id.or_else(|| {
        state
            .account_usage_cache
            .get(&acct.id)
            .and_then(|h| h.usage.clone())
            .and_then(|u| limit_reset_status("openai", &u))
            .and_then(|s| s.credit_id)
    });
    let prior: Option<(String, String)> = sqlx::query_as(
        "SELECT idempotency_key, outcome FROM account_limit_resets \
         WHERE provider_id = $1 AND credit_id IS NOT DISTINCT FROM $2 \
         ORDER BY at DESC LIMIT 1",
    )
    .bind(acct.id)
    .bind(&credit_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or_default();
    let (key, reused) = match plan_claim(prior, Uuid::new_v4().to_string()) {
        ClaimPlan::AlreadyRedeemed { idempotency_key } => {
            let mut r = blank("already_redeemed", idempotency_key);
            r.credit_id = credit_id;
            r.reused = true;
            return r;
        }
        ClaimPlan::Send { idempotency_key, reused } => (idempotency_key, reused),
    };
    let mut out = blank("error", key.clone());
    out.credit_id = credit_id.clone();
    out.reused = reused;
    let Some(account_id) = acct.provider_account_id.as_deref() else {
        return out;
    };
    let mut body = serde_json::json!({ "redeem_request_id": key });
    if let Some(c) = &credit_id {
        body["credit_id"] = serde_json::Value::String(c.clone());
    }
    let resp = state
        .http_client
        .post(openai_consume_url())
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header("chatgpt-account-id", account_id)
        .header(reqwest::header::ACCEPT, "*/*")
        .json(&body)
        .send()
        .await;
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!(account = %acct.id, status = %r.status(), "codex limit reset rejected");
            return out;
        }
        Err(e) => {
            tracing::warn!(account = %acct.id, "codex limit reset transport error: {e}");
            return out;
        }
    };
    let json: serde_json::Value = resp.json().await.unwrap_or_default();
    if json.get("outcome").and_then(|o| o.as_str()).is_none() {
        tracing::warn!(account = %acct.id, body = %json, "codex limit reset: 2xx body carries no outcome");
    }
    out.outcome = consume_outcome(credit_id.as_deref(), &json);
    out
}

async fn organization_uuid(state: &AppState, acct: &Account, access_token: &str) -> Option<String> {
    let stored: Option<Option<String>> =
        sqlx::query_scalar("SELECT organization_uuid FROM account_providers WHERE id = $1")
            .bind(acct.id)
            .fetch_optional(&state.pool)
            .await
            .ok()?;
    if let Some(org) = stored.flatten().filter(|s| !s.trim().is_empty()) {
        return Some(org);
    }
    let resp = state
        .http_client
        .get(anthropic_profile_url())
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header(reqwest::header::USER_AGENT, gateway::anthropic_usage_user_agent())
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|e| tracing::warn!(account = %acct.id, "oauth profile transport error: {e}"))
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(account = %acct.id, status = %resp.status(), "oauth profile rejected");
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let org = json.pointer("/organization/uuid")?.as_str()?.to_owned();
    gateway::remember_organization_uuid(state, acct.id, &org).await;
    Some(org)
}

/// Re-read the reset programs right before a claim, so the grant the body names
/// is the one upstream would spend now, and refresh the cached usage with it.
async fn fresh_reset_status(
    state: &AppState,
    acct: &Account,
    access_token: &str,
) -> Option<serde_json::Value> {
    let status = gateway::reset_status(state, acct, access_token, true).await?;
    let mut usage = state
        .account_usage_cache
        .get(&acct.id)
        .and_then(|h| h.usage.clone())
        .unwrap_or_else(|| serde_json::json!({}));
    gateway::merge_reset_status(&mut usage, &status);
    state.account_usage_cache.insert(
        acct.id,
        crate::state::CachedUsage { fetched_at: std::time::Instant::now(), usage: Some(usage) },
    );
    Some(status)
}

/// The grant a `cedar_ember` claim would name, or `None` for the at-wall
/// (`juniper_tide`) program.
pub fn cedar_ember_grant_to_claim(status: &serde_json::Value) -> Option<String> {
    let ce = status.get("cedar_ember").filter(|v| !v.is_null())?;
    if !ce.get("eligible").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        return None;
    }
    let grant = next_grant(ce).filter(|g| grant_is_offered(g))?;
    grant.get("id").and_then(|v| v.as_str()).map(str::to_owned)
}

/// What a Claude claim should send.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaudeTarget {
    /// Spend this `cedar_ember` grant.
    Grant(String),
    /// No grant named: fall back to the at-wall program.
    AtWall,
    /// A specific grant was asked for and upstream is not offering it.
    Unavailable,
}

/// Resolve the grant a claim names. A caller-supplied `requested` id is honoured
/// only while upstream still offers that grant — never silently swapped for the
/// `next_grant_id` one, which would spend a reset the user did not pick.
pub fn claude_claim_target(status: &serde_json::Value, requested: Option<&str>) -> ClaudeTarget {
    let Some(requested) = requested else {
        return cedar_ember_grant_to_claim(status)
            .map_or(ClaudeTarget::AtWall, ClaudeTarget::Grant);
    };
    if requested == "juniper_tide" {
        return ClaudeTarget::AtWall;
    }
    let offered = status
        .get("cedar_ember")
        .filter(|v| !v.is_null())
        .filter(|ce| ce.get("eligible").and_then(serde_json::Value::as_bool).unwrap_or(false))
        .and_then(|ce| ce.get("grants"))
        .and_then(|g| g.as_array())
        .is_some_and(|list| {
            list.iter().any(|g| {
                g.get("id").and_then(|v| v.as_str()) == Some(requested) && grant_is_offered(g)
            })
        });
    if offered { ClaudeTarget::Grant(requested.to_owned()) } else { ClaudeTarget::Unavailable }
}

async fn claim_claude(
    state: &AppState,
    acct: &Account,
    access_token: &str,
    credit_id: Option<String>,
) -> LimitResetResponse {
    let status = fresh_reset_status(state, acct, access_token).await;
    let target = status
        .as_ref()
        .map_or(ClaudeTarget::Unavailable, |s| claude_claim_target(s, credit_id.as_deref()));
    let key = Uuid::new_v4().to_string();
    let mut out = blank("unavailable", key);
    let grant_id = match target {
        ClaudeTarget::Grant(id) => Some(id),
        ClaudeTarget::AtWall => None,
        ClaudeTarget::Unavailable => {
            out.credit_id = credit_id;
            return out;
        }
    };
    let Some(org) = organization_uuid(state, acct, access_token).await else {
        tracing::warn!(account = %acct.id, "claude limit reset: organization uuid unknown");
        return out;
    };
    let body = if let Some(grant_id) = &grant_id {
        let prior: Option<(String, String)> = sqlx::query_as(
            "SELECT idempotency_key, outcome FROM account_limit_resets \
             WHERE provider_id = $1 AND credit_id IS NOT DISTINCT FROM $2 \
             ORDER BY at DESC LIMIT 1",
        )
        .bind(acct.id)
        .bind(grant_id)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or_default();
        let (request_id, reused) = match plan_claim(prior, out.idempotency_key.clone()) {
            ClaimPlan::AlreadyRedeemed { idempotency_key } => {
                let mut r = blank("already_used", idempotency_key);
                r.credit_id = Some(grant_id.clone());
                r.reused = true;
                return r;
            }
            ClaimPlan::Send { idempotency_key, reused } => (idempotency_key, reused),
        };
        out.idempotency_key.clone_from(&request_id);
        out.credit_id = Some(grant_id.clone());
        out.reused = reused;
        let Some(body) = cedar_ember_claim_body(grant_id, &request_id) else {
            tracing::warn!(account = %acct.id, "claude limit reset: malformed cedar_ember ids");
            out.outcome = "error".into();
            return out;
        };
        body
    } else {
        serde_json::json!({ "program": "juniper_tide" })
    };
    let lock = state
        .account_locks
        .entry(acct.id)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;
    let resp = state
        .http_client
        .post(anthropic_reset_url(&org))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header(reqwest::header::USER_AGENT, gateway::anthropic_usage_user_agent())
        .header("anthropic-beta", "oauth-2025-04-20")
        .json(&body)
        .send()
        .await;
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!(account = %acct.id, status = %r.status(), "claude limit reset rejected");
            out.outcome = "error".into();
            return out;
        }
        Err(e) => {
            tracing::warn!(account = %acct.id, "claude limit reset transport error: {e}");
            out.outcome = "error".into();
            return out;
        }
    };
    let json: serde_json::Value = resp.json().await.unwrap_or_default();
    let s = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    out.outcome = s("result").map_or_else(|| "error".to_owned(), |r| normalize_outcome(&r));
    out.next_available_at = s("next_available_at");
    out.weekly_resets_at = s("weekly_resets_at");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn juniper_tide_block_is_surfaced() {
        let usage = serde_json::json!({
            "five_hour": { "utilization": 100.0, "resets_at": "2026-09-05T10:00:00Z" },
            "juniper_tide": {
                "eligible": true, "ineligible_reason": null, "in_experiment": true,
                "arm": "reset", "available": true,
                "next_available_at": "2026-09-12T00:00:00Z",
                "weekly_resets_at": "2026-09-08T00:00:00Z", "resets_per_week": 1
            }
        });
        let s = limit_reset_status("anthropic", &usage).unwrap();
        assert_eq!(s.kind, "claude");
        assert!(s.available);
        assert_eq!(s.ineligible_reason, None);
        assert_eq!(s.next_available_at.as_deref(), Some("2026-09-12T00:00:00Z"));
        assert_eq!(s.weekly_resets_at.as_deref(), Some("2026-09-08T00:00:00Z"));

        let not_at_wall = serde_json::json!({
            "juniper_tide": { "eligible": false, "ineligible_reason": "not_at_wall", "available": true }
        });
        let s = limit_reset_status("anthropic", &not_at_wall).unwrap();
        assert!(!s.available);
        assert_eq!(s.ineligible_reason.as_deref(), Some("not_at_wall"));

        assert!(limit_reset_status("anthropic", &serde_json::json!({ "five_hour": {} })).is_none());
    }

    fn cedar_usage(grant: &serde_json::Value, extra: &serde_json::Value) -> serde_json::Value {
        let mut ce = serde_json::json!({
            "eligible": true,
            "next_grant_id": "opus_55_explore",
            "grants": [grant],
            "weekly_resets_at": "2026-10-05T00:00:00Z"
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            ce[k] = v;
        }
        serde_json::json!({ "five_hour": { "utilization": 12.0 }, "cedar_ember": ce })
    }

    fn usable_grant() -> serde_json::Value {
        serde_json::json!({
            "id": "opus_55_explore",
            "label": "Get extra wiggle room to explore Opus 5.5",
            "resets_total": 3, "resets_left": 2,
            "ends_at": "2126-10-23T00:00:00Z",
            "clears": ["five_hour", "seven_day"],
            "paused": false, "usable_now": true, "use_requires_limit": false,
            "percent_used": { "five_hour": 12 }, "blocking": []
        })
    }

    #[test]
    fn cedar_ember_grant_is_surfaced() {
        let s =
            limit_reset_status("anthropic", &cedar_usage(&usable_grant(), &serde_json::json!({})))
                .unwrap();
        assert_eq!(s.kind, "claude");
        assert!(s.available);
        assert_eq!(s.title.as_deref(), Some("Get extra wiggle room to explore Opus 5.5"));
        assert_eq!(s.credit_id.as_deref(), Some("opus_55_explore"));
        assert_eq!(s.next_available_at.as_deref(), Some("2126-10-23T00:00:00Z"));
        assert_eq!(s.weekly_resets_at.as_deref(), Some("2026-10-05T00:00:00Z"));
        assert_eq!(s.resets_left, Some(2));
        assert_eq!(s.requires_limit, Some(false));
        let clears = ["five_hour".to_owned(), "seven_day".to_owned()];
        assert_eq!(s.clears.as_deref(), Some(clears.as_slice()));
    }

    #[test]
    fn an_unofferable_cedar_ember_grant_is_not_available() {
        let unavailable = |patch: serde_json::Value| {
            let mut grant = usable_grant();
            for (k, v) in patch.as_object().cloned().unwrap_or_default() {
                grant[k] = v;
            }
            let s = limit_reset_status("anthropic", &cedar_usage(&grant, &serde_json::json!({})))
                .unwrap();
            assert!(!s.available, "{patch} should not be claimable");
        };
        unavailable(serde_json::json!({ "paused": true }));
        unavailable(serde_json::json!({ "usable_now": false }));
        unavailable(serde_json::json!({ "resets_left": 0 }));
        unavailable(serde_json::json!({ "ends_at": "2020-01-01T00:00:00Z" }));

        let orphan = limit_reset_status(
            "anthropic",
            &cedar_usage(&usable_grant(), &serde_json::json!({ "next_grant_id": "other" })),
        )
        .unwrap();
        assert!(!orphan.available);
        assert_eq!(orphan.credit_id, None);

        let null_next = serde_json::json!({ "next_grant_id": serde_json::Value::Null });
        let no_next = cedar_usage(&usable_grant(), &null_next);
        assert!(!limit_reset_status("anthropic", &no_next).unwrap().available);

        let ineligible = cedar_usage(
            &usable_grant(),
            &serde_json::json!({ "eligible": false, "ineligible_reason": "no_grant" }),
        );
        let s = limit_reset_status("anthropic", &ineligible).unwrap();
        assert!(!s.available);
        assert_eq!(s.ineligible_reason.as_deref(), Some("no_grant"));
    }

    #[test]
    fn a_null_reset_block_is_absent_rather_than_a_dead_button() {
        let nulls = serde_json::json!({
            "five_hour": { "utilization": 3.0 },
            "juniper_tide": serde_json::Value::Null,
            "cedar_ember": serde_json::Value::Null
        });
        assert!(limit_reset_status("anthropic", &nulls).is_none());
    }

    #[test]
    fn cedar_ember_outranks_juniper_tide() {
        let mut usage = cedar_usage(&usable_grant(), &serde_json::json!({}));
        usage["juniper_tide"] = serde_json::json!({
            "eligible": true, "available": true, "next_available_at": "2026-09-30T00:00:00Z"
        });
        let s = limit_reset_status("anthropic", &usage).unwrap();
        assert_eq!(s.credit_id.as_deref(), Some("opus_55_explore"));
        assert_eq!(s.next_available_at.as_deref(), Some("2126-10-23T00:00:00Z"));
    }

    #[test]
    fn the_cedar_ember_claim_body_carries_the_program_and_the_grant() {
        let body = cedar_ember_claim_body("opus_55_explore", "b3f4-Req_1").unwrap();
        assert_eq!(body["program"], "cedar_ember");
        assert_eq!(body["grant_id"], "opus_55_explore");
        assert_eq!(body["request_id"], "b3f4-Req_1");
        assert!(cedar_ember_claim_body("opus_55_explore", &Uuid::new_v4().to_string()).is_some());

        assert!(cedar_ember_claim_body("Opus 5.5!", "req").is_none());
        assert!(cedar_ember_claim_body("", "req").is_none());
        assert!(cedar_ember_claim_body(&"a".repeat(41), "req").is_none());
        assert!(cedar_ember_claim_body("grant", "").is_none());
        assert!(cedar_ember_claim_body("grant", &"r".repeat(65)).is_none());
        assert!(cedar_ember_claim_body("grant", "req id").is_none());
    }

    #[test]
    fn the_claim_names_only_an_offered_grant() {
        let ce = cedar_usage(&usable_grant(), &serde_json::json!({}))["cedar_ember"].clone();
        let status = serde_json::json!({ "cedar_ember": ce });
        assert_eq!(cedar_ember_grant_to_claim(&status).as_deref(), Some("opus_55_explore"));

        let mut paused = status.clone();
        paused["cedar_ember"]["grants"][0]["paused"] = serde_json::json!(true);
        assert!(cedar_ember_grant_to_claim(&paused).is_none());

        let mut ineligible = status;
        ineligible["cedar_ember"]["eligible"] = serde_json::json!(false);
        assert!(cedar_ember_grant_to_claim(&ineligible).is_none());

        let null_block = serde_json::json!({ "cedar_ember": serde_json::Value::Null });
        assert!(cedar_ember_grant_to_claim(&null_block).is_none());
        assert!(cedar_ember_grant_to_claim(&serde_json::json!({})).is_none());
    }

    #[test]
    fn a_settled_grant_claim_never_re_sends_and_a_retry_reuses_its_request_id() {
        assert_eq!(
            plan_claim(Some(("req-1".into(), "already_used".into())), "fresh".into()),
            ClaimPlan::AlreadyRedeemed { idempotency_key: "req-1".into() }
        );
        assert_eq!(
            plan_claim(Some(("req-1".into(), "unavailable".into())), "fresh".into()),
            ClaimPlan::Send { idempotency_key: "req-1".into(), reused: true }
        );
    }

    #[test]
    fn the_reset_status_url_carries_the_read_flags_and_is_a_get() {
        let url = gateway::anthropic_reset_status_url();
        assert!(url.contains("cedar_ember=1"), "{url}");
        assert!(url.contains("skip_spend=1"), "{url}");
        assert!(!url.contains("reset_rate_limits"), "{url}");
        assert!(!gateway::anthropic_usage_url().contains("cedar_ember"));
    }

    #[test]
    fn only_the_reset_program_keys_are_merged_into_usage() {
        let mut usage =
            serde_json::json!({ "five_hour": { "utilization": 9.0 }, "spend": { "usd": 1 } });
        let status = serde_json::json!({
            "five_hour": { "utilization": 0.0 },
            "cedar_ember": { "eligible": true },
            "juniper_tide": serde_json::Value::Null
        });
        gateway::merge_reset_status(&mut usage, &status);
        assert_eq!(usage["five_hour"]["utilization"], 9.0);
        assert_eq!(usage["spend"]["usd"], 1);
        assert_eq!(usage["cedar_ember"]["eligible"], true);
        assert!(usage.get("juniper_tide").is_none());
    }

    #[test]
    fn codex_reset_credits_are_surfaced() {
        let usage = serde_json::json!({
            "reset_credits": {
                "available_count": 1,
                "credits": [{
                    "id": "cr_1", "status": "available", "reset_type": "full",
                    "granted_at": "2026-09-01T00:00:00Z", "expires_at": "2026-09-08T00:00:00Z",
                    "title": "Full reset (Weekly + 5 hr)"
                }]
            }
        });
        let s = limit_reset_status("openai", &usage).unwrap();
        assert_eq!(s.kind, "codex");
        assert!(s.available);
        assert_eq!(s.title.as_deref(), Some("Full reset (Weekly + 5 hr)"));
        assert_eq!(s.credit_id.as_deref(), Some("cr_1"));

        let none = serde_json::json!({ "reset_credits": { "available_count": 0, "credits": [] } });
        let s = limit_reset_status("openai", &none).unwrap();
        assert!(!s.available);
        assert_eq!(s.credit_id, None);

        assert!(limit_reset_status("openai", &serde_json::json!({ "five_hour": {} })).is_none());
        assert!(limit_reset_status("fireworks", &usage).is_none());
    }

    #[test]
    fn wham_usage_embeds_reset_credits() {
        let body = serde_json::json!({
            "rate_limit": {
                "primary_window": { "used_percent": 42.0, "reset_at": 1_800_000_000 },
                "secondary_window": { "used_percent": 7.5, "reset_at": 1_800_500_000 }
            },
            "rate_limit_reset_credits": {
                "availableCount": 2,
                "credits": [
                    { "id": "a", "status": "available", "resetType": "full", "title": "Full reset" },
                    { "id": "b", "status": "redeemed", "resetType": "full", "title": "Full reset" }
                ]
            }
        });
        let usage = gateway::map_wham_usage(&body).unwrap();
        assert_eq!(usage["reset_credits"]["available_count"], 2);
        assert_eq!(usage["reset_credits"]["credits"][0]["reset_type"], "full");
        assert_eq!(usage["reset_credits"]["credits"][1]["status"], "redeemed");

        let plain = serde_json::json!({ "rate_limit": body["rate_limit"].clone() });
        assert!(gateway::map_wham_usage(&plain).unwrap().get("reset_credits").is_none());

        let counted = gateway::map_reset_credits(&serde_json::json!({
            "credits": [{ "id": "a", "status": "available" }, { "id": "b", "status": "expired" }]
        }))
        .unwrap();
        assert_eq!(counted["available_count"], 1);
    }

    #[test]
    fn repeat_claim_reuses_idempotency_key_and_skips_consume() {
        let fresh = || "fresh".to_owned();
        assert_eq!(
            plan_claim(None, fresh()),
            ClaimPlan::Send { idempotency_key: "fresh".into(), reused: false }
        );
        assert_eq!(
            plan_claim(Some(("k1".into(), "reset".into())), fresh()),
            ClaimPlan::AlreadyRedeemed { idempotency_key: "k1".into() }
        );
        assert_eq!(
            plan_claim(Some(("k1".into(), "already_redeemed".into())), fresh()),
            ClaimPlan::AlreadyRedeemed { idempotency_key: "k1".into() }
        );
        assert_eq!(
            plan_claim(Some(("k1".into(), "error".into())), fresh()),
            ClaimPlan::Send { idempotency_key: "k1".into(), reused: true }
        );
    }

    #[test]
    fn consume_body_without_outcome_is_read_from_the_credits_block() {
        let claimed = "RateLimitResetCredit_6feb0bc2664c8191ae128bfe969348f5";

        assert_eq!(
            consume_outcome(Some(claimed), &serde_json::json!({ "outcome": "reset" })),
            "reset"
        );
        assert_eq!(
            consume_outcome(Some(claimed), &serde_json::json!({ "outcome": "alreadyRedeemed" })),
            "already_redeemed"
        );
        assert_eq!(
            consume_outcome(Some(claimed), &serde_json::json!({ "outcome": "nothingToReset" })),
            "nothing_to_reset"
        );

        let wham = serde_json::json!({
            "rateLimitResetCredits": {
                "availableCount": 1,
                "credits": [{
                    "id": "RateLimitResetCredit_72cf3aea", "status": "available",
                    "resetType": "full", "grantedAt": "2026-09-04T00:00:00Z",
                    "expiresAt": "2026-09-11T00:00:00Z", "title": "Full reset"
                }]
            },
            "rate_limit": {
                "primary_window": { "used_percent": 0.0, "reset_at": 1_800_000_000 },
                "secondary_window": { "used_percent": 0.0, "reset_at": 1_800_500_000 }
            }
        });
        assert_eq!(consume_outcome(Some(claimed), &wham), "reset");

        let redeemed = serde_json::json!({
            "credits": [{ "id": claimed, "status": "redeemed" }]
        });
        assert_eq!(consume_outcome(Some(claimed), &redeemed), "reset");

        let untouched = serde_json::json!({
            "credits": [{ "id": claimed, "status": "available" }]
        });
        assert_eq!(consume_outcome(Some(claimed), &untouched), "unconfirmed");
        assert_eq!(consume_outcome(None, &untouched), "unconfirmed");
        assert_eq!(
            consume_outcome(None, &serde_json::json!({ "availableCount": 0, "credits": [] })),
            "reset"
        );

        assert_eq!(consume_outcome(Some(claimed), &serde_json::json!({})), "unconfirmed");
        assert_eq!(consume_outcome(Some(claimed), &serde_json::Value::Null), "error");
        assert_eq!(consume_outcome(Some(claimed), &serde_json::json!("not json")), "error");
    }

    #[test]
    fn usage_cache_is_dropped_for_every_outcome_that_may_have_reset() {
        assert!(invalidates_usage("reset"));
        assert!(invalidates_usage("unconfirmed"));
        assert!(invalidates_usage("nothing_to_reset"));
        assert!(!invalidates_usage("error"));
        assert!(!invalidates_usage("unavailable"));
        assert!(!invalidates_usage("already_redeemed"));
    }

    #[test]
    fn every_available_codex_credit_is_listed_soonest_first() {
        let usage = serde_json::json!({
            "reset_credits": {
                "available_count": 2,
                "credits": [
                    { "id": "cr_late", "status": "available", "reset_type": "full",
                      "expires_at": "2126-10-30T00:00:00Z", "title": "Full reset" },
                    { "id": "cr_soon", "status": "available", "reset_type": "full",
                      "expires_at": "2126-10-23T00:00:00Z", "title": "Full reset" },
                    { "id": "cr_gone", "status": "redeemed", "reset_type": "full",
                      "expires_at": "2126-10-24T00:00:00Z", "title": "Full reset" }
                ]
            }
        });
        let entries = limit_resets("openai", &usage);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "cr_soon");
        assert_eq!(entries[1].id, "cr_late");
        assert!(entries.iter().all(|e| e.kind == "codex" && e.usable));
        assert_eq!(entries[0].title.as_deref(), Some("Full reset"));
        assert_eq!(entries[0].restores, ["five_hour", "seven_day"]);
        assert_eq!(entries[0].resets_left, None);

        let expired = serde_json::json!({
            "reset_credits": { "credits": [
                { "id": "old", "status": "available", "expires_at": "2020-01-01T00:00:00Z" }
            ] }
        });
        assert!(limit_resets("openai", &expired).is_empty());
        assert!(limit_resets("openai", &serde_json::json!({ "five_hour": {} })).is_empty());
        assert!(limit_resets("fireworks", &usage).is_empty());
    }

    #[test]
    fn an_unknown_codex_reset_type_names_no_window() {
        assert!(codex_restores(None).is_empty());
        assert!(codex_restores(Some("something_new")).is_empty());
        assert_eq!(codex_restores(Some("weekly")), ["seven_day"]);
        assert_eq!(codex_restores(Some("five_hour")), ["five_hour"]);
    }

    #[test]
    fn every_claude_grant_and_the_at_wall_program_are_listed_with_their_reasons() {
        let mut paused = usable_grant();
        paused["id"] = serde_json::json!("paused_grant");
        paused["paused"] = serde_json::json!(true);
        paused["ends_at"] = serde_json::json!("2126-10-30T00:00:00Z");
        let usage = serde_json::json!({
            "cedar_ember": {
                "eligible": true,
                "next_grant_id": "opus_55_explore",
                "grants": [paused, usable_grant()]
            },
            "juniper_tide": {
                "eligible": true, "available": false,
                "ineligible_reason": "not_at_wall",
                "next_available_at": "2126-09-12T00:00:00Z"
            }
        });
        let entries = limit_resets("anthropic", &usage);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].id, "opus_55_explore");
        assert!(entries[0].usable);
        assert_eq!(entries[0].unusable_reason, None);
        assert_eq!(entries[0].restores, ["five_hour", "seven_day"]);
        assert_eq!(entries[0].resets_left, Some(2));
        assert_eq!(entries[0].requires_limit, Some(false));
        assert_eq!(entries[0].expires_at.as_deref(), Some("2126-10-23T00:00:00Z"));

        assert_eq!(entries[1].id, "juniper_tide");
        assert!(!entries[1].usable);
        assert_eq!(entries[1].unusable_reason.as_deref(), Some("not_at_wall"));
        assert_eq!(entries[1].title, None);
        assert!(entries[1].restores.is_empty());

        assert_eq!(entries[2].id, "paused_grant");
        assert!(!entries[2].usable);
        assert_eq!(entries[2].unusable_reason.as_deref(), Some("paused"));
    }

    #[test]
    fn spent_and_expired_claude_grants_are_dropped_and_an_ineligible_program_says_why() {
        let mut spent = usable_grant();
        spent["id"] = serde_json::json!("spent");
        spent["resets_left"] = serde_json::json!(0);
        let mut gone = usable_grant();
        gone["id"] = serde_json::json!("gone");
        gone["ends_at"] = serde_json::json!("2020-01-01T00:00:00Z");
        let usage = serde_json::json!({
            "cedar_ember": { "eligible": true, "grants": [spent, gone] }
        });
        assert!(limit_resets("anthropic", &usage).is_empty());

        let ineligible = serde_json::json!({
            "cedar_ember": {
                "eligible": false, "ineligible_reason": "no_grant",
                "grants": [usable_grant()]
            }
        });
        let entries = limit_resets("anthropic", &ineligible);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].usable);
        assert_eq!(entries[0].unusable_reason.as_deref(), Some("no_grant"));
    }

    #[test]
    fn a_null_reset_block_lists_nothing() {
        let nulls = serde_json::json!({
            "five_hour": { "utilization": 3.0 },
            "juniper_tide": serde_json::Value::Null,
            "cedar_ember": serde_json::Value::Null
        });
        assert!(limit_resets("anthropic", &nulls).is_empty());
        assert!(limit_resets("anthropic", &serde_json::json!({})).is_empty());
        assert!(limit_resets("openai", &serde_json::json!({ "reset_credits": null })).is_empty());
    }

    #[test]
    fn a_requested_claude_grant_is_claimed_instead_of_the_next_one() {
        let mut other = usable_grant();
        other["id"] = serde_json::json!("other_grant");
        let status = serde_json::json!({
            "cedar_ember": {
                "eligible": true,
                "next_grant_id": "opus_55_explore",
                "grants": [usable_grant(), other]
            }
        });

        assert_eq!(
            claude_claim_target(&status, Some("other_grant")),
            ClaudeTarget::Grant("other_grant".into())
        );
        assert_eq!(
            claude_claim_target(&status, None),
            ClaudeTarget::Grant("opus_55_explore".into())
        );
        assert_eq!(claude_claim_target(&status, Some("no_such")), ClaudeTarget::Unavailable);
        assert_eq!(claude_claim_target(&status, Some("juniper_tide")), ClaudeTarget::AtWall);

        let mut paused = status.clone();
        paused["cedar_ember"]["grants"][1]["paused"] = serde_json::json!(true);
        assert_eq!(claude_claim_target(&paused, Some("other_grant")), ClaudeTarget::Unavailable);

        let mut ineligible = status;
        ineligible["cedar_ember"]["eligible"] = serde_json::json!(false);
        assert_eq!(
            claude_claim_target(&ineligible, Some("other_grant")),
            ClaudeTarget::Unavailable
        );

        let at_wall = serde_json::json!({ "juniper_tide": { "eligible": true } });
        assert_eq!(claude_claim_target(&at_wall, None), ClaudeTarget::AtWall);
        assert_eq!(
            claude_claim_target(&at_wall, Some("opus_55_explore")),
            ClaudeTarget::Unavailable
        );
    }

    #[test]
    fn outcomes_normalize_to_snake_case() {
        assert_eq!(normalize_outcome("alreadyRedeemed"), "already_redeemed");
        assert_eq!(normalize_outcome("nothingToReset"), "nothing_to_reset");
        assert_eq!(normalize_outcome("noCredit"), "no_credit");
        assert_eq!(normalize_outcome("reset"), "reset");
        assert_eq!(normalize_outcome("already_used"), "already_used");
    }
}
