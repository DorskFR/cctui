//! The dispatchers panel: enroll, rebind and remove the executors that run
//! dispatched sessions.
//!
//! Listing is readable by anyone the server answers; every mutation needs the
//! `enroll` scope, which is the same check the server makes, so the panel grays
//! what the key cannot do instead of letting it 403.
//!
//! The minted key is held only as long as its dialog is open and is never put in
//! a log, a toast or the draft store: closing the dialog drops it, and it cannot
//! be fetched again.

use cctui_client::{Dispatcher, EnrollDispatcher, EnrolledDispatcher, UpdateDispatcher};
use cctui_proto::models::MachineLiveness;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// The scope the server requires for every mutation here.
pub const ENROLL_SCOPE: &str = "enroll";

/// Which field the enroll or edit form is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Kind,
    Binding,
}

impl Field {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Name => Self::Kind,
            Self::Kind => Self::Binding,
            Self::Binding => Self::Name,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Kind => "kind",
            Self::Binding => "account or pool:name",
        }
    }
}

/// The kinds the enroll form cycles, as the dispatcher binaries report them.
pub const KINDS: [&str; 3] = ["kubernetes", "docker", "http"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub name: String,
    /// Index into [`KINDS`].
    pub kind: usize,
    /// `pool:work` binds a pool; anything else is an account name.
    pub binding: String,
    pub field_name: bool,
}

impl Form {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        KINDS.get(self.kind).copied().unwrap_or("http")
    }

    /// `pool:x` is a pool, everything else an account; empty binds neither.
    #[must_use]
    pub fn binding(&self) -> (Option<String>, Option<String>) {
        let raw = self.binding.trim();
        if raw.is_empty() {
            return (None, None);
        }
        match raw.strip_prefix("pool:") {
            Some(pool) if !pool.trim().is_empty() => (None, Some(pool.trim().to_owned())),
            _ => (Some(raw.to_owned()), None),
        }
    }

    #[must_use]
    pub fn enroll_request(&self) -> EnrollDispatcher {
        let (account, pool) = self.binding();
        EnrollDispatcher {
            name: self.name.trim().to_owned(),
            kind: Some(self.kind().to_owned()),
            account,
            pool,
        }
    }

    #[must_use]
    pub fn update_request(&self) -> UpdateDispatcher {
        let (account, pool) = self.binding();
        UpdateDispatcher { name: Some(self.name.trim().to_owned()), account, pool }
    }
}

/// What the panel is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Enroll {
        form: Form,
        field: Field,
    },
    Edit {
        id: String,
        form: Form,
        field: Field,
    },
    ConfirmDelete {
        id: String,
        name: String,
    },
    /// The one-shot key, held only while this dialog is up.
    ShowKey {
        name: String,
        key: String,
    },
}

#[derive(Debug, Default)]
pub struct Dispatchers {
    pub rows: Vec<Dispatcher>,
    pub selected: usize,
    pub open: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub mode: Option<Mode>,
}

impl Dispatchers {
    #[must_use]
    pub fn selected_row(&self) -> Option<&Dispatcher> {
        self.rows.get(self.selected)
    }

    /// The mode, or `Browse` when the panel is simply listing.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode.clone().unwrap_or(Mode::Browse)
    }
}

/// What the state column says, matching the web UI's liveness label: a live
/// socket is `connected`, otherwise the tier derived from the last heartbeat.
#[must_use]
pub const fn state_text(row: &Dispatcher) -> &'static str {
    if row.connected {
        return "connected";
    }
    match row.liveness {
        MachineLiveness::Online => "online",
        MachineLiveness::Stale => "stale",
        MachineLiveness::Offline => "offline",
    }
}

/// `pool:work`, an account name, or `-`.
#[must_use]
pub fn binding_text(row: &Dispatcher) -> String {
    if let Some(pool) = row.default_pool.as_deref().filter(|p| !p.is_empty()) {
        return format!("pool:{pool}");
    }
    row.default_account.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| "-".to_owned())
}

pub enum DispatcherAction {
    Open,
    Close,
    Refresh,
    Loaded(Vec<Dispatcher>),
    Failed(String),
    SelectNext,
    SelectPrev,
    StartEnroll,
    StartEdit,
    StartDelete,
    /// A key typed into whichever form is up.
    Key(crossterm::event::KeyEvent),
    /// `Tab`: next field, or cycle the kind when the cursor is on it.
    NextField,
    Commit,
    Cancel,
    /// The enrollment succeeded; the key is shown once.
    Enrolled {
        name: String,
        reply: Box<EnrolledDispatcher>,
    },
    /// `y` in the key dialog: put it on the clipboard.
    CopyKey,
}

