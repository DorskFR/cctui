//! `POST /api/v1/daemon/sessions/{id}/context` — the `CctuiContext` tool.
//!
//! The daemon relays the call with its machine key; the session's owner is the
//! user whose items are read and written. A rule, or anything a dispatched
//! worker saves, lands as a pending proposal and files a "needs you" card: a
//! rule rides into every future matching launch, so an agent that read a
//! hostile page must not be able to plant one unseen.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::context::{ContextToolEntry, ContextToolRequest, ContextToolResponse};
use cctui_proto::api::{AddUserActionRequest, UserActionKind};
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::context::{
    ContextItem, ContextItemSpec, KINDS, approval_title, attach_memories, clean_name, clean_spec,
    count_for_user, db_err, insert, list_for_user, propose, spec_of, stored_scope, update,
};
use crate::state::AppState;

pub const SAVES_PER_HOUR: i64 = 20;
pub const MAX_ITEMS_PER_USER: i64 = 200;

fn bad(msg: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, msg)
}

async fn calling_owner(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<Uuid, AppError> {
    let caller = crate::routes::spawn_child::machine_user(state, headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let owner: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    match owner {
        None => Err(AppError::new(StatusCode::NOT_FOUND, "calling session not found")),
        Some(Some(uid)) if uid == caller => Ok(uid),
        Some(_) => Err(AppError::new(StatusCode::FORBIDDEN, "session belongs to another user")),
    }
}

#[must_use]
pub fn entry(item: &ContextItem) -> ContextToolEntry {
    ContextToolEntry {
        kind: item.kind.clone(),
        name: item.name.clone(),
        title: item.title.clone(),
        match_: item.match_.clone(),
        enabled: item.enabled,
        version: item.version,
        pending: item.pending.is_some(),
        created_by_session: item.created_by_session.clone(),
        updated_by_session: item.updated_by_session.clone(),
    }
}

#[must_use]
pub fn append_paragraph(existing: &str, text: &str, date: &str) -> String {
    let para = format!("{date}: {}", text.trim());
    if existing.trim().is_empty() { para } else { format!("{}\n\n{para}", existing.trim_end()) }
}

/// The spec an edit produces: the pending proposal if there is one, else the
/// live spec, with the call's title, body and filter applied.
#[must_use]
pub fn edited_spec(
    existing: &ContextItem,
    mode: &str,
    req: &ContextToolRequest,
    date: &str,
) -> ContextItemSpec {
    let base = existing.pending.clone().unwrap_or_else(|| spec_of(existing));
    let text = req.body.clone().unwrap_or_default();
    let body = if mode == "append" { append_paragraph(&base.body, &text, date) } else { text };
    let title = req.title.clone().filter(|t| !t.trim().is_empty());
    let mut next =
        ContextItemSpec { title: title.unwrap_or_else(|| base.title.clone()), body, ..base };
    if let Some(m) = req.match_.clone() {
        next.match_ = m;
        "user".clone_into(&mut next.scope);
        next.scope_ref = None;
    }
    next
}

/// `POST /api/v1/daemon/sessions/{id}/context`.
pub async fn context_tool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<ContextToolRequest>,
) -> Result<Json<ContextToolResponse>, AppError> {
    let owner = calling_owner(&state, &headers, &session_id).await?;
    let out = match req.action.as_str() {
        "list" => list(&state, owner, &req).await?,
        "show" => show(&state, owner, &req).await?,
        "save" => save(&state, owner, &session_id, &req).await?,
        "attach" => {
            if req.names.iter().all(|n| n.trim().is_empty()) {
                return Err(bad("names is required: the memories to attach"));
            }
            let (path, done) = attach_memories(&state, &session_id, owner, &req.names).await?;
            ContextToolResponse {
                path,
                attached: done.attached,
                unknown: done.unknown,
                ..ContextToolResponse::default()
            }
        }
        _ => return Err(bad("action must be list, show, save or attach")),
    };
    Ok(Json(out))
}

fn known_kind(kind: &str) -> Result<(), AppError> {
    if KINDS.contains(&kind) { Ok(()) } else { Err(bad("kind must be memory, prompt or rule")) }
}

async fn list(
    state: &AppState,
    owner: Uuid,
    req: &ContextToolRequest,
) -> Result<ContextToolResponse, AppError> {
    let kind = req.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
    if let Some(kind) = kind {
        known_kind(kind)?;
    }
    let items = list_for_user(&state.pool, owner).await?;
    Ok(ContextToolResponse {
        items: items.iter().filter(|i| kind.is_none_or(|k| i.kind == k)).map(entry).collect(),
        ..ContextToolResponse::default()
    })
}

async fn show(
    state: &AppState,
    owner: Uuid,
    req: &ContextToolRequest,
) -> Result<ContextToolResponse, AppError> {
    let kind = req.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()).unwrap_or("memory");
    known_kind(kind)?;
    let name = clean_name(req.name.as_deref().unwrap_or_default())?;
    let items = list_for_user(&state.pool, owner).await?;
    let item = items
        .iter()
        .find(|i| i.kind == kind && i.name == name)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, format!("no {kind} named `{name}`")))?;
    Ok(ContextToolResponse {
        item: Some(entry(item)),
        body: Some(item.body.clone()),
        ..ContextToolResponse::default()
    })
}

