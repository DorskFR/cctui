//! Dispatch grants, request normalization and execution policy enforcement.
use crate::store::dispatch_policies as store;
use axum::Json;
use axum::http::StatusCode;
use cctui_proto::api::admin::{ContextPackGrant, DispatchPolicy, DispatchPolicyRules};
use cctui_proto::api::{ApiError, DispatchRequest};
use std::fmt;
use uuid::Uuid;

fn clean_list(list: &mut Vec<String>) {
    for s in list.iter_mut() {
        *s = s.trim().to_owned();
    }
    list.retain(|s| !s.is_empty());
    list.sort_unstable();
    list.dedup();
}

fn clean_opt(v: &mut Option<String>) {
    *v = v.take().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
}

/// Whether the worker would split a `#subdir` or a path `@ref` out of `url`.
#[must_use]
fn url_embeds_pin(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    url.contains('#') || rest.split_once('/').is_some_and(|(_, path)| path.contains('@'))
}

/// Trim and dedup every list; a pack needs a name and a url, and names are
/// unique.
pub fn normalize(rules: &mut DispatchPolicyRules) -> Result<(), String> {
    clean_list(&mut rules.dispatchers);
    clean_list(&mut rules.profiles);
    clean_list(&mut rules.adapters);
    clean_list(&mut rules.models);
    for pack in &mut rules.context_packs {
        pack.name = pack.name.trim().to_owned();
        pack.url = pack.url.trim().to_owned();
        clean_opt(&mut pack.git_ref);
        clean_opt(&mut pack.subdir);
        if pack.name.is_empty() || pack.url.is_empty() {
            return Err("a context pack needs a name and a url".into());
        }
        if url_embeds_pin(&pack.url) {
            return Err(format!(
                "context pack {:?}: set git_ref/subdir instead of `@`/`#` in the url",
                pack.name
            ));
        }
    }
    let mut names: Vec<&str> = rules.context_packs.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| w[0] == w[1]) {
        return Err("context pack names must be unique".into());
    }
    for (label, limit) in [
        ("max_timeout_minutes", rules.max_timeout_minutes),
        ("max_concurrent", rules.max_concurrent),
        ("daily_cap", rules.daily_cap),
    ] {
        if limit == Some(0) {
            return Err(format!("{label} must be positive or unset"));
        }
    }
    Ok(())
}

/// The policy dimension a dispatch failed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Dispatcher,
    Profile,
    Adapter,
    Model,
    ContextPack,
    Egress,
    Timeout,
    Concurrency,
    DailyCap,
}

impl Rule {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Dispatcher => "dispatcher",
            Self::Profile => "profile",
            Self::Adapter => "adapter",
            Self::Model => "model",
            Self::ContextPack => "context_pack",
            Self::Egress => "egress",
            Self::Timeout => "max_timeout_minutes",
            Self::Concurrency => "max_concurrent",
            Self::DailyCap => "daily_cap",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: Rule,
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dispatch policy violated ({}): {}", self.rule.as_str(), self.detail)
    }
}

fn violation(rule: Rule, detail: impl Into<String>) -> Violation {
    Violation { rule, detail: detail.into() }
}