#[allow(clippy::too_many_lines)]
pub fn reduce_dispatchers(app: &mut App, action: DispatcherAction) -> Vec<Effect> {
    match action {
        DispatcherAction::Open => {
            app.dispatchers.open = true;
            app.router.push(super::state::View::Dispatchers);
            refresh(app)
        }
        DispatcherAction::Close => {
            // Whatever the dialog held goes with it; the key is not kept.
            app.dispatchers.mode = None;
            if std::mem::take(&mut app.dispatchers.open) {
                app.router.pop();
            }
            Vec::new()
        }
        DispatcherAction::Refresh => refresh(app),
        DispatcherAction::Loaded(rows) => {
            app.dispatchers.rows = rows;
            app.dispatchers.loading = false;
            app.dispatchers.error = None;
            clamp(app);
            Vec::new()
        }
        DispatcherAction::Failed(message) => {
            app.dispatchers.loading = false;
            app.dispatchers.error = Some(message);
            Vec::new()
        }
        DispatcherAction::SelectNext => {
            let len = app.dispatchers.rows.len();
            if len > 0 {
                app.dispatchers.selected = (app.dispatchers.selected + 1).min(len - 1);
            }
            Vec::new()
        }
        DispatcherAction::SelectPrev => {
            app.dispatchers.selected = app.dispatchers.selected.saturating_sub(1);
            Vec::new()
        }
        DispatcherAction::StartEnroll => {
            if !allowed(app) {
                return Vec::new();
            }
            app.dispatchers.mode = Some(Mode::Enroll { form: Form::default(), field: Field::Name });
            Vec::new()
        }
        DispatcherAction::StartEdit => {
            if !allowed(app) {
                return Vec::new();
            }
            let Some(row) = app.dispatchers.selected_row() else { return Vec::new() };
            let form = Form {
                name: row.name.clone(),
                kind: KINDS.iter().position(|k| *k == row.kind).unwrap_or(2),
                binding: binding_text(row).replace('-', ""),
                field_name: true,
            };
            app.dispatchers.mode =
                Some(Mode::Edit { id: row.id.clone(), form, field: Field::Name });
            Vec::new()
        }
        DispatcherAction::StartDelete => {
            if !allowed(app) {
                return Vec::new();
            }
            let Some(row) = app.dispatchers.selected_row() else { return Vec::new() };
            app.dispatchers.mode =
                Some(Mode::ConfirmDelete { id: row.id.clone(), name: row.name.clone() });
            Vec::new()
        }
        DispatcherAction::Key(key) => {
            key_in_mode(app, key);
            Vec::new()
        }
        DispatcherAction::NextField => {
            next_field(app);
            Vec::new()
        }
        DispatcherAction::Commit => commit(app),
        // Esc backs out of a form first and closes the panel only when there is
        // no form left to leave.
        DispatcherAction::Cancel => {
            if app.dispatchers.mode.take().is_some() {
                return Vec::new();
            }
            reduce_dispatchers(app, DispatcherAction::Close)
        }
        DispatcherAction::Enrolled { name, reply } => {
            app.dispatchers.mode = Some(Mode::ShowKey { name, key: reply.dispatcher_key.clone() });
            refresh(app)
        }
        DispatcherAction::CopyKey => {
            let Some(Mode::ShowKey { key, .. }) = app.dispatchers.mode.clone() else {
                return Vec::new();
            };
            // `Effect::Copy` is the one clipboard path (OSC 52 first, so it works
            // over ssh); its own label is what the user is told, never the key.
            vec![Effect::Copy { text: key, label: "dispatcher key" }]
        }
    }
}

/// Whether the key may mutate. Mirrors the server's own `requires(Scope::Enroll)`
/// so the panel refuses locally for the same reason the server would.
#[must_use]
pub fn can_manage(app: &App) -> bool {
    match &app.auth {
        super::identity::AuthState::Identified(identity) => {
            identity.scopes.iter().any(|s| s == ENROLL_SCOPE)
        }
        _ => false,
    }
}

fn allowed(app: &mut App) -> bool {
    if can_manage(app) {
        return true;
    }
    app.toast(Level::Warn, "this key lacks the enroll scope — dispatchers are read-only");
    false
}