async fn saves_this_hour(pool: &sqlx::PgPool, session_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM context_tool_saves \
         WHERE session_id = $1 AND created_at > now() - interval '1 hour'",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
}

async fn record_save(pool: &sqlx::PgPool, session_id: &str) {
    let done = sqlx::query(
        "WITH gone AS (DELETE FROM context_tool_saves \
                        WHERE session_id = $1 AND created_at < now() - interval '1 day') \
         INSERT INTO context_tool_saves (session_id) VALUES ($1)",
    )
    .bind(session_id)
    .execute(pool)
    .await;
    if let Err(e) = done {
        tracing::warn!(%session_id, error = %e, "could not record a context save");
    }
}

async fn save(
    state: &AppState,
    owner: Uuid,
    session_id: &str,
    req: &ContextToolRequest,
) -> Result<ContextToolResponse, AppError> {
    let kind = req.kind.as_deref().map(str::trim).unwrap_or_default();
    if kind != "memory" && kind != "rule" {
        return Err(bad("kind must be memory or rule"));
    }
    let name = clean_name(req.name.as_deref().unwrap_or_default())?;
    let mode = req.mode.as_deref().map(str::trim).filter(|m| !m.is_empty()).unwrap_or("create");
    if !["create", "replace", "append"].contains(&mode) {
        return Err(bad("mode must be create, replace or append"));
    }
    if saves_this_hour(&state.pool, session_id).await? >= SAVES_PER_HOUR {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "rate limited: a session may save at most {SAVES_PER_HOUR} context items per \
                 hour; try again later"
            ),
        ));
    }
    let dispatched = stored_scope(&state.pool, session_id).await.is_some_and(|s| s.dispatched);
    let needs_approval = kind == "rule" || dispatched;
    let items = list_for_user(&state.pool, owner).await?;
    let existing = items.iter().find(|i| i.kind == kind && i.name == name);
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let item = match (mode, existing) {
        ("create", Some(_)) => {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                format!("a {kind} named `{name}` already exists; save with mode replace or append"),
            ));
        }
        ("create", None) => {
            if count_for_user(&state.pool, owner).await? >= MAX_ITEMS_PER_USER {
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    format!("a user may hold at most {MAX_ITEMS_PER_USER} context items"),
                ));
            }
            let spec = clean_spec(ContextItemSpec {
                kind: kind.to_owned(),
                name: name.clone(),
                title: req
                    .title
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| name.clone()),
                body: req.body.clone().unwrap_or_default(),
                match_: req.match_.clone().unwrap_or_default(),
                enabled: true,
                ..ContextItemSpec::default()
            })?;
            let created = if needs_approval {
                let live = ContextItemSpec { body: String::new(), enabled: false, ..spec.clone() };
                insert(&state.pool, owner, &live, Some(&spec), Some(session_id)).await
            } else {
                insert(&state.pool, owner, &spec, None, Some(session_id)).await
            };
            created.map_err(db_err)?
        }
        (_, None) => {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                format!("no {kind} named `{name}`; save with mode create"),
            ));
        }
        (mode, Some(existing)) => {
            let next = clean_spec(edited_spec(existing, mode, req, &date))?;
            let saved = if needs_approval {
                propose(&state.pool, owner, existing.id, &next, session_id).await
            } else {
                update(&state.pool, owner, existing.id, &next, Some(session_id)).await
            };
            saved
                .map_err(db_err)?
                .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "context item not found"))?
        }
    };
    record_save(&state.pool, session_id).await;
    if needs_approval {
        file_approval_card(state, session_id, &item).await;
    }
    Ok(ContextToolResponse {
        item: Some(entry(&item)),
        pending: needs_approval,
        ..ContextToolResponse::default()
    })
}

