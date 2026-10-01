//! The spawn dialog's Dispatch tab: hand a job to a docker/kube/http
//! dispatcher with a context pack.
//!
//! Self-contained so the core dialog plugs it in with one field and one arm:
//! [`DispatchFields`] is the state, [`reduce_dispatch`] the reducer,
//! `views::dispatch::draw` the section, and [`available`] says whether the tab
//! should be offered at all. The request body itself is
//! `cctui_clientcore::dispatch`, shared with the web UI.

use cctui_clientcore::dispatch::{self, ContextPack, DispatchForm};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

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

    pub fn toggle_adapter(&mut self) {
        self.form.dispatch_adapter =
            if self.is_codex() { String::new() } else { "codex".to_owned() };
    }

    pub fn cycle_dispatcher(&mut self) {
        if self.dispatchers.is_empty() {
            return;
        }
        let next = self
            .dispatchers
            .iter()
            .position(|d| *d == self.form.dispatcher)
            .map_or(0, |i| (i + 1) % self.dispatchers.len());
        self.form.dispatcher = self.dispatchers[next].clone();
        self.recall();
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

/// What the dispatch tab still needs the store for: the server's answer.
pub enum DispatchAction {
    Submitted {
        session_id: String,
        /// The server had this one already: an idempotent resubmit.
        existing: bool,
    },
}

pub fn reduce_dispatch(app: &mut App, action: DispatchAction) -> Vec<Effect> {
    let DispatchAction::Submitted { session_id, existing } = action;
    let what = if existing { "already running" } else { "dispatched" };
    app.toast(Level::Info, format!("{what}: {}", short(&session_id)));
    if let Some(form) = app.spawn.as_mut() {
        for section in &mut form.sections {
            section.remember_dispatch();
        }
    }
    super::conversation::switch_to(app, session_id)
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

/// The spawn dialog's Dispatch section.
///
/// It holds its own fields, like every other section, so the dialog needed no
/// field of its own. `apply` is empty on purpose: a dispatch is a different
/// route with a different body, not a `SpawnRequest`, so the dialog's submit
/// asks this section for its body instead when the Dispatch tab is showing.
#[derive(Debug, Default)]
pub struct DispatchSection {
    pub fields: DispatchFields,
}

impl super::spawn::SpawnSection for DispatchSection {
    fn title(&self) -> &'static str {
        "Dispatch"
    }

    /// Zero rows is how a section hides: no dispatcher enrolled means there is
    /// nothing to dispatch to, and an empty picker is worse than no section.
    fn rows(&self, _fields: &cctui_clientcore::spawn::SpawnFields) -> usize {
        if self.fields.dispatchers.is_empty() { 0 } else { Field::ORDER.len() }
    }

    fn lines(
        &self,
        focused: Option<usize>,
        width: u16,
        _fields: &cctui_clientcore::spawn::SpawnFields,
    ) -> Vec<Line<'static>> {
        if self.fields.dispatchers.is_empty() {
            return Vec::new();
        }
        super::spawn::clamp_rows(crate::views::dispatch::lines(&self.fields, focused, width), width)
    }

    fn handle(
        &mut self,
        row: usize,
        key: KeyEvent,
        _fields: &mut cctui_clientcore::spawn::SpawnFields,
    ) -> Vec<Effect> {
        self.fields.focus = row;
        match (key.code, key.modifiers) {
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => self.fields.cycle_dispatcher(),
            (KeyCode::Char('h'), KeyModifiers::CONTROL) => self.fields.toggle_adapter(),
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => return self.submit(),
            _ => {
                let which = self.fields.focused();
                edit(self.fields.field(which), key);
                if matches!(which, Field::Dispatcher | Field::Repo) {
                    self.fields.recall();
                }
            }
        }
        Vec::new()
    }

    /// A dispatch is not a spawn: nothing of this section belongs on a
    /// `SpawnRequest`.
    fn apply(&self, _request: &mut cctui_proto::api::SpawnRequest) {}

    fn problems(&self) -> Vec<String> {
        if self.fields.dispatchers.is_empty() || self.fields.ready() {
            Vec::new()
        } else {
            vec!["pick a dispatcher".to_owned()]
        }
    }

    fn receive(&mut self, data: &super::spawn::SpawnData) {
        let first = data.dispatchers.first().cloned();
        self.fields.dispatchers.clone_from(&data.dispatchers);
        if self.fields.form.dispatcher.is_empty()
            && let Some(name) = first
        {
            self.fields.form.dispatcher = name;
            self.fields.recall();
        }
    }

    fn dispatch_body(&self) -> Option<serde_json::Value> {
        if !self.fields.ready() {
            return None;
        }
        // The id is the idempotency key, so the same job resubmitted lands on
        // the session already running rather than starting a second one.
        let session_id = dispatch_id(&self.fields);
        Some(dispatch::build_dispatch_body(
            &self.fields.form,
            &[],
            &self.fields.pack,
            None,
            &session_id,
        ))
    }

    fn remember_dispatch(&mut self) {
        self.fields.remember();
    }
}

