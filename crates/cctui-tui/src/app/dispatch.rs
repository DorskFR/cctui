//! The spawn dialog's Dispatch tab: hand a job to a docker/kube/http
//! dispatcher with a context pack.
//!
//! Self-contained so the core dialog plugs it in with one field and one arm:
//! [`DispatchFields`] is the state, [`reduce_dispatch`] the reducer,
//! `views::dispatch::draw` the section, and [`available`] says whether the tab
//! should be offered at all. The request body itself is
//! `cctui_clientcore::dispatch`, shared with the web UI.

use cctui_clientcore::dispatch::{self, ContextPack, DispatchForm};
use crossterm::event::{KeyCode, KeyEvent};

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// The fields in the order the tab shows them, which is also the order `Tab`
/// walks. The adapter is a radio, not a text field, so it is not in here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Dispatcher,
    Repo,
    Ticket,
    Timeout,
    Prompt,
    PackUrl,
    PackRef,
    PackSubdir,
    PackToken,
}

impl Field {
    pub const ORDER: [Self; 9] = [
        Self::Dispatcher,
        Self::Repo,
        Self::Ticket,
        Self::Timeout,
        Self::Prompt,
        Self::PackUrl,
        Self::PackRef,
        Self::PackSubdir,
        Self::PackToken,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dispatcher => "Dispatcher",
            Self::Repo => "Repo",
            Self::Ticket => "Ticket",
            Self::Timeout => "Timeout",
            Self::Prompt => "Prompt",
            Self::PackUrl => "URL",
            Self::PackRef => "ref",
            Self::PackSubdir => "subdir",
            Self::PackToken => "token",
        }
    }

    /// A git credential is shown as dots and never remembered.
    pub const fn secret(self) -> bool {
        matches!(self, Self::PackToken)
    }
}

/// Everything the Dispatch tab holds. The account and provider come from the
/// core dialog, which owns the account picker.
#[derive(Debug, Clone, Default)]
pub struct DispatchFields {
    pub form: DispatchForm,
    pub pack: ContextPack,
    /// Dispatchers the server offers, `GET /sessions/dispatchers`.
    pub dispatchers: Vec<String>,
    pub focus: usize,
    /// Remembered per dispatcher+repo, as the web UI remembers it.
    remembered: std::collections::HashMap<String, Remembered>,
}

/// What is worth restoring when the operator comes back to the same job. The
/// token is deliberately absent.
#[derive(Debug, Clone, Default)]
struct Remembered {
    ticket: String,
    prompt: String,
    timeout: String,
    pack_url: String,
    pack_ref: String,
    pack_subdir: String,
}

impl DispatchFields {
    pub const fn field(&mut self, which: Field) -> &mut String {
        match which {
            Field::Dispatcher => &mut self.form.dispatcher,
            Field::Repo => &mut self.form.repo,
            Field::Ticket => &mut self.form.ticket,
            Field::Timeout => &mut self.form.timeout,
            Field::Prompt => &mut self.form.prompt,
            Field::PackUrl => &mut self.pack.url,
            Field::PackRef => &mut self.pack.r#ref,
            Field::PackSubdir => &mut self.pack.subdir,
            Field::PackToken => &mut self.pack.token,
        }
    }

    #[must_use]
    pub fn read(&self, which: Field) -> &str {
        match which {
            Field::Dispatcher => &self.form.dispatcher,
            Field::Repo => &self.form.repo,
            Field::Ticket => &self.form.ticket,
            Field::Timeout => &self.form.timeout,
            Field::Prompt => &self.form.prompt,
            Field::PackUrl => &self.pack.url,
            Field::PackRef => &self.pack.r#ref,
            Field::PackSubdir => &self.pack.subdir,
            Field::PackToken => &self.pack.token,
        }
    }

    #[must_use]
    pub fn focused(&self) -> Field {
        Field::ORDER[self.focus.min(Field::ORDER.len() - 1)]
    }

    #[must_use]
    pub fn is_codex(&self) -> bool {
        self.form.dispatch_adapter == "codex"
    }

