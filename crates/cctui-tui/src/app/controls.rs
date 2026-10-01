//! Controlling a running session: interrupt with feedback, the model/effort
//! picker, and a whole-session fork.

use std::collections::HashMap;

use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::{ForkRequest, SessionListItem};
use cctui_proto::harness_models::HarnessModels;

use super::action::{Effect, ModelsFor};
use super::conversation;
use super::state::{App, View};
use super::toast::Level;

/// How long an armed key waits for its second press.
pub const CONFIRM_MS: i64 = 2_000;

/// A key that does something irreversible enough to want confirming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Interrupt,
    Fork,
}

impl Confirm {
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Interrupt => "press again to interrupt",
            Self::Fork => "press again to fork this session",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Armed {
    pub what: Confirm,
    pub session_id: String,
    pub expires_ms: i64,
}

/// Which column of the picker has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerColumn {
    Model,
    Effort,
}

#[derive(Debug, Clone)]
pub struct ModelPicker {
    pub session_id: String,
    pub harness: String,
    pub models: Vec<cctui_proto::harness_models::ModelOption>,
    pub efforts: Vec<String>,
    pub model_index: usize,
    pub effort_index: usize,
    pub column: PickerColumn,
    /// The lists are still on their way; the overlay says so.
    pub loading: bool,
}

impl ModelPicker {
    pub fn selected_model(&self) -> Option<&cctui_proto::harness_models::ModelOption> {
        self.models.get(self.model_index)
    }

    pub fn selected_effort(&self) -> Option<&str> {
        self.efforts.get(self.effort_index).map(String::as_str)
    }

    /// A disabled entry is shown with its reason but cannot be applied.
    pub fn can_apply(&self) -> bool {
        !self.loading && self.selected_model().is_some_and(|m| !m.disabled)
    }

    fn move_cursor(&mut self, delta: isize) {
        let (index, len) = match self.column {
            PickerColumn::Model => (&mut self.model_index, self.models.len()),
            PickerColumn::Effort => (&mut self.effort_index, self.efforts.len()),
        };
        if len == 0 {
            return;
        }
        let last = len - 1;
        *index = match delta {
            d if d < 0 => index.checked_sub(1).unwrap_or(last),
            _ => {
                if *index >= last {
                    0
                } else {
                    *index + 1
                }
            }
        };
    }
}

#[derive(Debug, Default)]
pub struct Controls {
    pub armed: Option<Armed>,
    /// Sessions whose interrupt is in flight, with the clock it left at.
    interrupting: HashMap<String, i64>,
    pub picker: Option<ModelPicker>,
    /// A fork whose new session has not reached the list yet.
    pub pending_jump: Option<String>,
}

impl Controls {
    pub fn is_interrupting(&self, session_id: &str) -> bool {
        self.interrupting.contains_key(session_id)
    }

    /// The armed confirmation for this session, if it has not timed out.
    pub fn armed_for(&self, session_id: &str, now_ms: i64) -> Option<Confirm> {
        self.armed
            .as_ref()
            .filter(|a| a.session_id == session_id && a.expires_ms > now_ms)
            .map(|a| a.what)
    }
}

pub enum ControlsAction {
    /// One press of the interrupt key: arms, or fires when already armed.
    Interrupt,
    InterruptFinished {
        session_id: String,
        error: Option<String>,
    },
    Forked(Option<String>),
    OpenModelPicker,
    ClosePicker,
    PickerMove(isize),
    PickerColumn(PickerColumn),
    ModelsLoaded(Box<HarnessModels>),
    PickerApply,
    ModelSet {
        model: String,
        effort: String,
    },
}

