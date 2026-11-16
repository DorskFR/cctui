use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

pub use cctui_proto::api::notifications::{
    NotificationActRequest, NotificationList, NotificationReadAll, NotificationTransition,
};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::notify::store::{self, Edge, NotificationRecord, Page};
use crate::notify::tokens::{self, TokenRow};
use crate::routes::device_auth::{PeerAddr, caller_key};
use crate::state::AppState;

type ApiResult<T> = Result<T, AppError>;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_NOTE_CHARS: usize = 1000;
pub const TOKEN_ACTS_PER_MIN: usize = 30;
const BEARER_CHANNELS: &[&str] = &["web", "tui"];

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub state: Option<String>,
    pub limit: Option<i64>,
    pub before: Option<DateTime<Utc>>,
}

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no notification with that id")
}

fn bad_request(msg: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, msg)
}

async fn owned(state: &AppState, ctx: &AuthContext, id: Uuid) -> ApiResult<NotificationRecord> {
    store::owned(&state.pool, ctx.user_id, id).await?.ok_or_else(not_found)
}

async fn apply(
    state: &AppState,
    id: Uuid,
    edge: Edge<'_>,
) -> ApiResult<Json<NotificationTransition>> {
    store::transition(&state.pool, id, edge).await?.map(Json).ok_or_else(not_found)
}

pub fn page(query: &ListQuery) -> Result<Page, String> {
    let open_only = match query.state.as_deref().unwrap_or("open") {
        "open" => true,
        "all" => false,
        other => return Err(format!("unknown state '{other}': open or all")),
    };
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    Ok(Page { open_only, limit, before: query.before })
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<NotificationList>> {
    let page = page(&query).map_err(bad_request)?;
    let mut notifications = store::list(&state.pool, ctx.user_id, page).await?;
    let has_more = notifications.len() > usize::try_from(page.limit).unwrap_or(usize::MAX);
    notifications.truncate(usize::try_from(page.limit).unwrap_or(usize::MAX));
    let unread = store::unread_count(&state.pool, ctx.user_id).await?;
    Ok(Json(NotificationList { notifications, unread, has_more }))
}

pub async fn read(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<NotificationTransition>> {
    owned(&state, &ctx, id).await?;
    apply(&state, id, Edge::Read).await
}

pub async fn read_all(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<NotificationReadAll>> {
    let read = store::read_all(&state.pool, ctx.user_id).await?;
    let unread = store::unread_count(&state.pool, ctx.user_id).await?;
    Ok(Json(NotificationReadAll { read: i64::try_from(read.len()).unwrap_or(i64::MAX), unread }))
}

pub fn validate_act<'a>(
    n: &NotificationRecord,
    req: &'a NotificationActRequest,
) -> Result<(&'a str, Option<&'a str>, &'a str), String> {
    let action = req.action.trim();
    if store::action(n, action).is_none() {
        let offered = store::action_ids(n);
        if offered.is_empty() {
            return Err("this notification offers no actions".into());
        }
        return Err(format!("unknown action '{action}': one of {}", offered.join(", ")));
    }
    let note = req.note.as_deref().map(str::trim).filter(|note| !note.is_empty());
    if note.is_some_and(|note| note.chars().count() > MAX_NOTE_CHARS) {
        return Err(format!("note must be at most {MAX_NOTE_CHARS} characters"));
    }
    let channel = req.channel.as_deref().unwrap_or("web");
    if !BEARER_CHANNELS.contains(&channel) {
        return Err(format!("unknown channel '{channel}': web or tui"));
    }
    Ok((action, note, channel))
}

pub async fn act(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<NotificationActRequest>,
) -> ApiResult<Json<NotificationTransition>> {
    let n = owned(&state, &ctx, id).await?;
    let (action, note, channel) = validate_act(&n, &req).map_err(bad_request)?;
    apply(&state, id, Edge::Act { action, note, channel, by: Some(ctx.user_id) }).await
}

pub async fn dismiss(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<NotificationTransition>> {
    let n = owned(&state, &ctx, id).await?;
    if store::is_open(&n) && store::is_blocking(&state.pool, &n).await? {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "a blocking approval cannot be dismissed: answer it instead",
        ));
    }
    apply(&state, id, Edge::Dismiss).await
}

fn throttle(caller: &str) -> ApiResult<()> {
    let key = format!("notification-token:{caller}");
    if crate::routes::peer::limiter().admit(&key, TOKEN_ACTS_PER_MIN, std::time::Instant::now()) {
        return Ok(());
    }
    Err(AppError::new(StatusCode::TOO_MANY_REQUESTS, "too many notification actions, slow down"))
}

async fn token_transition(
    state: &AppState,
    hash: &[u8],
) -> ApiResult<Option<NotificationTransition>> {
    if let Some(t) = tokens::burn(&state.pool, hash).await? {
        let edge = Edge::Act { action: &t.action, note: None, channel: &t.channel, by: None };
        return Ok(store::transition(&state.pool, t.notification_id, edge).await?);
    }
    let Some(t) = tokens::peek(&state.pool, hash).await? else { return Ok(None) };
    let current = store::get(&state.pool, t.notification_id).await?;
    Ok(current.map(|notification| NotificationTransition { notification, applied: false }))
}

/// A burnt or expired token answers with the notice as it stands, not an error.
pub async fn act_with_token(
    State(state): State<AppState>,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    throttle(&caller_key(&headers, peer, state.config.trusted_proxy_hops))?;
    let html = wants_html(&headers);
    let Some(hash) = tokens::hash(&token) else { return unknown_link(html) };
    let Some(outcome) = token_transition(&state, &hash).await? else { return unknown_link(html) };
    if html {
        return Ok(page_response(StatusCode::OK, &result_page(&outcome)));
    }
    Ok(Json(outcome).into_response())
}

fn unknown_link(html: bool) -> ApiResult<Response> {
    if html {
        return Ok(page_response(StatusCode::NOT_FOUND, &unknown_page()));
    }
    Err(AppError::new(StatusCode::NOT_FOUND, "unknown action link"))
}

/// Never mutates: link previews and mail scanners fetch it.
pub async fn confirm_page(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    match confirm_view(&state, &token).await {
        Ok(Some((t, n))) => page_response(StatusCode::OK, &confirm_body(&token, &t, &n)),
        Ok(None) => page_response(StatusCode::NOT_FOUND, &unknown_page()),
        Err(e) => e.into_response(),
    }
}

async fn confirm_view(
    state: &AppState,
    token: &str,
) -> ApiResult<Option<(TokenRow, NotificationRecord)>> {
    let Some(hash) = tokens::hash(token) else { return Ok(None) };
    let Some(t) = tokens::peek(&state.pool, &hash).await? else { return Ok(None) };
    Ok(store::get(&state.pool, t.notification_id).await?.map(|n| (t, n)))
}

fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

#[must_use]
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn page_response(status: StatusCode, body: &str) -> Response {
    let mut response = (status, Html(body.to_owned())).into_response();
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex, nofollow"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; \
             frame-ancestors 'none'",
        ),
    );
    response
}