    /// Nothing to dispatch to without a dispatcher name.
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.form.dispatcher.trim().is_empty()
    }

    fn remember(&mut self) {
        let key = dispatch::memory_key(&self.form.dispatcher, &self.form.repo);
        self.remembered.insert(
            key,
            Remembered {
                ticket: self.form.ticket.clone(),
                prompt: self.form.prompt.clone(),
                timeout: self.form.timeout.clone(),
                pack_url: self.pack.url.clone(),
                pack_ref: self.pack.r#ref.clone(),
                pack_subdir: self.pack.subdir.clone(),
            },
        );
    }

    /// Restores what was last dispatched to this dispatcher+repo. The token is
    /// never restored: it was never kept.
    fn recall(&mut self) {
        let key = dispatch::memory_key(&self.form.dispatcher, &self.form.repo);
        let Some(seen) = self.remembered.get(&key).cloned() else { return };
        self.form.ticket = seen.ticket;
        self.form.prompt = seen.prompt;
        self.form.timeout = seen.timeout;
        self.pack.url = seen.pack_url;
        self.pack.r#ref = seen.pack_ref;
        self.pack.subdir = seen.pack_subdir;
    }
}

pub enum DispatchAction {
    /// Show the tab on its own. The spawn dialog will carry it instead, and
    /// then this is the dialog opening on the Dispatch target.
    Open,
    Close,
    DispatchersLoaded(Vec<String>),
    FocusNext,
    FocusPrev,
    Key(KeyEvent),
    /// Cycle the harness radio.
    ToggleAdapter,
    /// Take the next dispatcher the server offered.
    CycleDispatcher,
    Submit,
    Submitted {
        session_id: String,
        /// The server had this one already: an idempotent resubmit.
        existing: bool,
    },
}

pub fn reduce_dispatch(app: &mut App, action: DispatchAction) -> Vec<Effect> {
    match action {
        DispatchAction::Open => open(app),
        DispatchAction::Close => close(app),
        DispatchAction::DispatchersLoaded(names) => {
            let first = names.first().cloned();
            app.dispatch.dispatchers = names;
            if app.dispatch.form.dispatcher.is_empty()
                && let Some(name) = first
            {
                app.dispatch.form.dispatcher = name;
                app.dispatch.recall();
            }
            Vec::new()
        }
        DispatchAction::FocusNext => {
            app.dispatch.focus = (app.dispatch.focus + 1) % Field::ORDER.len();
            Vec::new()
        }
        DispatchAction::FocusPrev => {
            app.dispatch.focus =
                app.dispatch.focus.checked_sub(1).unwrap_or(Field::ORDER.len() - 1);
            Vec::new()
        }
        DispatchAction::Key(key) => {
            let which = app.dispatch.focused();
            edit(app.dispatch.field(which), key);
            if which == Field::Dispatcher || which == Field::Repo {
                app.dispatch.recall();
            }
            Vec::new()
        }
        DispatchAction::ToggleAdapter => {
            app.dispatch.form.dispatch_adapter =
                if app.dispatch.is_codex() { String::new() } else { "codex".to_owned() };
            Vec::new()
        }
        DispatchAction::CycleDispatcher => {
            cycle_dispatcher(app);
            Vec::new()
        }
        DispatchAction::Submit => submit(app),
        DispatchAction::Submitted { session_id, existing } => {
            let what = if existing { "already running" } else { "dispatched" };
            app.toast(Level::Info, format!("{what}: {}", short(&session_id)));
            app.dispatch.remember();
            super::conversation::switch_to(app, session_id)
        }
    }
}

fn open(app: &mut App) -> Vec<Effect> {
    if !available(app) {
        app.toast(Level::Info, "no dispatcher is enrolled");
        return Vec::new();
    }
    app.dispatch.focus = 0;
    app.router.push(super::state::View::Dispatch);
    Vec::new()
}

fn close(app: &mut App) -> Vec<Effect> {
    if app.view() == super::state::View::Dispatch {
        app.router.pop();
    }
    Vec::new()
}

fn cycle_dispatcher(app: &mut App) {
    if app.dispatch.dispatchers.is_empty() {
        return;
    }
    let next = app
        .dispatch
        .dispatchers
        .iter()
        .position(|d| *d == app.dispatch.form.dispatcher)
        .map_or(0, |i| (i + 1) % app.dispatch.dispatchers.len());
    app.dispatch.form.dispatcher = app.dispatch.dispatchers[next].clone();
    app.dispatch.recall();
}

fn submit(app: &mut App) -> Vec<Effect> {
    if !app.dispatch.ready() {
        app.toast(Level::Warn, "pick a dispatcher first");
        return Vec::new();
    }
    // The id is the idempotency key, so the same job resubmitted lands on the
    // session already running instead of starting a second one.
    let session_id = dispatch_id(&app.dispatch);
    let body = dispatch::build_dispatch_body(
        &app.dispatch.form,
        &[],
        &app.dispatch.pack,
        None,
        &session_id,
    );
    vec![Effect::Dispatch { body: Box::new(body) }]
}