impl DispatchSection {
    fn submit(&self) -> Vec<Effect> {
        use super::spawn::SpawnSection as _;
        self.dispatch_body()
            .map_or_else(Vec::new, |body| vec![Effect::Dispatch { body: Box::new(body) }])
    }
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
    use cctui_clientcore::spawn::SpawnFields;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{DispatchAction, DispatchSection, Field, dispatch_id};
    use crate::app::action::Effect;
    use crate::app::spawn::{SpawnData, SpawnSection};
    use crate::app::{Action, App, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    fn section(names: &[&str]) -> DispatchSection {
        let mut s = DispatchSection::default();
        s.receive(&SpawnData {
            dispatchers: names.iter().map(|n| (*n).to_owned()).collect(),
            ..SpawnData::default()
        });
        s
    }

    fn key(section: &mut DispatchSection, field: Field, code: KeyCode) -> Vec<Effect> {
        let row = Field::ORDER.iter().position(|f| *f == field).expect("a field");
        section.handle(row, KeyEvent::new(code, KeyModifiers::NONE), &mut SpawnFields::default())
    }

    fn type_text(section: &mut DispatchSection, field: Field, text: &str) {
        for c in text.chars() {
            key(section, field, KeyCode::Char(c));
        }
    }

    fn ctrl(section: &mut DispatchSection, c: char) -> Vec<Effect> {
        section.handle(
            0,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
            &mut SpawnFields::default(),
        )
    }

    /// Zero rows is how the section hides when nothing can be dispatched to.
    #[test]
    fn the_section_hides_until_the_server_offers_a_dispatcher() {
        let empty = DispatchSection::default();
        assert_eq!(empty.rows(&SpawnFields::default()), 0);
        assert!(empty.lines(None, 100, &SpawnFields::default()).is_empty());
        assert!(empty.problems().is_empty(), "a hidden section blocks nothing");

        let filled = section(&["k8s-cyberia"]);
        assert_eq!(filled.rows(&SpawnFields::default()), Field::ORDER.len());
        assert_eq!(filled.fields.form.dispatcher, "k8s-cyberia", "the first is preselected");
    }

    #[test]
    fn typing_lands_in_the_row_the_key_was_aimed_at() {
        let mut s = section(&["k8s"]);
        type_text(&mut s, Field::Repo, "cctui");
        assert_eq!(s.fields.form.repo, "cctui");
        assert!(s.fields.form.ticket.is_empty());
        key(&mut s, Field::Repo, KeyCode::Backspace);
        assert_eq!(s.fields.form.repo, "cctu");
    }

    #[test]
    fn the_harness_and_the_dispatcher_cycle_on_their_own_keys() {
        let mut s = section(&["a", "b"]);
        assert!(!s.fields.is_codex());
        ctrl(&mut s, 'h');
        assert!(s.fields.is_codex());
        ctrl(&mut s, 'h');
        assert!(!s.fields.is_codex());

        ctrl(&mut s, 'd');
        assert_eq!(s.fields.form.dispatcher, "b");
        ctrl(&mut s, 'd');
        assert_eq!(s.fields.form.dispatcher, "a", "it wraps");
    }

    #[test]
    fn a_section_with_no_dispatcher_picked_says_what_is_missing() {
        let mut s = section(&["k8s"]);
        s.fields.form.dispatcher.clear();
        assert_eq!(s.problems(), vec!["pick a dispatcher".to_owned()]);
        assert!(s.dispatch_body().is_none(), "and posts nothing");
        assert!(ctrl(&mut s, 's').is_empty());
    }

    #[test]
    fn the_body_is_the_shared_one_and_carries_the_context_pack() {
        let mut s = section(&["k8s"]);
        type_text(&mut s, Field::Repo, "cctui");
        type_text(&mut s, Field::Ticket, "CCT-1102");
        type_text(&mut s, Field::Timeout, "60");
        type_text(&mut s, Field::PackUrl, "https://git/p.git");
        type_text(&mut s, Field::PackToken, "tok");

        let body = s.dispatch_body().expect("a body");
        assert_eq!(body["dispatcher"], "k8s");
        assert_eq!(body["timeout"], 60);
        assert_eq!(body["payload"]["repo"], "cctui");
        assert_eq!(body["payload"]["context"]["issue_id"], "CCT-1102");
        assert_eq!(body["payload"]["env"]["CONTEXT_PACK_URL"], "https://git/p.git");
        assert_eq!(body["payload"]["env"]["CONTEXT_PACK_TOKEN"], "tok");

        match ctrl(&mut s, 's').as_slice() {
            [Effect::Dispatch { body }] => assert_eq!(body["dispatcher"], "k8s"),
            other => panic!("expected one dispatch effect, got {}", other.len()),
        }
    }

    /// A dispatch is a different route: nothing of this section may land on a
    /// spawn request.
    #[test]
    fn the_section_writes_nothing_onto_a_spawn_request() {
        let mut s = section(&["k8s"]);
        type_text(&mut s, Field::Repo, "cctui");
        let mut request = cctui_clientcore::spawn::build_spawn_body(
            &SpawnFields::default(),
            None,
            std::collections::BTreeMap::new(),
            None,
            None,
        );
        let before = serde_json::to_value(&request).expect("serialises");
        s.apply(&mut request);
        assert_eq!(serde_json::to_value(&request).expect("serialises"), before);
    }

    /// The id is the idempotency key, so the same job twice is the same id.
    #[test]
    fn the_same_job_resubmits_onto_the_same_id() {
        let mut a = section(&["k8s"]);
        type_text(&mut a, Field::Ticket, "CCT-1");
        let first = dispatch_id(&a.fields);

        let mut b = section(&["k8s"]);
        type_text(&mut b, Field::Ticket, "CCT-1");
        assert_eq!(dispatch_id(&b.fields), first);
        type_text(&mut b, Field::Ticket, "9");
        assert_ne!(dispatch_id(&b.fields), first, "a different ticket is a different job");
    }

    #[test]
    fn an_idempotent_resubmit_says_the_session_was_already_running() {
        let mut app = app();
        app.sessions.push(session("s-disp", "worker", "active", "working"));
        reduce(
            &mut app,
            Action::Dispatch(DispatchAction::Submitted {
                session_id: "s-disp".to_owned(),
                existing: true,
            }),
        );
        assert!(app.toasts.latest().expect("a toast").text.contains("already running"));
        assert_eq!(app.selected_session_id().as_deref(), Some("s-disp"));
    }

    /// The form comes back per dispatcher and repo — but never the token.
    #[test]
    fn the_form_is_remembered_per_dispatcher_and_repo_without_the_token() {
        let mut s = section(&["k8s"]);
        type_text(&mut s, Field::Repo, "a");
        type_text(&mut s, Field::Ticket, "T-1");
        type_text(&mut s, Field::PackToken, "secret");
        s.remember_dispatch();

        type_text(&mut s, Field::Repo, "b");
        for _ in 0..3 {
            key(&mut s, Field::Ticket, KeyCode::Backspace);
        }
        type_text(&mut s, Field::Ticket, "T-2");
        s.remember_dispatch();

        // Clear the typed credential, so what comes back is only what was kept.
        for _ in 0..6 {
            key(&mut s, Field::PackToken, KeyCode::Backspace);
        }
        key(&mut s, Field::Repo, KeyCode::Backspace);
        assert_eq!(s.fields.form.repo, "a");
        assert_eq!(s.fields.form.ticket, "T-1", "repo a's ticket came back");
        assert!(s.fields.pack.token.is_empty(), "the credential was never kept");
    }

    /// A repo with no history leaves what is being typed alone.
    #[test]
    fn an_unseen_repo_does_not_wipe_the_form() {
        let mut s = section(&["k8s"]);
        type_text(&mut s, Field::Ticket, "T-9");
        type_text(&mut s, Field::Repo, "fresh");
        assert_eq!(s.fields.form.ticket, "T-9");
    }

    #[test]
    fn only_the_token_is_hidden_when_it_is_drawn() {
        assert!(Field::PackToken.secret());
        for other in Field::ORDER.iter().filter(|f| **f != Field::PackToken) {
            assert!(!other.secret(), "{other:?}");
        }
    }
}