pub fn reduce_controls(app: &mut App, action: ControlsAction) -> Vec<Effect> {
    match action {
        ControlsAction::Interrupt => confirm(app, Confirm::Interrupt),
        ControlsAction::InterruptFinished { session_id, error } => {
            interrupt_finished(app, &session_id, error)
        }
        ControlsAction::Forked(session_id) => forked(app, session_id),
        ControlsAction::OpenModelPicker => open_picker(app),
        ControlsAction::ClosePicker => close_picker(app),
        ControlsAction::PickerMove(delta) => {
            if let Some(picker) = app.controls.picker.as_mut() {
                picker.move_cursor(delta);
            }
            Vec::new()
        }
        ControlsAction::PickerColumn(column) => {
            if let Some(picker) = app.controls.picker.as_mut() {
                picker.column = column;
            }
            Vec::new()
        }
        ControlsAction::ModelsLoaded(models) => {
            models_loaded(app, *models);
            Vec::new()
        }
        ControlsAction::PickerApply => apply_picker(app),
        ControlsAction::ModelSet { model, effort } => {
            model_set(app, &model, &effort);
            Vec::new()
        }
    }
}

/// The first press arms and says so; a second press inside [`CONFIRM_MS`]
/// goes through. Anything else — another session, a late second press —
/// arms afresh rather than acting on a stale intent.
fn confirm(app: &mut App, what: Confirm) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    if app.controls.armed_for(&session_id, app.clock_ms) == Some(what) {
        app.controls.armed = None;
        return fire(app, what, session_id);
    }
    let expires_ms = app.clock_ms + CONFIRM_MS;
    app.controls.armed = Some(Armed { what, session_id, expires_ms });
    Vec::new()
}

/// Fork now, no confirmation: typing `:fork` and pressing Enter is already
/// deliberate, where a single `Ctrl-f` is not.
pub fn fork_now(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    fire(app, Confirm::Fork, session_id)
}

fn fire(app: &mut App, what: Confirm, session_id: String) -> Vec<Effect> {
    match what {
        Confirm::Interrupt => {
            app.controls.interrupting.insert(session_id.clone(), app.clock_ms);
            vec![Effect::Interrupt { session_id }]
        }
        Confirm::Fork => {
            app.toast(Level::Info, "forking…");
            vec![Effect::Fork { session_id, request: Box::new(ForkRequest::default()) }]
        }
    }
}

fn interrupt_finished(app: &mut App, session_id: &str, error: Option<String>) -> Vec<Effect> {
    app.controls.interrupting.remove(session_id);
    match error {
        Some(error) => app.toast(Level::Error, format!("interrupt failed: {error}")),
        None => app.toast(Level::Info, "interrupted"),
    }
    Vec::new()
}

/// The fork's session is brand new, so it is not in the list yet: remember it
/// and let the refresh that follows open it.
fn forked(app: &mut App, session_id: Option<String>) -> Vec<Effect> {
    let Some(session_id) = session_id else {
        app.toast(Level::Warn, "fork accepted; the new session has not appeared yet");
        return vec![Effect::RefreshSessions];
    };
    app.controls.pending_jump = Some(session_id);
    vec![Effect::RefreshSessions]
}

/// Called after the session list changes: opens a forked session once it is
/// actually listed.
pub fn take_pending_jump(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.controls.pending_jump.clone() else { return Vec::new() };
    if !app.sessions.iter().any(|s| s.id == session_id) {
        return Vec::new();
    }
    app.controls.pending_jump = None;
    app.toast(Level::Info, "opened the fork");
    conversation::switch_to(app, session_id)
}

/// Changing the model in place is a codex-only control, exactly as the web UI
/// gates it; every other harness can only be given a model when it is spawned.
pub fn model_control_available(session: &SessionListItem) -> bool {
    is_codex(session) && session.end_reason.is_none()
}

fn is_codex(session: &SessionListItem) -> bool {
    harness_of(session).starts_with("codex")
}

/// The harness the picker asks the server about; codex is the only one whose
/// model can change in place, so it is also the only sensible fallback.
fn harness_of(session: &SessionListItem) -> String {
    session.adapter_id.as_ref().map_or_else(|| "codex".to_owned(), |a| a.as_str().to_owned())
}

