//! The context browser: the caller's rules, memories and prompts, and what
//! the selected session was given.

use std::collections::HashSet;

use cctui_proto::api::context::{
    AttachContextRequest, AttachContextResponse, ContextItem, SessionContextResponse,
};

use super::action::{Action, Effect};
use super::state::{App, View};
use super::toast::Level;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Catalogue,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: i32,
    /// The catalogue version, when it is past the one the session received.
    pub newer: Option<i32>,
    pub pending: bool,
    pub enabled: bool,
    /// Catalogue tab: the session this browser opened from already has it.
    pub attached: bool,
}

impl Row {
    pub fn version_cell(&self) -> String {
        match self.newer {
            Some(newer) => format!("v{} (v{newer} available)", self.version),
            None => format!("v{}", self.version),
        }
    }

    /// The markers after the title, most urgent first.
    pub fn flags(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.pending {
            out.push("pending");
        }
        if !self.enabled {
            out.push("disabled");
        }
        if self.attached {
            out.push("attached");
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct Browser {
    /// The session the browser opened from; `None` from a list with no row.
    pub session_id: Option<String>,
    pub tab: Tab,
    pub items: Vec<ContextItem>,
    pub session: Option<SessionContextResponse>,
    pub rows: Vec<Row>,
    pub focused: usize,
    pub loading: bool,
    pub busy: bool,
    pub error: Option<String>,
    /// The item `d` was pressed on once; a second `d` on it deletes.
    pub armed_delete: Option<String>,
}

impl Browser {
    #[allow(clippy::missing_const_for_fn, reason = "Vec::new is not const here")]
    fn new(session_id: Option<String>) -> Self {
        let tab = if session_id.is_some() { Tab::Session } else { Tab::Catalogue };
        Self {
            session_id,
            tab,
            items: Vec::new(),
            session: None,
            rows: Vec::new(),
            focused: 0,
            loading: true,
            busy: false,
            error: None,
            armed_delete: None,
        }
    }

    #[allow(clippy::missing_const_for_fn, reason = "Vec deref is not const")]
    pub fn focused_row(&self) -> Option<&Row> {
        self.rows.get(self.focused)
    }

    fn rebuild(&mut self) {
        let focused_id = self.focused_row().map(|r| r.id.clone());
        self.rows = match self.tab {
            Tab::Catalogue => catalogue_rows(&self.items, self.session.as_ref()),
            Tab::Session => session_rows(self.session.as_ref(), &self.items),
        };
        self.focused = focused_id
            .and_then(|id| self.rows.iter().position(|r| r.id == id))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.armed_delete = None;
    }
}

/// Every item, rules first, then memories, then prompts, by name within a kind.
pub fn catalogue_rows(items: &[ContextItem], session: Option<&SessionContextResponse>) -> Vec<Row> {
    let attached: HashSet<&str> = session
        .map(|s| s.rules.iter().chain(&s.memories).map(|r| r.id.as_str()).collect())
        .unwrap_or_default();
    let mut rows: Vec<Row> = items
        .iter()
        .map(|item| Row {
            attached: attached.contains(item.id.to_string().as_str()),
            id: item.id.to_string(),
            kind: item.kind.clone(),
            name: item.name.clone(),
            title: item.title.clone(),
            version: item.version,
            newer: None,
            pending: item.pending.is_some(),
            enabled: item.enabled,
        })
        .collect();
    rows.sort_by(|a, b| kind_rank(&a.kind).cmp(&kind_rank(&b.kind)).then(a.name.cmp(&b.name)));
    rows
}

/// What the session received, rules then memories, each at the version it got.
pub fn session_rows(session: Option<&SessionContextResponse>, items: &[ContextItem]) -> Vec<Row> {
    let Some(session) = session else { return Vec::new() };
    session
        .rules
        .iter()
        .chain(&session.memories)
        .map(|r| {
            let item = items.iter().find(|i| i.id.to_string() == r.id);
            Row {
                id: r.id.clone(),
                kind: r.kind.clone(),
                name: r.name.clone(),
                title: r.title.clone(),
                version: r.version,
                newer: item.map(|i| i.version).filter(|v| *v > r.version),
                pending: item.is_some_and(|i| i.pending.is_some()),
                enabled: item.is_none_or(|i| i.enabled),
                attached: true,
            }
        })
        .collect()
}

const fn kind_rank(kind: &str) -> u8 {
    match kind {
        "rule" => 0,
        "memory" => 1,
        "prompt" => 2,
        _ => 3,
    }
}

pub async fn load(
    server: &cctui_client::Client,
    session_id: Option<&str>,
) -> Result<(Vec<ContextItem>, Option<SessionContextResponse>), cctui_client::ClientError> {
    let items: Vec<ContextItem> = server.get_as("get_context", &[]).await?;
    let session = match session_id {
        Some(id) => Some(server.get_as("get_sessions_by_id_context", &[("id", id)]).await?),
        None => None,
    };
    Ok((items, session))
}

/// The writes the browser makes, each a toast on success.
pub async fn run(server: &cctui_client::Client, effect: ContextEffect) -> Vec<Action> {
    let done = match effect {
        ContextEffect::Fetch { session_id } => {
            return match load(server, session_id.as_deref()).await {
                Ok((items, session)) => {
                    vec![Action::ContextItems(ContextAction::Loaded { items, session })]
                }
                Err(e) => {
                    tracing::warn!(%e, "loading the context browser failed");
                    vec![Action::ContextItems(ContextAction::Failed(e.to_string()))]
                }
            };
        }
        ContextEffect::Approve { id, name } => server
            .post("post_context_by_id_pending_approve", &[("id", id.as_str())], None)
            .await
            .map(|_| format!("{name}: proposal approved")),
        ContextEffect::Reject { id, name } => server
            .post("post_context_by_id_pending_reject", &[("id", id.as_str())], None)
            .await
            .map(|_| format!("{name}: proposal rejected")),
        ContextEffect::Delete { id, name } => server
            .delete("delete_context_by_id", &[("id", id.as_str())])
            .await
            .map(|_| format!("{name} deleted")),
        ContextEffect::Attach { session_id, names } => server
            .post_as::<_, AttachContextResponse>(
                "post_sessions_by_id_context",
                &[("id", session_id.as_str())],
                &AttachContextRequest { names },
            )
            .await
            .map(|out| attach_summary(&out)),
    };
    match done {
        Ok(message) => vec![Action::ContextItems(ContextAction::Done(message))],
        Err(e) => {
            tracing::warn!(%e, "context browser write failed");
            vec![Action::ContextItems(ContextAction::WriteFailed(e.to_string()))]
        }
    }
}

pub fn attach_summary(out: &AttachContextResponse) -> String {
    let mut text = if out.attached.is_empty() {
        "nothing attached".to_owned()
    } else {
        format!("attached {}", out.attached.join(", "))
    };
    if !out.unknown.is_empty() {
        text.push_str(&format!(" (unknown: {})", out.unknown.join(", ")));
    }
    text
}

pub enum ContextEffect {
    /// `GET /context`, plus `GET /sessions/{id}/context` when opened from a session.
    Fetch {
        session_id: Option<String>,
    },
    Approve {
        id: String,
        name: String,
    },
    Reject {
        id: String,
        name: String,
    },
    Delete {
        id: String,
        name: String,
    },
    Attach {
        session_id: String,
        names: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum ContextAction {
    Open,
    Close,
    SelectNext,
    SelectPrev,
    ToggleTab,
    Refresh,
    Loaded {
        items: Vec<ContextItem>,
        session: Option<SessionContextResponse>,
    },
    Failed(String),
    Approve,
    Reject,
    /// Arms on the first press, fires on the second press on the same item.
    Delete,
    /// Attach the focused memory to the session the browser opened from.
    Attach,
    Done(String),
    WriteFailed(String),
}

pub fn reduce_context_items(app: &mut App, action: ContextAction) -> Vec<Effect> {
    match action {
        ContextAction::Open => {
            let session_id = app.selected_session_id();
            app.context_items = Some(Browser::new(session_id.clone()));
            app.router.push(View::ContextItems);
            vec![Effect::Context(ContextEffect::Fetch { session_id })]
        }
        ContextAction::Close => {
            close(app);
            Vec::new()
        }
        ContextAction::SelectNext => {
            if let Some(b) = app.context_items.as_mut()
                && !b.rows.is_empty()
            {
                b.focused = (b.focused + 1) % b.rows.len();
                b.armed_delete = None;
            }
            Vec::new()
        }
        ContextAction::SelectPrev => {
            if let Some(b) = app.context_items.as_mut()
                && !b.rows.is_empty()
            {
                b.focused = b.focused.checked_sub(1).unwrap_or(b.rows.len() - 1);
                b.armed_delete = None;
            }
            Vec::new()
        }
        ContextAction::ToggleTab => {
            if let Some(b) = app.context_items.as_mut() {
                if b.session_id.is_none() {
                    b.error = Some("open the browser from a session to see its context".to_owned());
                    return Vec::new();
                }
                b.tab = match b.tab {
                    Tab::Catalogue => Tab::Session,
                    Tab::Session => Tab::Catalogue,
                };
                b.focused = 0;
                b.error = None;
                b.rebuild();
            }
            Vec::new()
        }
        ContextAction::Refresh => refresh(app),
        ContextAction::Loaded { items, session } => {
            if let Some(b) = app.context_items.as_mut() {
                b.items = items;
                b.session = session;
                b.loading = false;
                b.busy = false;
                b.error = None;
                b.rebuild();
            }
            Vec::new()
        }
        ContextAction::Failed(message) => {
            if let Some(b) = app.context_items.as_mut() {
                b.loading = false;
                b.busy = false;
                b.error = Some(message);
            }
            Vec::new()
        }
        ContextAction::Approve => settle(app, true),
        ContextAction::Reject => settle(app, false),
        ContextAction::Delete => delete(app),
        ContextAction::Attach => attach(app),
        ContextAction::Done(message) => {
            app.toast(Level::Info, message);
            refresh(app)
        }
        ContextAction::WriteFailed(message) => {
            if let Some(b) = app.context_items.as_mut() {
                b.busy = false;
                b.error = Some(message);
            }
            Vec::new()
        }
    }
}

fn close(app: &mut App) {
    app.context_items = None;
    if app.view() == View::ContextItems {
        app.router.pop();
    }
}

fn refresh(app: &mut App) -> Vec<Effect> {
    let Some(b) = app.context_items.as_mut() else { return Vec::new() };
    b.loading = true;
    b.armed_delete = None;
    vec![Effect::Context(ContextEffect::Fetch { session_id: b.session_id.clone() })]
}

/// The focused row, when the browser is idle enough to act on it.
fn target(b: &mut Browser) -> Option<Row> {
    if b.busy || b.loading {
        return None;
    }
    let row = b.focused_row().cloned();
    if row.is_none() {
        b.error = Some("nothing selected".to_owned());
    }
    row
}

fn settle(app: &mut App, approve: bool) -> Vec<Effect> {
    let Some(b) = app.context_items.as_mut() else { return Vec::new() };
    let Some(row) = target(b) else { return Vec::new() };
    if !row.pending {
        b.error = Some(format!("{} has no pending proposal", row.name));
        return Vec::new();
    }
    b.busy = true;
    b.error = None;
    let (id, name) = (row.id, row.name);
    vec![Effect::Context(if approve {
        ContextEffect::Approve { id, name }
    } else {
        ContextEffect::Reject { id, name }
    })]
}

fn delete(app: &mut App) -> Vec<Effect> {
    let Some(b) = app.context_items.as_mut() else { return Vec::new() };
    let Some(row) = target(b) else { return Vec::new() };
    if b.armed_delete.as_deref() != Some(row.id.as_str()) {
        b.armed_delete = Some(row.id);
        b.error = None;
        return Vec::new();
    }
    b.armed_delete = None;
    b.busy = true;
    vec![Effect::Context(ContextEffect::Delete { id: row.id, name: row.name })]
}

fn attach(app: &mut App) -> Vec<Effect> {
    let Some(b) = app.context_items.as_mut() else { return Vec::new() };
    let Some(row) = target(b) else { return Vec::new() };
    let Some(session_id) = b.session_id.clone() else {
        b.error = Some("open the browser from a session to attach to it".to_owned());
        return Vec::new();
    };
    if row.kind != "memory" {
        b.error = Some("only memories can be attached to a running session".to_owned());
        return Vec::new();
    }
    b.busy = true;
    b.error = None;
    vec![Effect::Context(ContextEffect::Attach { session_id, names: vec![row.name] })]
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::context::{
        AttachContextResponse, AttachedContextRef, ContextItem, ContextItemSpec, ContextMatch,
        SessionContextResponse,
    };

    use super::{Browser, ContextAction, ContextEffect, Tab, attach_summary};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::app_with_sessions;

    fn item(id: u8, kind: &str, name: &str, version: i32) -> ContextItem {
        let now = chrono::Utc::now();
        ContextItem {
            id: uuid::Uuid::from_u128(u128::from(id)),
            user_id: uuid::Uuid::nil(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            title: name.to_uppercase(),
            body: String::new(),
            scope: "user".to_owned(),
            scope_ref: None,
            match_: ContextMatch::default(),
            tags: Vec::new(),
            enabled: true,
            version,
            pending: None,
            created_by_session: None,
            updated_by_session: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn with_pending(mut item: ContextItem) -> ContextItem {
        item.pending = Some(ContextItemSpec {
            kind: item.kind.clone(),
            name: item.name.clone(),
            title: "proposed".to_owned(),
            ..ContextItemSpec::default()
        });
        item
    }

    fn attached(item: &ContextItem, version: i32) -> AttachedContextRef {
        AttachedContextRef {
            id: item.id.to_string(),
            kind: item.kind.clone(),
            name: item.name.clone(),
            title: item.title.clone(),
            version,
        }
    }

    fn items() -> Vec<ContextItem> {
        let mut prompt = item(3, "prompt", "review", 1);
        prompt.enabled = false;
        vec![
            item(1, "memory", "zebra", 3),
            with_pending(item(2, "rule", "style", 1)),
            prompt,
            item(4, "memory", "alpha", 1),
        ]
    }

    fn session() -> SessionContextResponse {
        let all = items();
        SessionContextResponse {
            rules: vec![attached(&all[1], 1)],
            memories: vec![attached(&all[0], 2)],
        }
    }

    fn dispatch(app: &mut App, action: ContextAction) -> Vec<Effect> {
        reduce(app, Action::ContextItems(action))
    }

    fn open(app: &mut App) -> Vec<Effect> {
        let effects = dispatch(app, ContextAction::Open);
        dispatch(app, ContextAction::Loaded { items: items(), session: Some(session()) });
        effects
    }

    fn browser(app: &App) -> &Browser {
        app.context_items.as_ref().expect("browser open")
    }

    fn names(app: &App) -> Vec<&str> {
        browser(app).rows.iter().map(|r| r.name.as_str()).collect()
    }

    fn focus(app: &mut App, name: &str) {
        while browser(app).focused_row().expect("a row").name != name {
            dispatch(app, ContextAction::SelectNext);
        }
    }

    #[test]
    fn opening_from_a_session_fetches_both_lists_and_lands_on_the_session_tab() {
        let mut app = app_with_sessions();
        let effects = dispatch(&mut app, ContextAction::Open);
        assert_eq!(app.view(), View::ContextItems);
        assert!(browser(&app).loading);
        assert_eq!(browser(&app).tab, Tab::Session);
        match effects.as_slice() {
            [Effect::Context(ContextEffect::Fetch { session_id })] => {
                assert!(session_id.is_some());
                assert_eq!(*session_id, app.selected_session_id());
            }
            _ => panic!("expected one fetch effect"),
        }
    }

    #[test]
    fn opening_without_a_session_lists_the_catalogue_only() {
        let mut app = App::new();
        let effects = dispatch(&mut app, ContextAction::Open);
        assert_eq!(browser(&app).tab, Tab::Catalogue);
        match effects.as_slice() {
            [Effect::Context(ContextEffect::Fetch { session_id })] => assert!(session_id.is_none()),
            _ => panic!("expected one fetch effect"),
        }
        dispatch(&mut app, ContextAction::ToggleTab);
        assert_eq!(browser(&app).tab, Tab::Catalogue);
        assert!(browser(&app).error.is_some());
    }

    #[test]
    fn the_session_tab_shows_delivered_versions_and_flags_newer_ones() {
        let mut app = app_with_sessions();
        open(&mut app);
        assert_eq!(names(&app), vec!["style", "zebra"]);
        let rows = &browser(&app).rows;
        assert_eq!(rows[0].version_cell(), "v1");
        assert!(rows[0].pending);
        assert_eq!(rows[1].version_cell(), "v2 (v3 available)");
        assert_eq!(rows[1].newer, Some(3));
        assert!(rows.iter().all(|r| r.attached));
    }

    #[test]
    fn the_catalogue_sorts_by_kind_then_name_and_marks_pending_disabled_attached() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, ContextAction::ToggleTab);
        assert_eq!(browser(&app).tab, Tab::Catalogue);
        assert_eq!(names(&app), vec!["style", "alpha", "zebra", "review"]);
        let rows = &browser(&app).rows;
        assert_eq!(rows[0].flags(), vec!["pending", "attached"]);
        assert_eq!(rows[1].flags(), Vec::<&str>::new());
        assert_eq!(rows[2].flags(), vec!["attached"]);
        assert_eq!(rows[3].flags(), vec!["disabled"]);
        assert!(rows.iter().all(|r| r.newer.is_none()));
    }

    #[test]
    fn the_focus_wraps_both_ways() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, ContextAction::SelectPrev);
        assert_eq!(browser(&app).focused, 1);
        dispatch(&mut app, ContextAction::SelectNext);
        assert_eq!(browser(&app).focused, 0);
    }

    #[test]
    fn approving_posts_only_on_an_item_with_a_proposal() {
        let mut app = app_with_sessions();
        open(&mut app);
        let effects = dispatch(&mut app, ContextAction::Approve);
        match effects.as_slice() {
            [Effect::Context(ContextEffect::Approve { id, name })] => {
                assert_eq!(id, &uuid::Uuid::from_u128(2).to_string());
                assert_eq!(name, "style");
            }
            _ => panic!("expected one approve effect"),
        }
        assert!(browser(&app).busy, "a second press is ignored while it is in flight");
        assert!(dispatch(&mut app, ContextAction::Approve).is_empty());

        dispatch(&mut app, ContextAction::WriteFailed("500".to_owned()));
        assert!(!browser(&app).busy);
        focus(&mut app, "zebra");
        assert!(dispatch(&mut app, ContextAction::Reject).is_empty());
        assert_eq!(browser(&app).error.as_deref(), Some("zebra has no pending proposal"));
    }

    #[test]
    fn rejecting_posts_the_reject_route() {
        let mut app = app_with_sessions();
        open(&mut app);
        let effects = dispatch(&mut app, ContextAction::Reject);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Context(ContextEffect::Reject { name, .. })] if name == "style"
        ));
    }

    #[test]
    fn deleting_takes_two_presses_on_the_same_item() {
        let mut app = app_with_sessions();
        open(&mut app);
        let style = uuid::Uuid::from_u128(2).to_string();
        assert!(dispatch(&mut app, ContextAction::Delete).is_empty());
        assert_eq!(browser(&app).armed_delete.as_deref(), Some(style.as_str()));
        dispatch(&mut app, ContextAction::SelectNext);
        assert!(browser(&app).armed_delete.is_none(), "moving disarms");
        assert!(dispatch(&mut app, ContextAction::Delete).is_empty());
        let effects = dispatch(&mut app, ContextAction::Delete);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Context(ContextEffect::Delete { name, .. })] if name == "zebra"
        ));
        assert!(browser(&app).armed_delete.is_none());
    }

    #[test]
    fn attaching_posts_the_focused_memory_to_the_session() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, ContextAction::ToggleTab);
        assert!(dispatch(&mut app, ContextAction::Attach).is_empty(), "a rule cannot be attached");
        assert!(browser(&app).error.is_some());
        focus(&mut app, "alpha");
        let effects = dispatch(&mut app, ContextAction::Attach);
        match effects.as_slice() {
            [Effect::Context(ContextEffect::Attach { session_id, names })] => {
                assert_eq!(Some(session_id.clone()), app.selected_session_id());
                assert_eq!(names, &["alpha".to_owned()]);
            }
            _ => panic!("expected one attach effect"),
        }
    }

    #[test]
    fn a_finished_write_toasts_and_reloads() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, ContextAction::Approve);
        let effects =
            dispatch(&mut app, ContextAction::Done("style: proposal approved".to_owned()));
        assert!(matches!(effects.as_slice(), [Effect::Context(ContextEffect::Fetch { .. })]));
        assert!(browser(&app).loading);
        assert!(app.toasts.latest().expect("a toast").text.contains("style"));
        assert_eq!(app.view(), View::ContextItems, "the browser stays open");
    }

    #[test]
    fn a_reload_keeps_the_focused_item() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, ContextAction::ToggleTab);
        focus(&mut app, "zebra");
        let mut fewer = items();
        fewer.remove(1);
        dispatch(&mut app, ContextAction::Loaded { items: fewer, session: Some(session()) });
        assert_eq!(browser(&app).focused_row().expect("a row").name, "zebra");
    }

    #[test]
    fn a_failed_load_stops_the_spinner_and_offers_nothing() {
        let mut app = app_with_sessions();
        dispatch(&mut app, ContextAction::Open);
        dispatch(&mut app, ContextAction::Failed("502".to_owned()));
        assert!(!browser(&app).loading);
        assert!(browser(&app).rows.is_empty());
        assert!(dispatch(&mut app, ContextAction::Approve).is_empty());
    }

    #[test]
    fn closing_posts_nothing_and_leaves_the_view() {
        let mut app = app_with_sessions();
        open(&mut app);
        assert!(dispatch(&mut app, ContextAction::Close).is_empty());
        assert!(app.context_items.is_none());
        assert_ne!(app.view(), View::ContextItems);
    }

    #[test]
    fn the_attach_summary_names_what_landed_and_what_did_not() {
        let out = AttachContextResponse {
            attached: vec!["alpha".to_owned()],
            unknown: vec!["ghost".to_owned()],
        };
        assert_eq!(attach_summary(&out), "attached alpha (unknown: ghost)");
        assert_eq!(attach_summary(&AttachContextResponse::default()), "nothing attached");
    }
}