fn payload_str<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn env_str<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload
        .get("env")
        .and_then(|e| e.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn check_listed(rule: Rule, allowed: &[String], value: Option<&str>) -> Result<(), Violation> {
    if allowed.is_empty() {
        return Ok(());
    }
    let Some(value) = value else {
        return Err(violation(
            rule,
            format!("the payload names no {}; allowed: [{}]", rule.as_str(), allowed.join(", ")),
        ));
    };
    if allowed.iter().any(|a| a == value) {
        return Ok(());
    }
    Err(violation(rule, format!("{value:?} is not allowed; allowed: [{}]", allowed.join(", "))))
}

fn pack_matches(grant: &ContextPackGrant, payload: &serde_json::Value) -> bool {
    let pinned = |pin: Option<&String>, key: &str| {
        pin.is_none_or(|want| env_str(payload, key) == Some(want.as_str()))
    };
    env_str(payload, "CONTEXT_PACK_URL") == Some(grant.url.as_str())
        && pinned(grant.git_ref.as_ref(), "CONTEXT_PACK_REF")
        && pinned(grant.subdir.as_ref(), "CONTEXT_PACK_SUBDIR")
}

fn check_context_pack(
    allowed: &[ContextPackGrant],
    payload: &serde_json::Value,
) -> Result<(), Violation> {
    if allowed.is_empty() {
        return Ok(());
    }
    let Some(url) = env_str(payload, "CONTEXT_PACK_URL") else {
        let stray = ["CONTEXT_PACK_REF", "CONTEXT_PACK_SUBDIR"]
            .into_iter()
            .find(|k| env_str(payload, k).is_some());
        return stray.map_or(Ok(()), |k| {
            Err(violation(Rule::ContextPack, format!("`{k}` needs a CONTEXT_PACK_URL")))
        });
    };
    if allowed.iter().any(|g| pack_matches(g, payload)) {
        return Ok(());
    }
    let names: Vec<&str> = allowed.iter().map(|g| g.name.as_str()).collect();
    Err(violation(
        Rule::ContextPack,
        format!("{url:?} matches no allowed pack; allowed: [{}]", names.join(", ")),
    ))
}

/// Refuse caller env that redirects egress or, under a model list, the model.
///
/// The server mints its own `*_BASE_URL` after this check.
pub fn check_env_keys(
    rules: &DispatchPolicyRules,
    payload: &serde_json::Value,
) -> Result<(), Violation> {
    let Some(env) = payload.get("env").and_then(serde_json::Value::as_object) else {
        return Ok(());
    };
    for key in env.keys() {
        let upper = key.to_ascii_uppercase();
        if upper.ends_with("_BASE_URL") || upper.starts_with("WORKER_NET_") {
            return Err(violation(Rule::Egress, format!("payload env `{key}` is reserved")));
        }
        if !rules.models.is_empty() && upper.ends_with("_MODEL") {
            return Err(violation(
                Rule::Model,
                format!("payload env `{key}` would override the model"),
            ));
        }
    }
    Ok(())
}

/// The request-shape rules: every listed dimension and the timeout ceiling.
/// A request without a timeout is given the ceiling rather than refused.
pub fn check_rules(
    rules: &DispatchPolicyRules,
    req: &mut DispatchRequest,
) -> Result<(), Violation> {
    check_listed(Rule::Dispatcher, &rules.dispatchers, Some(req.dispatcher.trim()))?;
    check_listed(Rule::Profile, &rules.profiles, payload_str(&req.payload, "profile"))?;
    let adapter = crate::dispatch::payload_adapter(&req.payload).unwrap_or("claude-code");
    check_listed(Rule::Adapter, &rules.adapters, Some(adapter))?;
    check_listed(Rule::Model, &rules.models, payload_str(&req.payload, "model"))?;
    if let Some(codex_model) = payload_str(&req.payload, "codex_model") {
        check_listed(Rule::Model, &rules.models, Some(codex_model))?;
    }
    check_env_keys(rules, &req.payload)?;
    check_context_pack(&rules.context_packs, &req.payload)?;
    if let Some(max) = rules.max_timeout_minutes {
        match req.timeout {
            Some(t) if t > max => {
                return Err(violation(
                    Rule::Timeout,
                    format!("timeout {t} exceeds the allowed {max} minutes"),
                ));
            }
            Some(_) => {}
            None => req.timeout = Some(max),
        }
    }
    Ok(())
}

fn forbidden(v: &Violation) -> (StatusCode, Json<ApiError>) {
    (StatusCode::FORBIDDEN, Json(ApiError { error: v.to_string() }))
}

fn lookup_failed(what: &str, e: &sqlx::Error) -> (StatusCode, Json<ApiError>) {
    tracing::error!(error = %e, "dispatch policy {what} lookup failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError { error: "could not evaluate dispatch policy".into() }),
    )
}

/// Load `user_id`'s policy and refuse the dispatch with a 403 naming the rule
/// it breaks. `Ok(None)` means the user has no policy and nothing was checked.
pub async fn enforce(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    req: &mut DispatchRequest,
) -> Result<Option<DispatchPolicy>, (StatusCode, Json<ApiError>)> {
    let Some(policy) = store::get(pool, user_id).await.map_err(|e| lookup_failed("policy", &e))?
    else {
        return Ok(None);
    };
    let dispatcher = req.dispatcher.clone();
    let refuse = |v: Violation| {
        tracing::warn!(%user_id, %dispatcher, "dispatch refused: {v}");
        forbidden(&v)
    };
    check_rules(&policy.rules, req).map_err(refuse)?;
    if let Some(max) = policy.rules.max_concurrent {
        let live = store::live_dispatches(pool, user_id)
            .await
            .map_err(|e| lookup_failed("live count", &e))?;
        if live >= i64::from(max) {
            return Err(refuse(violation(
                Rule::Concurrency,
                format!("{live} dispatched sessions are still live; the limit is {max}"),
            )));
        }
    }
    if let Some(cap) = policy.rules.daily_cap {
        let today = store::dispatches_today(pool, user_id)
            .await
            .map_err(|e| lookup_failed("daily count", &e))?;
        if today >= i64::from(cap) {
            return Err(refuse(violation(
                Rule::DailyCap,
                format!("{today} dispatches today; the daily cap is {cap}"),
            )));
        }
    }
    Ok(Some(policy))
}