fn refresh(app: &mut App) -> Vec<Effect> {
    app.dispatchers.loading = true;
    vec![Effect::FetchDispatchers]
}

fn clamp(app: &mut App) {
    let len = app.dispatchers.rows.len();
    app.dispatchers.selected = if len == 0 { 0 } else { app.dispatchers.selected.min(len - 1) };
}

fn next_field(app: &mut App) {
    let Some(mode) = app.dispatchers.mode.as_mut() else { return };
    match mode {
        Mode::Enroll { form, field } | Mode::Edit { form, field, .. } => {
            if *field == Field::Kind {
                form.kind = (form.kind + 1) % KINDS.len();
                return;
            }
            *field = field.next();
        }
        _ => {}
    }
}

fn key_in_mode(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let Some(mode) = app.dispatchers.mode.as_mut() else { return };
    match mode {
        Mode::Enroll { form, field } | Mode::Edit { form, field, .. } => {
            let target = match field {
                Field::Name => &mut form.name,
                Field::Binding => &mut form.binding,
                // The kind is a cycle, not a text field.
                Field::Kind => return,
            };
            match key.code {
                KeyCode::Char(c)
                    if !key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    target.push(c);
                }
                KeyCode::Backspace => {
                    target.pop();
                }
                _ => {}
            }
        }
        // A confirmation, the key dialog and plain browsing take no text.
        Mode::ConfirmDelete { .. } | Mode::ShowKey { .. } | Mode::Browse => {}
    }
}