async fn file_approval_card(state: &AppState, session_id: &str, item: &ContextItem) {
    let req = AddUserActionRequest {
        title: approval_title(&item.kind, &item.name),
        detail: Some(format!(
            "An agent in this session proposed a {kind} for your cctui context. Review the diff \
             in Settings → Context and approve or reject it; it does not apply until you \
             approve it.",
            kind = item.kind
        )),
        kind: UserActionKind::Decision,
        blocking: false,
    };
    match crate::routes::user_actions::add(state, session_id, &req).await {
        Ok(result) => {
            if let Some(err) = result.error {
                tracing::warn!(%session_id, %err, "approval card not filed");
            }
        }
        Err(e) => tracing::warn!(%session_id, error = ?e, "approval card not filed"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::routes::context::ContextMatch;

    fn memory(body: &str) -> ContextItem {
        ContextItem {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            kind: "memory".into(),
            name: "notes".into(),
            title: "Notes".into(),
            body: body.into(),
            scope: "path".into(),
            scope_ref: Some("/w".into()),
            match_: ContextMatch { paths: vec!["/w".into()], ..ContextMatch::default() },
            tags: Vec::new(),
            enabled: true,
            version: 3,
            pending: None,
            created_by_session: Some("s-0".into()),
            updated_by_session: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn call(mode: &str, body: &str) -> ContextToolRequest {
        ContextToolRequest {
            action: "save".into(),
            kind: Some("memory".into()),
            name: Some("notes".into()),
            body: Some(body.into()),
            mode: Some(mode.into()),
            ..ContextToolRequest::default()
        }
    }

    #[test]
    fn append_adds_a_dated_paragraph_and_never_overwrites() {
        let next = edited_spec(&memory("first fact\n"), "append", &call("append", "second"), "D");
        assert_eq!(next.body, "first fact\n\nD: second");
        assert_eq!(next.title, "Notes", "no title in the call keeps the item's");
        assert_eq!(next.match_.paths, ["/w"], "no match in the call keeps the item's");
        assert_eq!(append_paragraph("", " only ", "D"), "D: only");
    }

    #[test]
    fn replace_swaps_the_body_and_a_given_filter_replaces_the_scope() {
        let mut req = call("replace", "new body");
        req.match_ = Some(ContextMatch::default());
        req.title = Some("Renamed".into());
        let next = edited_spec(&memory("old"), "replace", &req, "D");
        assert_eq!(next.body, "new body");
        assert_eq!(next.title, "Renamed");
        assert!(next.match_.is_empty());
        assert_eq!((next.scope.as_str(), next.scope_ref), ("user", None));
    }

    /// Successive edits to an unapproved rule stack on the proposal, not on
    /// the live body the owner has not seen change.
    #[test]
    fn an_edit_builds_on_the_pending_proposal_when_there_is_one() {
        let mut item = memory("live");
        item.pending = Some(ContextItemSpec { body: "proposed".into(), ..spec_of(&item) });
        let next = edited_spec(&item, "append", &call("append", "more"), "D");
        assert_eq!(next.body, "proposed\n\nD: more");
    }

    #[test]
    fn a_list_entry_carries_provenance_and_the_pending_flag_but_no_body() {
        let mut item = memory("secret-ish body");
        item.pending = Some(spec_of(&item));
        let e = entry(&item);
        assert!(e.pending);
        assert_eq!(e.created_by_session.as_deref(), Some("s-0"));
        assert_eq!(e.version, 3);
        let wire = serde_json::to_string(&e).expect("encodes");
        assert!(!wire.contains("secret-ish"), "{wire}");
    }

    /// The tool reuses the human API's validation, so an agent sees the same
    /// message a person would.
    #[test]
    fn an_oversize_rule_fails_with_the_human_apis_message() {
        let mut item = memory("");
        item.kind = "rule".into();
        let next = edited_spec(&item, "replace", &call("replace", &"x".repeat(5000)), "D");
        let err = clean_spec(next).expect_err("over the rule cap");
        assert!(format!("{err:?}").contains("4KiB"), "{err:?}");
    }

    #[tokio::test]
    async fn saves_are_counted_per_session_within_the_hour() {
        let Some(url) = crate::routes::gateway::test_db_url("context_tool_saves") else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (me, other) =
            (format!("ctx-rate-{}", Uuid::new_v4()), format!("ctx-rate-{}", Uuid::new_v4()));
        for _ in 0..3 {
            record_save(&pool, &me).await;
        }
        record_save(&pool, &other).await;
        sqlx::query(
            "INSERT INTO context_tool_saves (session_id, created_at) \
             VALUES ($1, now() - interval '2 hours')",
        )
        .bind(&me)
        .execute(&pool)
        .await
        .expect("seed an old save");

        assert_eq!(saves_this_hour(&pool, &me).await.expect("count"), 3, "the old save aged out");
        assert_eq!(saves_this_hour(&pool, &other).await.expect("count"), 1);

        sqlx::query("DELETE FROM context_tool_saves WHERE session_id = ANY($1)")
            .bind(vec![me, other])
            .execute(&pool)
            .await
            .ok();
    }
}