fn open_picker(app: &mut App) -> Vec<Effect> {
    let Some(session) = app.selected_session().cloned() else { return Vec::new() };
    if !model_control_available(&session) {
        let why = if is_codex(&session) {
            "this session has ended"
        } else {
            "changing the model in place is codex-only"
        };
        app.toast(Level::Info, why);
        return Vec::new();
    }
    let harness = harness_of(&session);
    let model = session.model.clone().unwrap_or_default();
    app.controls.picker = Some(ModelPicker {
        session_id: session.id.clone(),
        harness: harness.clone(),
        models: Vec::new(),
        efforts: Vec::new(),
        model_index: 0,
        effort_index: 0,
        column: PickerColumn::Model,
        loading: true,
    });
    app.router.push(View::ModelPicker);
    vec![Effect::FetchHarnessModels {
        want: ModelsFor::RunningSession,
        harness,
        machine_id: session.machine_id,
        model,
    }]
}

fn close_picker(app: &mut App) -> Vec<Effect> {
    app.controls.picker = None;
    if app.view() == View::ModelPicker {
        app.router.pop();
    }
    Vec::new()
}

/// The cursors start on what the session already runs, so applying without
/// moving is a no-op rather than a surprise.
fn models_loaded(app: &mut App, models: HarnessModels) {
    let current_model = app.selected_session().and_then(|s| s.model.clone()).unwrap_or_default();
    let current_effort = app.selected_session().and_then(|s| s.effort.clone()).unwrap_or_default();
    let Some(picker) = app.controls.picker.as_mut() else { return };
    picker.model_index = models.models.iter().position(|m| m.v == current_model).unwrap_or(0);
    picker.effort_index = models.efforts.iter().position(|e| *e == current_effort).unwrap_or(0);
    picker.models = models.models;
    picker.efforts = models.efforts;
    picker.loading = false;
}

fn apply_picker(app: &mut App) -> Vec<Effect> {
    let Some(picker) = app.controls.picker.as_ref() else { return Vec::new() };
    if !picker.can_apply() {
        return Vec::new();
    }
    let session_id = picker.session_id.clone();
    let model = picker.selected_model().map(|m| m.v.clone()).unwrap_or_default();
    let effort = picker.selected_effort().unwrap_or_default().to_owned();
    close_picker(app);
    vec![Effect::SetModel { session_id, model, effort }]
}

fn model_set(app: &mut App, model: &str, effort: &str) {
    if let Some(session) =
        app.selected_session_id().and_then(|id| app.sessions.iter_mut().find(|s| s.id == id))
    {
        session.model = (!model.is_empty()).then(|| model.to_owned());
        session.effort = (!effort.is_empty()).then(|| effort.to_owned());
    }
    let shown = match (model.is_empty(), effort.is_empty()) {
        (true, true) => "harness default".to_owned(),
        (false, true) => model.to_owned(),
        (true, false) => format!("effort {effort}"),
        (false, false) => format!("{model} · {effort}"),
    };
    app.toast(Level::Info, format!("model set to {shown}"));
}

/// The header badge: the session's permission posture, in the domain's own
/// words rather than the TUI's.
pub fn permission_badge(session: &SessionListItem) -> Option<&'static str> {
    let raw = session.permission_mode.as_deref()?;
    PermissionMode::from_session_label(raw).map(mode_label)
}

const fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
        PermissionMode::Whip => "whip",
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::harness_models::{HarnessModels, ModelOption};

    use super::{CONFIRM_MS, Confirm, ControlsAction, PickerColumn, permission_badge};
    use crate::app::action::Effect;
    use crate::app::{Action, App, View, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        let mut codex = session("s-a", "alpha", "active", "working");
        codex.adapter_id = Some(cctui_proto::adapter::AdapterId::new("codex"));
        codex.model = Some("gpt-5.6-sol".to_owned());
        app.sessions = vec![codex, session("s-b", "beta", "active", "working")];
        app.update_aggregates();
        app
    }

    fn controls(app: &mut App, action: ControlsAction) -> Vec<Effect> {
        reduce(app, Action::Controls(action))
    }

    fn registered_session(id: &str) -> cctui_proto::models::Session {
        cctui_proto::models::Session {
            id: id.to_owned(),
            parent_id: None,
            account_id: None,
            machine_id: "orion".to_owned(),
            working_dir: "/home/dev/fork".to_owned(),
            status: cctui_proto::models::SessionStatus::Active,
            registered_at: chrono::DateTime::UNIX_EPOCH,
            last_heartbeat: chrono::DateTime::UNIX_EPOCH,
            metadata: serde_json::json!({ "project_name": "fork" }),
            adapter_id: None,
        }
    }

    fn models() -> HarnessModels {
        HarnessModels {
            harness: "codex".to_owned(),
            models: vec![
                ModelOption {
                    v: String::new(),
                    label: "Default".to_owned(),
                    hint: None,
                    disabled: false,
                },
                ModelOption {
                    v: "gpt-5.6-sol".to_owned(),
                    label: "Sol".to_owned(),
                    hint: None,
                    disabled: false,
                },
                ModelOption {
                    v: "gpt-6".to_owned(),
                    label: "Six".to_owned(),
                    hint: None,
                    disabled: true,
                },
            ],
            efforts: vec![String::new(), "low".to_owned(), "high".to_owned()],
        }
    }

    #[test]
    fn one_press_only_arms_the_interrupt() {
        let mut app = app();
        assert!(controls(&mut app, ControlsAction::Interrupt).is_empty());
        assert_eq!(app.controls.armed_for("s-a", app.clock_ms), Some(Confirm::Interrupt));
        assert!(!app.controls.is_interrupting("s-a"));
    }

    #[test]
    fn a_second_press_interrupts_and_shows_it_in_flight() {
        let mut app = app();
        controls(&mut app, ControlsAction::Interrupt);
        let effects = controls(&mut app, ControlsAction::Interrupt);
        assert!(
            matches!(effects.as_slice(), [Effect::Interrupt { session_id }] if session_id == "s-a")
        );
        assert!(app.controls.is_interrupting("s-a"));
        assert!(app.controls.armed.is_none());
    }

    #[test]
    fn a_late_second_press_only_re_arms() {
        let mut app = app();
        controls(&mut app, ControlsAction::Interrupt);
        app.clock_ms += CONFIRM_MS + 1;
        assert!(controls(&mut app, ControlsAction::Interrupt).is_empty());
        assert!(!app.controls.is_interrupting("s-a"));
        assert_eq!(app.controls.armed_for("s-a", app.clock_ms), Some(Confirm::Interrupt));
    }

    /// Arming in one session must not let the next keystroke fire in another.
    #[test]
    fn arming_does_not_carry_to_another_session() {
        let mut app = app();
        controls(&mut app, ControlsAction::Interrupt);
        reduce(&mut app, Action::SelectNext);
        assert!(controls(&mut app, ControlsAction::Interrupt).is_empty());
        assert!(!app.controls.is_interrupting("s-b"));
        assert_eq!(app.controls.armed_for("s-b", app.clock_ms), Some(Confirm::Interrupt));
    }

    #[test]
    fn the_result_clears_the_in_flight_state_and_toasts() {
        let mut app = app();
        controls(&mut app, ControlsAction::Interrupt);
        controls(&mut app, ControlsAction::Interrupt);
        controls(
            &mut app,
            ControlsAction::InterruptFinished { session_id: "s-a".to_owned(), error: None },
        );
        assert!(!app.controls.is_interrupting("s-a"));
        assert_eq!(app.toasts.latest().expect("a toast").text, "interrupted");

        controls(&mut app, ControlsAction::Interrupt);
        controls(&mut app, ControlsAction::Interrupt);
        controls(
            &mut app,
            ControlsAction::InterruptFinished {
                session_id: "s-a".to_owned(),
                error: Some("gone".to_owned()),
            },
        );
        assert!(!app.controls.is_interrupting("s-a"));
        assert!(app.toasts.latest().expect("a toast").text.contains("gone"));
    }

    /// `:fork` fires without a confirmation, and the fork opens once the
    /// session it made reaches the list.
    #[test]
    fn a_fork_opens_the_new_session_once_it_is_listed() {
        let mut app = app();
        let effects = super::fork_now(&mut app);
        assert!(
            matches!(effects.as_slice(), [Effect::Fork { session_id, .. }] if session_id == "s-a")
        );

        let effects = controls(&mut app, ControlsAction::Forked(Some("s-new".to_owned())));
        assert!(matches!(effects.as_slice(), [Effect::RefreshSessions]));
        assert!(super::take_pending_jump(&mut app).is_empty(), "not listed yet");

        app.sessions.push(session("s-new", "fork", "active", "working"));
        let effects = super::take_pending_jump(&mut app);
        assert!(effects.iter().any(|e| matches!(e, Effect::Subscribe { .. })));
        assert_eq!(app.selected_session_id().as_deref(), Some("s-new"));
        assert!(app.controls.pending_jump.is_none());
    }

    #[test]
    fn a_fork_that_registers_over_the_socket_opens_without_waiting() {
        let mut app = app();
        controls(&mut app, ControlsAction::Forked(Some("s-new".to_owned())));

        let effects =
            reduce(&mut app, Action::SessionRegistered(Box::new(registered_session("s-new"))));
        assert!(effects.iter().any(|e| matches!(e, Effect::Subscribe { .. })));
        assert_eq!(app.selected_session_id().as_deref(), Some("s-new"));
        assert!(app.controls.pending_jump.is_none());

        let listed = app.sessions.clone();
        assert!(
            !reduce(&mut app, Action::SessionsLoaded(listed))
                .iter()
                .any(|e| matches!(e, Effect::Subscribe { .. })),
            "the refresh that follows must not re-open it"
        );
    }

    #[test]
    fn the_picker_is_codex_only_and_refuses_an_ended_session() {
        let mut app = app();
        reduce(&mut app, Action::SelectNext);
        assert!(controls(&mut app, ControlsAction::OpenModelPicker).is_empty());
        assert!(app.controls.picker.is_none());
        assert!(app.toasts.latest().expect("a toast").text.contains("codex-only"));

        reduce(&mut app, Action::SelectPrev);
        app.sessions[0].end_reason = Some(cctui_proto::models::SessionEndReason::Completed);
        assert!(controls(&mut app, ControlsAction::OpenModelPicker).is_empty());
        assert!(app.controls.picker.is_none());
        assert!(app.toasts.latest().expect("a toast").text.contains("ended"));
    }

    #[test]
    fn opening_the_picker_asks_the_server_for_that_harness() {
        let mut app = app();
        let effects = controls(&mut app, ControlsAction::OpenModelPicker);
        match effects.as_slice() {
            [Effect::FetchHarnessModels { harness, model, .. }] => {
                assert_eq!(harness, "codex");
                assert_eq!(model, "gpt-5.6-sol");
            }
            _ => panic!("expected a model list fetch"),
        }
        assert_eq!(app.view(), View::ModelPicker);
        assert!(app.controls.picker.as_ref().expect("a picker").loading);
    }

    #[test]
    fn the_cursors_land_on_what_the_session_already_runs() {
        let mut app = app();
        controls(&mut app, ControlsAction::OpenModelPicker);
        controls(&mut app, ControlsAction::ModelsLoaded(Box::new(models())));
        let picker = app.controls.picker.as_ref().expect("a picker");
        assert!(!picker.loading);
        assert_eq!(picker.model_index, 1, "the running model");
        assert_eq!(picker.effort_index, 0, "the harness default");
    }

    #[test]
    fn the_cursor_wraps_within_its_own_column() {
        let mut app = app();
        controls(&mut app, ControlsAction::OpenModelPicker);
        controls(&mut app, ControlsAction::ModelsLoaded(Box::new(models())));
        controls(&mut app, ControlsAction::PickerMove(1));
        controls(&mut app, ControlsAction::PickerMove(1));
        assert_eq!(app.controls.picker.as_ref().expect("a picker").model_index, 0);
        controls(&mut app, ControlsAction::PickerMove(-1));
        assert_eq!(app.controls.picker.as_ref().expect("a picker").model_index, 2);

        controls(&mut app, ControlsAction::PickerColumn(PickerColumn::Effort));
        controls(&mut app, ControlsAction::PickerMove(1));
        let picker = app.controls.picker.as_ref().expect("a picker");
        assert_eq!(picker.effort_index, 1);
        assert_eq!(picker.model_index, 2, "the other column does not move");
    }

    #[test]
    fn a_disabled_model_cannot_be_applied() {
        let mut app = app();
        controls(&mut app, ControlsAction::OpenModelPicker);
        controls(&mut app, ControlsAction::ModelsLoaded(Box::new(models())));
        controls(&mut app, ControlsAction::PickerMove(1));
        assert_eq!(app.controls.picker.as_ref().expect("a picker").model_index, 2);
        assert!(controls(&mut app, ControlsAction::PickerApply).is_empty());
        assert!(app.controls.picker.is_some(), "the picker stays open");
    }

    #[test]
    fn applying_sends_both_dials_and_closes() {
        let mut app = app();
        controls(&mut app, ControlsAction::OpenModelPicker);
        controls(&mut app, ControlsAction::ModelsLoaded(Box::new(models())));
        controls(&mut app, ControlsAction::PickerColumn(PickerColumn::Effort));
        controls(&mut app, ControlsAction::PickerMove(1));
        let effects = controls(&mut app, ControlsAction::PickerApply);
        match effects.as_slice() {
            [Effect::SetModel { session_id, model, effort }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(model, "gpt-5.6-sol");
                assert_eq!(effort, "low");
            }
            _ => panic!("expected a set-model effect"),
        }
        assert!(app.controls.picker.is_none());
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn escaping_the_picker_changes_nothing() {
        let mut app = app();
        controls(&mut app, ControlsAction::OpenModelPicker);
        controls(&mut app, ControlsAction::ClosePicker);
        assert!(app.controls.picker.is_none());
        assert_eq!(app.view(), View::SessionList);
        assert_eq!(app.sessions[0].model.as_deref(), Some("gpt-5.6-sol"));
    }

    /// The row only changes once the server has taken the change.
    #[test]
    fn the_row_follows_the_server_not_the_keystroke() {
        let mut app = app();
        controls(
            &mut app,
            ControlsAction::ModelSet { model: "gpt-6".to_owned(), effort: "high".to_owned() },
        );
        assert_eq!(app.sessions[0].model.as_deref(), Some("gpt-6"));
        assert_eq!(app.sessions[0].effort.as_deref(), Some("high"));
        assert!(app.toasts.latest().expect("a toast").text.contains("gpt-6 · high"));

        controls(
            &mut app,
            ControlsAction::ModelSet { model: String::new(), effort: String::new() },
        );
        assert!(app.sessions[0].model.is_none());
        assert!(app.toasts.latest().expect("a toast").text.contains("harness default"));
    }

    #[test]
    fn the_permission_badge_reads_the_domains_own_labels() {
        let mut app = app();
        assert_eq!(permission_badge(&app.sessions[0]), None);
        for (raw, shown) in [
            ("ask", "ask"),
            ("default", "ask"),
            ("plan", "ask"),
            ("acceptEdits", "auto"),
            ("bypassPermissions", "yolo"),
            ("whip", "whip"),
        ] {
            app.sessions[0].permission_mode = Some(raw.to_owned());
            assert_eq!(permission_badge(&app.sessions[0]), Some(shown), "{raw}");
        }
        app.sessions[0].permission_mode = Some("nonsense".to_owned());
        assert_eq!(permission_badge(&app.sessions[0]), None);
    }
}