fn commit(app: &mut App) -> Vec<Effect> {
    let Some(mode) = app.dispatchers.mode.clone() else { return Vec::new() };
    match mode {
        Mode::Enroll { form, .. } => {
            if form.name.trim().is_empty() {
                app.toast(Level::Warn, "a dispatcher needs a name");
                return Vec::new();
            }
            let name = form.name.trim().to_owned();
            app.dispatchers.mode = None;
            vec![Effect::EnrollDispatcher { name, request: Box::new(form.enroll_request()) }]
        }
        Mode::Edit { id, form, .. } => {
            if form.name.trim().is_empty() {
                app.toast(Level::Warn, "a dispatcher needs a name");
                return Vec::new();
            }
            app.dispatchers.mode = None;
            vec![Effect::UpdateDispatcher { id, request: Box::new(form.update_request()) }]
        }
        Mode::ConfirmDelete { id, name } => {
            app.dispatchers.mode = None;
            app.toast(Level::Info, format!("removed {name}"));
            vec![Effect::DeleteDispatcher { id }]
        }
        // Enter dismisses the key dialog; the key is gone with it.
        Mode::ShowKey { .. } | Mode::Browse => {
            app.dispatchers.mode = None;
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use cctui_client::Dispatcher;
    use cctui_proto::models::MachineLiveness;

    use super::{DispatcherAction, Field, Form, Mode, binding_text, can_manage, state_text};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    fn row(id: &str, name: &str, kind: &str, connected: bool, tier: MachineLiveness) -> Dispatcher {
        Dispatcher {
            id: id.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            liveness: tier,
            connected,
            last_seen_at: chrono::DateTime::from_timestamp_millis(0).expect("stamp"),
            default_account: None,
            default_pool: None,
        }
    }

    fn app_with_scope(scopes: &[&str]) -> App {
        let mut app = App::new();
        app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
            role: "user".to_owned(),
            user_name: Some("dev".to_owned()),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            token_preview: "abc".to_owned(),
        });
        app
    }

    fn act(app: &mut App, action: DispatcherAction) -> Vec<Effect> {
        reduce(app, Action::Dispatchers(action))
    }

    fn loaded(app: &mut App) {
        act(
            app,
            DispatcherAction::Loaded(vec![
                row("d-1", "k8s-cyberia", "kubernetes", true, MachineLiveness::Online),
                row("d-2", "docker-mac", "docker", false, MachineLiveness::Stale),
            ]),
        );
    }

    #[test]
    fn the_state_column_matches_the_web_uis_label() {
        assert_eq!(state_text(&row("d", "n", "http", true, MachineLiveness::Offline)), "connected");
        assert_eq!(state_text(&row("d", "n", "http", false, MachineLiveness::Online)), "online");
        assert_eq!(state_text(&row("d", "n", "http", false, MachineLiveness::Stale)), "stale");
        assert_eq!(state_text(&row("d", "n", "http", false, MachineLiveness::Offline)), "offline");
    }

    #[test]
    fn the_binding_column_prefers_a_pool_then_an_account() {
        let mut r = row("d", "n", "http", false, MachineLiveness::Online);
        assert_eq!(binding_text(&r), "-");
        r.default_account = Some("personal-max".to_owned());
        assert_eq!(binding_text(&r), "personal-max");
        r.default_pool = Some("work".to_owned());
        assert_eq!(binding_text(&r), "pool:work", "a pool wins, as the server resolves it");
    }

    #[test]
    fn a_binding_string_splits_into_a_pool_or_an_account() {
        let mut form = Form::default();
        assert_eq!(form.binding(), (None, None));
        form.binding = "personal-max".to_owned();
        assert_eq!(form.binding(), (Some("personal-max".to_owned()), None));
        form.binding = "pool:work".to_owned();
        assert_eq!(form.binding(), (None, Some("work".to_owned())));
        form.binding = "pool:".to_owned();
        assert_eq!(form.binding(), (Some("pool:".to_owned()), None), "an empty pool is a name");
    }

    #[test]
    fn opening_fetches_and_closing_pops() {
        let mut app = app_with_scope(&["enroll"]);
        let effects = act(&mut app, DispatcherAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchDispatchers)));
        assert_eq!(app.view(), crate::app::View::Dispatchers);
        act(&mut app, DispatcherAction::Close);
        assert!(!app.dispatchers.open);
        assert_ne!(app.view(), crate::app::View::Dispatchers);
    }

    #[test]
    fn a_key_without_the_enroll_scope_can_look_but_not_touch() {
        let mut app = app_with_scope(&["sessions:write"]);
        assert!(!can_manage(&app));
        loaded(&mut app);
        assert_eq!(app.dispatchers.rows.len(), 2, "listing still works");

        act(&mut app, DispatcherAction::StartEnroll);
        assert_eq!(app.dispatchers.mode, None, "no form opens");
        assert!(app.toasts.latest().expect("a toast").text.contains("enroll scope"));
        act(&mut app, DispatcherAction::StartEdit);
        assert_eq!(app.dispatchers.mode, None);
        act(&mut app, DispatcherAction::StartDelete);
        assert_eq!(app.dispatchers.mode, None);
    }

    #[test]
    fn enrolling_walks_the_fields_and_cycles_the_kind() {
        let mut app = app_with_scope(&["enroll"]);
        loaded(&mut app);
        act(&mut app, DispatcherAction::StartEnroll);
        for c in "k8s".chars() {
            act(
                &mut app,
                DispatcherAction::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
        }
        act(&mut app, DispatcherAction::NextField);
        // On the kind now: Tab cycles it rather than leaving the field.
        act(&mut app, DispatcherAction::NextField);
        match app.dispatchers.mode() {
            Mode::Enroll { form, field } => {
                assert_eq!(form.name, "k8s");
                assert_eq!(form.kind(), "docker");
                assert_eq!(field, Field::Kind);
            }
            other => panic!("expected the enroll form, got {other:?}"),
        }
    }

    #[test]
    fn enrolling_sends_the_name_kind_and_binding() {
        let mut app = app_with_scope(&["enroll"]);
        act(&mut app, DispatcherAction::StartEnroll);
        if let Some(Mode::Enroll { form, .. }) = app.dispatchers.mode.as_mut() {
            form.name = "k8s-cyberia".to_owned();
            form.kind = 0;
            form.binding = "pool:work".to_owned();
        }
        match act(&mut app, DispatcherAction::Commit).as_slice() {
            [Effect::EnrollDispatcher { name, request }] => {
                assert_eq!(name, "k8s-cyberia");
                assert_eq!(request.name, "k8s-cyberia");
                assert_eq!(request.kind.as_deref(), Some("kubernetes"));
                assert_eq!(request.pool.as_deref(), Some("work"));
                assert_eq!(request.account, None);
            }
            other => panic!("expected one enroll effect, got {}", other.len()),
        }
        assert_eq!(app.dispatchers.mode, None, "the form closes on submit");
    }

    #[test]
    fn a_nameless_dispatcher_is_refused_rather_than_sent() {
        let mut app = app_with_scope(&["enroll"]);
        act(&mut app, DispatcherAction::StartEnroll);
        assert!(act(&mut app, DispatcherAction::Commit).is_empty());
        assert!(matches!(app.dispatchers.mode(), Mode::Enroll { .. }), "still asking");
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn the_minted_key_is_shown_once_and_dropped_with_its_dialog() {
        let mut app = app_with_scope(&["enroll"]);
        act(
            &mut app,
            DispatcherAction::Enrolled {
                name: "k8s".to_owned(),
                reply: Box::new(cctui_client::EnrolledDispatcher {
                    dispatcher_id: "d-9".to_owned(),
                    dispatcher_key: "SECRET-KEY".to_owned(),
                }),
            },
        );
        match app.dispatchers.mode() {
            Mode::ShowKey { name, key } => {
                assert_eq!(name, "k8s");
                assert_eq!(key, "SECRET-KEY");
            }
            other => panic!("expected the key dialog, got {other:?}"),
        }

        // Nothing else holds it: not a toast, not the composer, not a draft.
        assert!(app.toasts.latest().is_none_or(|t| !t.text.contains("SECRET-KEY")));
        act(&mut app, DispatcherAction::Close);
        assert_eq!(app.dispatchers.mode, None);
        assert!(!format!("{:?}", app.dispatchers).contains("SECRET-KEY"));
    }

    #[test]
    fn copying_the_key_names_the_clipboard_and_not_the_key() {
        let mut app = app_with_scope(&["enroll"]);
        act(
            &mut app,
            DispatcherAction::Enrolled {
                name: "k8s".to_owned(),
                reply: Box::new(cctui_client::EnrolledDispatcher {
                    dispatcher_id: "d-9".to_owned(),
                    dispatcher_key: "SECRET-KEY".to_owned(),
                }),
            },
        );
        match act(&mut app, DispatcherAction::CopyKey).as_slice() {
            [Effect::Copy { text, label }] => {
                assert_eq!(text, "SECRET-KEY");
                assert_eq!(*label, "dispatcher key", "the label is what gets said, not the key");
            }
            other => panic!("expected a clipboard effect, got {}", other.len()),
        }
    }

    #[test]
    fn editing_prefills_from_the_row_and_sends_a_patch() {
        let mut app = app_with_scope(&["enroll"]);
        loaded(&mut app);
        act(&mut app, DispatcherAction::SelectNext);
        act(&mut app, DispatcherAction::StartEdit);
        match app.dispatchers.mode() {
            Mode::Edit { id, form, .. } => {
                assert_eq!(id, "d-2");
                assert_eq!(form.name, "docker-mac");
                assert_eq!(form.kind(), "docker");
            }
            other => panic!("expected the edit form, got {other:?}"),
        }
        match act(&mut app, DispatcherAction::Commit).as_slice() {
            [Effect::UpdateDispatcher { id, request }] => {
                assert_eq!(id, "d-2");
                assert_eq!(request.name.as_deref(), Some("docker-mac"));
            }
            other => panic!("expected an update, got {}", other.len()),
        }
    }

    #[test]
    fn deleting_waits_for_a_confirmation() {
        let mut app = app_with_scope(&["enroll"]);
        loaded(&mut app);
        act(&mut app, DispatcherAction::StartDelete);
        assert_eq!(
            app.dispatchers.mode(),
            Mode::ConfirmDelete { id: "d-1".to_owned(), name: "k8s-cyberia".to_owned() }
        );
        act(&mut app, DispatcherAction::Cancel);
        assert_eq!(app.dispatchers.mode, None);

        act(&mut app, DispatcherAction::StartDelete);
        match act(&mut app, DispatcherAction::Commit).as_slice() {
            [Effect::DeleteDispatcher { id }] => assert_eq!(id, "d-1"),
            other => panic!("expected a delete, got {}", other.len()),
        }
    }

    #[test]
    fn the_cursor_stays_inside_the_table() {
        let mut app = app_with_scope(&["enroll"]);
        loaded(&mut app);
        for _ in 0..5 {
            act(&mut app, DispatcherAction::SelectNext);
        }
        assert_eq!(app.dispatchers.selected, 1);
        act(
            &mut app,
            DispatcherAction::Loaded(vec![row(
                "d-1",
                "k8s-cyberia",
                "kubernetes",
                true,
                MachineLiveness::Online,
            )]),
        );
        assert_eq!(app.dispatchers.selected, 0, "a shorter list brings it back");
    }

    #[test]
    fn a_failed_fetch_is_reported() {
        let mut app = app_with_scope(&["enroll"]);
        act(&mut app, DispatcherAction::Open);
        act(&mut app, DispatcherAction::Failed("forbidden".to_owned()));
        assert!(!app.dispatchers.loading);
        assert_eq!(app.dispatchers.error.as_deref(), Some("forbidden"));
    }
}
