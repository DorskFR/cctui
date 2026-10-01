//! The fork dialog and resume, the two ways to carry a session's history
//! somewhere else.
//!
//! `Ctrl+f` opens the dialog; `:fork` stays the no-options whole-session fork
//! it has always been. Resume is the action the ended-state banner offers.

use cctui_proto::adapter::{ForkExtract, ForkMode};
use cctui_proto::api::{ForkRequest, SessionListItem};
use crossterm::event::{KeyCode, KeyEvent};

use super::action::Effect;
use super::state::{App, View};
use super::toast::Level;

/// What the fork takes with it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Extract {
    #[default]
    Full,
    /// Up to and including the line the cursor is on.
    UpTo,
    /// Only what came after it.
    After,
}

impl Extract {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "full history",
            Self::UpTo => "up to the cursor",
            Self::After => "after the cursor",
        }
    }

    const fn mode(self) -> Option<ForkMode> {
        match self {
            Self::Full => None,
            Self::UpTo => Some(ForkMode::UpTo),
            Self::After => Some(ForkMode::After),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Model,
    Effort,
    Extract,
    Name,
    Prompt,
}

impl Field {
    /// Extract is claude-only: codex cannot fork a slice, so the row is not
    /// offered rather than offered and refused.
    #[must_use]
    pub fn order(codex: bool) -> Vec<Self> {
        if codex {
            vec![Self::Model, Self::Effort, Self::Name, Self::Prompt]
        } else {
            vec![Self::Model, Self::Effort, Self::Extract, Self::Name, Self::Prompt]
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Extract => "From",
            Self::Name => "Name",
            Self::Prompt => "Prompt",
        }
    }
}

/// The open fork dialog.
#[derive(Debug, Clone)]
pub struct ForkForm {
    pub session_id: String,
    /// The session's harness, so the dialog knows to hide the extract row.
    pub codex: bool,
    pub models: Vec<cctui_proto::harness_models::ModelOption>,
    pub efforts: Vec<String>,
    pub model_index: usize,
    pub effort_index: usize,
    pub extract: Extract,
    pub name: String,
    pub prompt: String,
    pub focus: usize,
    /// The line the cursor was on when the dialog opened; a slice anchors here.
    pub anchor_message_id: Option<String>,
    pub loading: bool,
    /// Waiting on the server's `command_id`.
    pub submitting: bool,
}

impl ForkForm {
    #[must_use]
    pub fn fields(&self) -> Vec<Field> {
        Field::order(self.codex)
    }

    #[must_use]
    pub fn focused(&self) -> Field {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    #[must_use]
    pub fn model(&self) -> &str {
        self.models.get(self.model_index).map_or("", |m| m.v.as_str())
    }

    #[must_use]
    pub fn effort(&self) -> &str {
        self.efforts.get(self.effort_index).map_or("", String::as_str)
    }

    /// A slice needs something to anchor to; without one, only a full fork is
    /// on the table.
    #[must_use]
    pub const fn can_slice(&self) -> bool {
        !self.codex && self.anchor_message_id.is_some()
    }

    fn request(&self) -> ForkRequest {
        let extract = self.extract.mode().filter(|_| self.can_slice()).map(|mode| ForkExtract {
            mode,
            anchor_message_id: self.anchor_message_id.clone(),
            selected_message_ids: Vec::new(),
        });
        ForkRequest {
            model: nonempty(self.model()),
            effort: nonempty(self.effort()),
            prompt: nonempty(self.prompt.trim()),
            name: nonempty(self.name.trim()),
            extract,
        }
    }
}

fn nonempty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

pub enum ForkAction {
    Open,
    Close,
    ModelsLoaded(Box<cctui_proto::harness_models::HarnessModels>),
    FocusNext,
    FocusPrev,
    /// Move the picker or the radio the focused row holds.
    Cycle(isize),
    Key(KeyEvent),
    Submit,
    Resume,
    Resumed(Option<String>),
}

pub fn reduce_fork(app: &mut App, action: ForkAction) -> Vec<Effect> {
    match action {
        ForkAction::Open => open(app),
        ForkAction::Close => close(app),
        ForkAction::ModelsLoaded(models) => {
            models_loaded(app, *models);
            Vec::new()
        }
        ForkAction::FocusNext => {
            if let Some(form) = app.fork.as_mut() {
                let len = form.fields().len();
                form.focus = (form.focus + 1) % len;
            }
            Vec::new()
        }
        ForkAction::FocusPrev => {
            if let Some(form) = app.fork.as_mut() {
                let len = form.fields().len();
                form.focus = form.focus.checked_sub(1).unwrap_or(len - 1);
            }
            Vec::new()
        }
        ForkAction::Cycle(delta) => {
            cycle(app, delta);
            Vec::new()
        }
        ForkAction::Key(key) => {
            type_into(app, key);
            Vec::new()
        }
        ForkAction::Submit => submit(app),
        ForkAction::Resume => resume(app),
        ForkAction::Resumed(error) => {
            match error {
                Some(error) => app.toast(Level::Error, format!("resume failed: {error}")),
                None => {
                    app.toast(Level::Info, "resuming…");
                }
            }
            Vec::new()
        }
    }
}

fn open(app: &mut App) -> Vec<Effect> {
    let Some(session) = app.selected_session().cloned() else { return Vec::new() };
    let codex = is_codex(&session);
    let harness = session
        .adapter_id
        .as_ref()
        .map_or_else(|| "claude-code".to_owned(), |a| a.as_str().to_owned());
    let name = session
        .name
        .clone()
        .or_else(|| {
            session
                .metadata
                .get("project_name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .map_or_else(String::new, |base| format!("{base} (fork)"));
    app.fork = Some(ForkForm {
        session_id: session.id.clone(),
        codex,
        models: Vec::new(),
        efforts: Vec::new(),
        model_index: 0,
        effort_index: 0,
        extract: Extract::Full,
        name,
        prompt: String::new(),
        focus: 0,
        anchor_message_id: anchor_of(app),
        loading: true,
        submitting: false,
    });
    app.router.push(View::ForkDialog);
    vec![Effect::FetchHarnessModels {
        harness,
        machine_id: session.machine_id,
        model: session.model.unwrap_or_default(),
        want: super::action::ModelsFor::ForkDialog,
    }]
}

fn close(app: &mut App) -> Vec<Effect> {
    app.fork = None;
    if app.view() == View::ForkDialog {
        app.router.pop();
    }
    Vec::new()
}

/// The parent's model is always offered, even when the catalog has stopped
/// listing it: forking onto the model the parent ran is the common case.
fn models_loaded(app: &mut App, models: cctui_proto::harness_models::HarnessModels) {
    let parent_model = app.selected_session().and_then(|s| s.model.clone()).unwrap_or_default();
    let parent_effort = app.selected_session().and_then(|s| s.effort.clone()).unwrap_or_default();
    let Some(form) = app.fork.as_mut() else { return };
    let mut options = models.models;
    if !parent_model.is_empty() && !options.iter().any(|m| m.v == parent_model) {
        options.insert(
            0,
            cctui_proto::harness_models::ModelOption {
                v: parent_model.clone(),
                label: format!("{parent_model} (parent)"),
                hint: None,
                disabled: false,
            },
        );
    }
    form.model_index = options.iter().position(|m| m.v == parent_model).unwrap_or(0);
    form.effort_index = models.efforts.iter().position(|e| *e == parent_effort).unwrap_or(0);
    form.models = options;
    form.efforts = models.efforts;
    form.loading = false;
}

fn cycle(app: &mut App, delta: isize) {
    let Some(form) = app.fork.as_mut() else { return };
    match form.focused() {
        Field::Model => step(&mut form.model_index, form.models.len(), delta),
        Field::Effort => step(&mut form.effort_index, form.efforts.len(), delta),
        Field::Extract => {
            let options: &[Extract] = if form.anchor_message_id.is_some() {
                &[Extract::Full, Extract::UpTo, Extract::After]
            } else {
                &[Extract::Full]
            };
            let mut index = options.iter().position(|e| *e == form.extract).unwrap_or(0);
            step(&mut index, options.len(), delta);
            form.extract = options[index];
        }
        Field::Name | Field::Prompt => {}
    }
}

fn step(index: &mut usize, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let last = len - 1;
    *index = if delta < 0 {
        index.checked_sub(1).unwrap_or(last)
    } else if *index >= last {
        0
    } else {
        *index + 1
    };
}

fn type_into(app: &mut App, key: KeyEvent) {
    let Some(form) = app.fork.as_mut() else { return };
    let buffer = match form.focused() {
        Field::Name => &mut form.name,
        Field::Prompt => &mut form.prompt,
        _ => return,
    };
    match key.code {
        KeyCode::Char(c) => buffer.push(c),
        KeyCode::Backspace => {
            buffer.pop();
        }
        _ => {}
    }
}

fn submit(app: &mut App) -> Vec<Effect> {
    let Some(form) = app.fork.as_mut() else { return Vec::new() };
    if form.loading {
        return Vec::new();
    }
    form.submitting = true;
    let session_id = form.session_id.clone();
    let request = form.request();
    app.toast(Level::Info, "forking…");
    close(app);
    vec![Effect::Fork { session_id, request: Box::new(request) }]
}

/// Only an ended session can be resumed; a live one has nothing to revive.
fn resume(app: &mut App) -> Vec<Effect> {
    let Some(session) = app.selected_session() else { return Vec::new() };
    if !resumable(session) {
        app.toast(Level::Info, "this session is still running");
        return Vec::new();
    }
    let session_id = session.id.clone();
    vec![Effect::Resume { session_id }]
}

/// What the banner's ended state offers its action for.
#[must_use]
pub fn resumable(session: &SessionListItem) -> bool {
    session.end_reason.is_some() || session.status == cctui_proto::models::SessionStatus::Archived
}

fn is_codex(session: &SessionListItem) -> bool {
    session.adapter_id.as_ref().is_some_and(|a| a.as_str().starts_with("codex"))
}

/// The message the line cursor is on, which is what a slice anchors to.
fn anchor_of(app: &App) -> Option<String> {
    app.focused_line().and_then(|line| line.message_id.clone())
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Extract, Field, ForkAction, resumable};
    use crate::app::action::Effect;
    use crate::app::{Action, App, View, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        let mut s = session("s-a", "alpha", "active", "working");
        s.model = Some("opus".to_owned());
        s.effort = Some("high".to_owned());
        app.sessions = vec![s];
        app.update_aggregates();
        app
    }

    fn codex_app() -> App {
        let mut app = app();
        app.sessions[0].adapter_id = Some(cctui_proto::adapter::AdapterId::new("codex"));
        app
    }

    fn act(app: &mut App, action: ForkAction) -> Vec<Effect> {
        reduce(app, Action::Fork(action))
    }

    fn models() -> cctui_proto::harness_models::HarnessModels {
        use cctui_proto::harness_models::{HarnessModels, ModelOption};
        HarnessModels {
            harness: "claude-code".to_owned(),
            models: vec![
                ModelOption {
                    v: String::new(),
                    label: "Default".to_owned(),
                    hint: None,
                    disabled: false,
                },
                ModelOption {
                    v: "sonnet".to_owned(),
                    label: "Sonnet".to_owned(),
                    hint: None,
                    disabled: false,
                },
            ],
            efforts: vec![String::new(), "low".to_owned(), "high".to_owned()],
        }
    }

    fn loaded(app: &mut App) {
        act(app, ForkAction::Open);
        act(app, ForkAction::ModelsLoaded(Box::new(models())));
    }

    #[test]
    fn opening_asks_for_the_harness_models_and_prefills_the_name() {
        let mut app = app();
        match act(&mut app, ForkAction::Open).as_slice() {
            [Effect::FetchHarnessModels { harness, model, .. }] => {
                assert_eq!(harness, "claude-code");
                assert_eq!(model, "opus");
            }
            _ => panic!("expected a model list fetch"),
        }
        assert_eq!(app.view(), View::ForkDialog);
        let form = app.fork.as_ref().expect("a form");
        assert_eq!(form.name, "alpha (fork)");
        assert!(form.loading);
    }

    /// The parent's model is always listed, even once the catalog drops it.
    #[test]
    fn the_parents_model_is_offered_even_when_the_catalog_forgot_it() {
        let mut app = app();
        loaded(&mut app);
        let form = app.fork.as_ref().expect("a form");
        assert_eq!(form.model(), "opus");
        assert_eq!(form.models[0].label, "opus (parent)");
        assert_eq!(form.effort(), "high", "and its effort");
        assert!(!form.loading);
    }

    #[test]
    fn a_codex_session_is_not_offered_an_extract_row() {
        let mut app = codex_app();
        loaded(&mut app);
        let form = app.fork.as_ref().expect("a form");
        assert!(form.codex);
        assert!(!form.fields().contains(&Field::Extract));
        assert_eq!(Field::order(false).len(), Field::order(true).len() + 1);
    }

    #[test]
    fn a_slice_needs_an_anchor_to_hang_on() {
        let mut app = app();
        loaded(&mut app);
        let form = app.fork.as_ref().expect("a form");
        assert!(!form.can_slice(), "no line cursor, no slice");

        // With no anchor the radio cannot leave `full history`.
        app.fork.as_mut().expect("a form").focus =
            Field::order(false).iter().position(|f| *f == Field::Extract).expect("the row");
        act(&mut app, ForkAction::Cycle(1));
        assert_eq!(app.fork.as_ref().expect("a form").extract, Extract::Full);
    }

    #[test]
    fn the_pickers_and_the_radio_cycle_the_focused_row_only() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, ForkAction::Cycle(1));
        let form = app.fork.as_ref().expect("a form");
        assert_eq!(form.model(), "", "the Default entry");
        assert_eq!(form.effort(), "high", "the other row did not move");

        act(&mut app, ForkAction::FocusNext);
        act(&mut app, ForkAction::Cycle(-1));
        assert_eq!(app.fork.as_ref().expect("a form").effort(), "low");
    }

    #[test]
    fn typing_only_lands_in_the_text_rows() {
        let mut app = app();
        loaded(&mut app);
        let fields = Field::order(false);
        app.fork.as_mut().expect("a form").focus =
            fields.iter().position(|f| *f == Field::Prompt).expect("the row");
        for c in "try again".chars() {
            act(&mut app, ForkAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert_eq!(app.fork.as_ref().expect("a form").prompt, "try again");

        // A picker row swallows the keystroke rather than typing into a field.
        app.fork.as_mut().expect("a form").focus = 0;
        act(&mut app, ForkAction::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)));
        assert_eq!(app.fork.as_ref().expect("a form").prompt, "try again");
    }

    #[test]
    fn submitting_sends_what_was_filled_in_and_closes() {
        let mut app = app();
        loaded(&mut app);
        let fields = Field::order(false);
        app.fork.as_mut().expect("a form").focus =
            fields.iter().position(|f| *f == Field::Prompt).expect("the row");
        for c in "go".chars() {
            act(&mut app, ForkAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        match act(&mut app, ForkAction::Submit).as_slice() {
            [Effect::Fork { session_id, request }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(request.model.as_deref(), Some("opus"));
                assert_eq!(request.effort.as_deref(), Some("high"));
                assert_eq!(request.prompt.as_deref(), Some("go"));
                assert_eq!(request.name.as_deref(), Some("alpha (fork)"));
                assert!(request.extract.is_none(), "no anchor, no slice");
            }
            other => panic!("expected one fork effect, got {}", other.len()),
        }
        assert!(app.fork.is_none());
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn a_blank_row_is_left_for_the_parent_to_supply() {
        let mut app = app();
        app.sessions[0].model = None;
        app.sessions[0].effort = None;
        loaded(&mut app);
        app.fork.as_mut().expect("a form").name.clear();
        match act(&mut app, ForkAction::Submit).as_slice() {
            [Effect::Fork { request, .. }] => {
                assert!(request.model.is_none());
                assert!(request.effort.is_none());
                assert!(request.name.is_none());
                assert!(request.prompt.is_none());
            }
            _ => panic!("expected a fork effect"),
        }
    }

    #[test]
    fn escaping_the_dialog_forks_nothing() {
        let mut app = app();
        loaded(&mut app);
        assert!(act(&mut app, ForkAction::Close).is_empty());
        assert!(app.fork.is_none());
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn submitting_while_the_lists_load_waits() {
        let mut app = app();
        act(&mut app, ForkAction::Open);
        assert!(act(&mut app, ForkAction::Submit).is_empty());
        assert!(app.fork.is_some(), "the dialog stays open");
    }

    #[test]
    fn only_an_ended_or_archived_session_can_be_resumed() {
        let mut app = app();
        assert!(!resumable(&app.sessions[0]));
        assert!(act(&mut app, ForkAction::Resume).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("still running"));

        app.sessions[0].end_reason = Some(cctui_proto::models::SessionEndReason::Crashed);
        assert!(resumable(&app.sessions[0]));
        match act(&mut app, ForkAction::Resume).as_slice() {
            [Effect::Resume { session_id }] => assert_eq!(session_id, "s-a"),
            _ => panic!("expected a resume effect"),
        }

        app.sessions[0].end_reason = None;
        app.sessions[0].status = cctui_proto::models::SessionStatus::Archived;
        assert!(resumable(&app.sessions[0]), "an archived session too");
    }

    #[test]
    fn a_failed_resume_says_why() {
        let mut app = app();
        act(&mut app, ForkAction::Resumed(Some("machine offline".to_owned())));
        assert!(app.toasts.latest().expect("a toast").text.contains("machine offline"));
        act(&mut app, ForkAction::Resumed(None));
        assert_eq!(app.toasts.latest().expect("a toast").text, "resuming…");
    }

    #[test]
    fn every_extract_has_a_label_and_only_full_has_no_mode() {
        assert_eq!(Extract::Full.label(), "full history");
        assert!(Extract::UpTo.label().contains("up to"));
        assert!(Extract::After.label().contains("after"));
    }
}