fn shell(title: &str, inner: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>body{{font-family:system-ui,sans-serif;max-width:32rem;\
         margin:3rem auto;padding:0 1rem;line-height:1.5}}button{{font-size:1.1rem;\
         padding:.6rem 1.4rem}}.muted{{color:#666}}</style></head><body>{inner}</body></html>",
        escape(title)
    )
}

fn unknown_page() -> String {
    shell("Unknown link", "<h1>Unknown link</h1><p>This action link is not valid.</p>")
}

fn action_label(n: &NotificationRecord, action: &str) -> String {
    store::action(n, action)
        .and_then(|a| a.get("label"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(action)
        .to_owned()
}

#[must_use]
pub fn status_line(n: &NotificationRecord) -> String {
    let via = n.acted_channel.as_deref().map(|c| format!(" from {c}")).unwrap_or_default();
    match n.state.as_str() {
        "acted" => format!("Already answered{via}: {}.", n.decision.as_deref().unwrap_or("?")),
        "resolved" => {
            format!("Resolved: {}.", n.resolved_reason.as_deref().unwrap_or("closed"))
        }
        "expired" => "This notification expired.".to_owned(),
        _ => "This link was already used or has expired.".to_owned(),
    }
}

fn card(n: &NotificationRecord) -> String {
    let mut out = format!("<h1>{}</h1>", escape(&n.title));
    if let Some(body) = &n.body {
        let _ = write!(out, "<p>{}</p>", escape(body));
    }
    out
}

fn confirm_body(token: &str, t: &TokenRow, n: &NotificationRecord) -> String {
    let mut inner = card(n);
    if t.usable && store::is_open(n) {
        let _ = write!(
            inner,
            "<form method=\"post\" action=\"/api/v1/notifications/actions/{}\">\
             <button type=\"submit\">{}</button></form>",
            escape(token),
            escape(&action_label(n, &t.action))
        );
    } else {
        let _ = write!(inner, "<p class=\"muted\">{}</p>", escape(&status_line(n)));
    }
    shell(&n.title, &inner)
}

fn result_page(outcome: &NotificationTransition) -> String {
    let n = &outcome.notification;
    let line = if outcome.applied {
        format!("Done: {}.", action_label(n, n.decision.as_deref().unwrap_or("")))
    } else {
        status_line(n)
    };
    let inner = format!("{}<p class=\"muted\">{}</p>", card(n), escape(&line));
    shell(&n.title, &inner)
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