/// A stable id for one job: the same dispatcher, repo and ticket resubmit onto
/// the same session rather than starting another.
fn dispatch_id(fields: &DispatchFields) -> String {
    let key = format!(
        "{}|{}|{}",
        fields.form.dispatcher.trim(),
        fields.form.repo.trim(),
        fields.form.ticket.trim()
    );
    format!("disp-{:016x}", fnv1a(&key))
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn short(session_id: &str) -> &str {
    session_id.get(..12).unwrap_or(session_id)
}

/// The tab is hidden when the server offers no dispatcher: there is nothing to
/// dispatch to, and an empty picker is worse than no tab.
#[must_use]
pub const fn available(app: &App) -> bool {
    !app.dispatch.dispatchers.is_empty()
}

/// One field's worth of editing. Each dialog in the TUI keeps its own, because
/// each one claims a different set of keys around it.
fn edit(buffer: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => buffer.push(c),
        KeyCode::Backspace => {
            buffer.pop();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{DispatchAction, Field, available, dispatch_id};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    fn act(app: &mut App, action: DispatchAction) -> Vec<Effect> {
        reduce(app, Action::Dispatch(action))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            act(app, DispatchAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
    }

    fn backspace(app: &mut App, times: usize) {
        for _ in 0..times {
            act(app, DispatchAction::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        }
    }

    fn focus(app: &mut App, which: Field) {
        app.dispatch.focus = Field::ORDER.iter().position(|f| *f == which).expect("a field");
    }

    #[test]
    fn the_tab_is_hidden_until_the_server_offers_a_dispatcher() {
        let mut app = app();
        assert!(!available(&app));
        act(&mut app, DispatchAction::DispatchersLoaded(vec!["k8s-cyberia".to_owned()]));
        assert!(available(&app));
        assert_eq!(app.dispatch.form.dispatcher, "k8s-cyberia", "the first is preselected");
    }

    #[test]
    fn the_focus_walks_the_fields_and_wraps_both_ways() {
        let mut app = app();
        assert_eq!(app.dispatch.focused(), Field::Dispatcher);
        act(&mut app, DispatchAction::FocusPrev);
        assert_eq!(app.dispatch.focused(), Field::PackToken, "it wraps backwards");
        act(&mut app, DispatchAction::FocusNext);
        assert_eq!(app.dispatch.focused(), Field::Dispatcher);
    }

    #[test]
    fn typing_lands_in_the_focused_field_only() {
        let mut app = app();
        focus(&mut app, Field::Repo);
        type_text(&mut app, "cctui");
        assert_eq!(app.dispatch.form.repo, "cctui");
        assert!(app.dispatch.form.ticket.is_empty());

        act(&mut app, DispatchAction::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(app.dispatch.form.repo, "cctu");
    }

    #[test]
    fn the_harness_radio_toggles_between_claude_and_codex() {
        let mut app = app();
        assert!(!app.dispatch.is_codex());
        act(&mut app, DispatchAction::ToggleAdapter);
        assert!(app.dispatch.is_codex());
        act(&mut app, DispatchAction::ToggleAdapter);
        assert!(!app.dispatch.is_codex());
    }

    #[test]
    fn the_dispatcher_key_cycles_what_the_server_offered() {
        let mut app = app();
        act(&mut app, DispatchAction::DispatchersLoaded(vec!["a".to_owned(), "b".to_owned()]));
        act(&mut app, DispatchAction::CycleDispatcher);
        assert_eq!(app.dispatch.form.dispatcher, "b");
        act(&mut app, DispatchAction::CycleDispatcher);
        assert_eq!(app.dispatch.form.dispatcher, "a", "it wraps");
    }

    #[test]
    fn submitting_without_a_dispatcher_says_so_and_posts_nothing() {
        let mut app = app();
        assert!(act(&mut app, DispatchAction::Submit).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("pick a dispatcher"));
    }

    #[test]
    fn the_body_is_the_shared_one_and_carries_the_context_pack() {
        let mut app = app();
        act(&mut app, DispatchAction::DispatchersLoaded(vec!["k8s".to_owned()]));
        focus(&mut app, Field::Repo);
        type_text(&mut app, "cctui");
        focus(&mut app, Field::Ticket);
        type_text(&mut app, "CCT-1102");
        focus(&mut app, Field::Timeout);
        type_text(&mut app, "60");
        focus(&mut app, Field::PackUrl);
        type_text(&mut app, "https://git/p.git");
        focus(&mut app, Field::PackToken);
        type_text(&mut app, "tok");

        match act(&mut app, DispatchAction::Submit).as_slice() {
            [Effect::Dispatch { body }] => {
                assert_eq!(body["dispatcher"], "k8s");
                assert_eq!(body["timeout"], 60);
                assert_eq!(body["payload"]["repo"], "cctui");
                assert_eq!(body["payload"]["context"]["issue_id"], "CCT-1102");
                assert_eq!(body["payload"]["env"]["CONTEXT_PACK_URL"], "https://git/p.git");
                assert_eq!(body["payload"]["env"]["CONTEXT_PACK_TOKEN"], "tok");
            }
            other => panic!("expected one dispatch effect, got {}", other.len()),
        }
    }

    /// The id is the idempotency key, so the same job twice is the same id.
    #[test]
    fn the_same_job_resubmits_onto_the_same_id() {
        let mut a = app();
        act(&mut a, DispatchAction::DispatchersLoaded(vec!["k8s".to_owned()]));
        focus(&mut a, Field::Ticket);
        type_text(&mut a, "CCT-1");
        let first = dispatch_id(&a.dispatch);

        let mut b = app();
        act(&mut b, DispatchAction::DispatchersLoaded(vec!["k8s".to_owned()]));
        focus(&mut b, Field::Ticket);
        type_text(&mut b, "CCT-1");
        assert_eq!(dispatch_id(&b.dispatch), first);

        type_text(&mut b, "9");
        assert_ne!(dispatch_id(&b.dispatch), first, "a different ticket is a different job");
    }

    #[test]
    fn an_idempotent_resubmit_says_the_session_was_already_running() {
        let mut app = app();
        app.sessions.push(session("s-disp", "worker", "active", "working"));
        act(
            &mut app,
            DispatchAction::Submitted { session_id: "s-disp".to_owned(), existing: true },
        );
        assert!(app.toasts.latest().expect("a toast").text.contains("already running"));
        assert_eq!(app.selected_session_id().as_deref(), Some("s-disp"));
    }

    /// The form comes back per dispatcher and repo — but never the token.
    /// Recall restores what was remembered; it never wipes what is being typed
    /// for a job that has not been dispatched before.
    #[test]
    fn the_form_is_remembered_per_dispatcher_and_repo_without_the_token() {
        let mut app = app();
        app.sessions.push(session("s-disp", "worker", "active", "working"));
        act(&mut app, DispatchAction::DispatchersLoaded(vec!["k8s".to_owned()]));

        // One job against repo `a`.
        focus(&mut app, Field::Repo);
        type_text(&mut app, "a");
        focus(&mut app, Field::Ticket);
        type_text(&mut app, "T-1");
        focus(&mut app, Field::PackToken);
        type_text(&mut app, "secret");
        act(
            &mut app,
            DispatchAction::Submitted { session_id: "s-disp".to_owned(), existing: false },
        );

        // Another against repo `b`, with its own ticket.
        focus(&mut app, Field::Repo);
        backspace(&mut app, 1);
        type_text(&mut app, "b");
        focus(&mut app, Field::Ticket);
        backspace(&mut app, 3);
        type_text(&mut app, "T-2");
        act(
            &mut app,
            DispatchAction::Submitted { session_id: "s-disp".to_owned(), existing: false },
        );

        // Clear the typed credential, so what comes back is only what was
        // remembered rather than what happens to still be on screen.
        focus(&mut app, Field::PackToken);
        backspace(&mut app, 6);
        assert!(app.dispatch.pack.token.is_empty());

        // Back to `a`: its own ticket returns, over the one just typed.
        focus(&mut app, Field::Repo);
        backspace(&mut app, 1);
        type_text(&mut app, "a");
        assert_eq!(app.dispatch.form.ticket, "T-1", "repo a's ticket came back");
        assert!(app.dispatch.pack.token.is_empty(), "the credential was never kept");
    }

    /// A repo with no history leaves what is being typed alone: clearing a
    /// half-filled form because the repo changed would lose work.
    #[test]
    fn an_unseen_repo_does_not_wipe_the_form() {
        let mut app = app();
        act(&mut app, DispatchAction::DispatchersLoaded(vec!["k8s".to_owned()]));
        focus(&mut app, Field::Ticket);
        type_text(&mut app, "T-9");
        focus(&mut app, Field::Repo);
        type_text(&mut app, "fresh");
        assert_eq!(app.dispatch.form.ticket, "T-9");
    }

    #[test]
    fn only_the_token_is_hidden_when_it_is_drawn() {
        assert!(Field::PackToken.secret());
        for other in Field::ORDER.iter().filter(|f| **f != Field::PackToken) {
            assert!(!other.secret(), "{other:?}");
        }
    }
}
