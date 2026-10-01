//! Draft sessions in the list: launch one, edit it, discard it, or start a new
//! spawn from any session's configuration.
//!
//! A draft is a session row the server holds without dispatching it. Its spawn
//! payload lives under `metadata.draft`; the env values it needs are never
//! stored, so a launch asks for them again.

use std::collections::BTreeMap;

use cctui_clientcore::drafts::{draft_env_keys, draft_label_ids, draft_payload};
use cctui_proto::api::SessionListItem;
use cctui_proto::models::SessionStatus;
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// The env a launch is collecting, one row per name the draft remembers.
#[derive(Debug)]
pub struct EnvPrompt {
    pub session_id: String,
    pub keys: Vec<String>,
    pub values: Vec<String>,
    pub at: usize,
}

impl EnvPrompt {
    #[must_use]
    pub fn key(&self) -> &str {
        self.keys.get(self.at).map_or("", String::as_str)
    }

    #[must_use]
    pub fn value(&self) -> &str {
        self.values.get(self.at).map_or("", String::as_str)
    }

    fn env(&self) -> BTreeMap<String, String> {
        self.keys
            .iter()
            .zip(&self.values)
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct SpawnDraftState {
    pub env_prompt: Option<EnvPrompt>,
    /// Pending delete, awaiting its confirm.
    pub confirm: Option<String>,
    /// The payload the dialog opens on next: an edited draft, a new spawn from
    /// a session's configuration, or a macro run.
    pub prefill: Option<Box<cctui_proto::api::SpawnRequest>>,
    /// The draft row the open dialog is editing, so its autosave replaces that
    /// row instead of making another.
    pub editing: Option<String>,
}

impl SpawnDraftState {
    /// The bottom line while a prompt is up, in the row-action strip's shape.
    #[must_use]
    pub fn strip(&self) -> Option<String> {
        if let Some(prompt) = &self.env_prompt {
            let at = prompt.at + 1;
            let of = prompt.keys.len();
            return Some(format!(
                " {}={}▏  ({at}/{of})  Tab next · Enter launch · Esc cancel",
                prompt.key(),
                prompt.value()
            ));
        }
        self.confirm.as_ref().map(|_| " discard this draft? y/N".to_owned())
    }
}

/// Keep the open dialog as a draft: the effects worker holds the request for
/// a quiet period, so a burst of typing is one save. Nothing is saved until
/// the spawn is addressable — a draft needs the machine and directory it would
/// run in.
pub fn autosave(app: &App) -> Vec<Effect> {
    let Some(request) = super::spawn::form_snapshot(app) else { return Vec::new() };
    if request.machine_id.is_empty() || request.working_dir.is_empty() {
        return Vec::new();
    }
    let env_keys = app
        .spawn
        .as_ref()
        .map(|form| {
            form.env
                .iter()
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, _)| k.trim().to_owned())
                .collect()
        })
        .unwrap_or_default();
    let draft = cctui_clientcore::spawn::draft_body(&request, env_keys, Vec::new());
    vec![Effect::AutosaveDraft {
        session_id: app.spawn_drafts.editing.clone(),
        request: Box::new(draft),
    }]
}

#[must_use]
pub fn is_draft(session: &SessionListItem) -> bool {
    session.status == SessionStatus::Draft
}

const fn metadata(session: &SessionListItem) -> &Value {
    &session.metadata
}

pub enum SpawnDraftAction {
    /// `L` on a draft row: launch it, asking for its env first.
    Launch,
    EnvKey(KeyEvent),
    EnvNext,
    EnvCommit,
    EnvCancel,
    Launched {
        session_id: String,
    },
    /// The row the first autosave minted, so later saves replace it.
    DraftCreated {
        session_id: String,
    },
    /// `X` on a draft row, behind a confirm.
    Discard,
    DiscardConfirm,
    DiscardCancel,
    Discarded {
        session_id: String,
    },
    /// `E` on a draft row: reopen the dialog on its stored payload.
    Edit,
    /// `N` on any row: a new spawn seeded from that session's configuration.
    NewFromConfig,
    /// Run the macro the picker has selected as a new session.
    RunSelectedMacro,
}

pub fn reduce_drafts(app: &mut App, action: SpawnDraftAction) -> Vec<Effect> {
    match action {
        SpawnDraftAction::Launch => launch(app),
        SpawnDraftAction::EnvKey(key) => {
            env_key(app, key);
            Vec::new()
        }
        SpawnDraftAction::EnvNext => {
            if let Some(prompt) = app.spawn_drafts.env_prompt.as_mut() {
                prompt.at = (prompt.at + 1) % prompt.keys.len().max(1);
            }
            Vec::new()
        }
        SpawnDraftAction::EnvCommit => env_commit(app),
        SpawnDraftAction::EnvCancel => {
            app.spawn_drafts.env_prompt = None;
            Vec::new()
        }
        SpawnDraftAction::DraftCreated { session_id } => {
            // A save that lands after the dialog closed has nothing to adopt it;
            // the row stays a draft the list can edit or discard.
            if app.spawn.is_none() {
                return Vec::new();
            }
            if app.spawn_drafts.editing.is_none() {
                app.spawn_drafts.editing = Some(session_id);
            }
            Vec::new()
        }
        SpawnDraftAction::Launched { session_id } => {
            // The row becomes a live session on the next refresh; dropping it
            // here would make the list flicker empty in between.
            app.toast(Level::Info, "launching the draft");
            app.spawn_drafts.editing.take_if(|id| *id == session_id);
            vec![Effect::RefreshSessions]
        }
        SpawnDraftAction::Discard => {
            let Some(session) = selected_draft(app) else {
                app.toast(Level::Info, "that row is not a draft");
                return Vec::new();
            };
            app.spawn_drafts.confirm = Some(session.id.clone());
            Vec::new()
        }
        SpawnDraftAction::DiscardConfirm => {
            let Some(session_id) = app.spawn_drafts.confirm.take() else { return Vec::new() };
            vec![Effect::DiscardDraftSession { session_id }]
        }
        SpawnDraftAction::DiscardCancel => {
            app.spawn_drafts.confirm = None;
            Vec::new()
        }
        SpawnDraftAction::Discarded { session_id } => {
            app.sessions.retain(|s| s.id != session_id);
            app.spawn_drafts.editing.take_if(|id| *id == session_id);
            app.update_aggregates();
            app.toast(Level::Info, "discarded the draft");
            Vec::new()
        }
        SpawnDraftAction::Edit => edit(app),
        SpawnDraftAction::NewFromConfig => new_from_config(app),
        SpawnDraftAction::RunSelectedMacro => run_macro(app),
    }
}

fn selected_draft(app: &App) -> Option<&SessionListItem> {
    app.selected_session().filter(|s| is_draft(s))
}

fn launch(app: &mut App) -> Vec<Effect> {
    let Some(session) = selected_draft(app) else {
        app.toast(Level::Info, "that row is not a draft");
        return Vec::new();
    };
    let session_id = session.id.clone();
    let keys = draft_env_keys(metadata(session));
    if keys.is_empty() {
        return vec![Effect::LaunchDraft { session_id, env: BTreeMap::new() }];
    }
    let values = vec![String::new(); keys.len()];
    app.spawn_drafts.env_prompt = Some(EnvPrompt { session_id, keys, values, at: 0 });
    Vec::new()
}

fn env_key(app: &mut App, key: KeyEvent) {
    let Some(prompt) = app.spawn_drafts.env_prompt.as_mut() else { return };
    let Some(value) = prompt.values.get_mut(prompt.at) else { return };
    match key.code {
        KeyCode::Char(c) => value.push(c),
        KeyCode::Backspace => {
            value.pop();
        }
        _ => {}
    }
}

fn env_commit(app: &mut App) -> Vec<Effect> {
    let Some(prompt) = app.spawn_drafts.env_prompt.take() else { return Vec::new() };
    let env = prompt.env();
    vec![Effect::LaunchDraft { session_id: prompt.session_id, env }]
}

/// The payload the dialog reopens on. Built from what the draft stored, so a
/// field the draft does not hold is left for the dialog's own defaults.
fn edit(app: &mut App) -> Vec<Effect> {
    let Some(session) = selected_draft(app) else {
        app.toast(Level::Info, "that row is not a draft");
        return Vec::new();
    };
    let mut request = request_of(session);
    request.env_keys = draft_env_keys(metadata(session));
    request.label_ids = draft_label_ids(metadata(session), &label_ids(session));
    let session_id = session.id.clone();
    app.spawn_drafts.editing = Some(session_id);
    app.spawn_drafts.prefill = Some(Box::new(request));
    Vec::new()
}

fn new_from_config(app: &mut App) -> Vec<Effect> {
    let Some(session) = app.selected_session() else { return Vec::new() };
    let mut request = request_of(session);
    // A new spawn starts from the configuration, not the old prompt or draft
    // row: a fresh launch writes its own.
    request.prompt = None;
    request.name = None;
    request.env_keys = Vec::new();
    app.spawn_drafts.editing = None;
    app.spawn_drafts.prefill = Some(Box::new(request));
    Vec::new()
}

fn run_macro(app: &mut App) -> Vec<Effect> {
    let Some(mac) = app.macros.selected().map(super::macros::Macro::spec) else {
        app.toast(Level::Info, "no macro selected");
        return Vec::new();
    };
    let problems = cctui_clientcore::macros::macro_problems(&mac);
    if !problems.is_empty() {
        let missing: Vec<&str> =
            problems.iter().map(|p| cctui_clientcore::macros::MacroProblem::as_str(*p)).collect();
        app.toast(Level::Warn, format!("“{}” needs a {}", mac.title, missing.join(", a ")));
        return Vec::new();
    }
    app.spawn_drafts.editing = None;
    app.spawn_drafts.prefill = Some(Box::new(cctui_clientcore::macros::macro_spawn_body(&mac)));
    Vec::new()
}

fn label_ids(session: &SessionListItem) -> Vec<String> {
    session.labels.iter().map(|l| l.id.clone()).collect()
}

/// A session's configuration as a spawn request: what the row itself says,
/// then whatever its draft payload adds.
fn request_of(session: &SessionListItem) -> cctui_proto::api::SpawnRequest {
    let draft = draft_payload(metadata(session));
    let text = |key: &str| draft.get(key).and_then(Value::as_str).map(str::to_owned);
    let adapter = text("adapter_id").or_else(|| session.adapter_id.clone().map(|a| a.0));
    cctui_proto::api::SpawnRequest {
        machine_id: text("machine_id").unwrap_or_else(|| session.machine_id.clone()),
        working_dir: text("working_dir").unwrap_or_else(|| session.working_dir.clone()),
        prompt: text("prompt"),
        prompt_name: None,
        name: text("name").or_else(|| session.name.clone()),
        adapter_id: adapter,
        permission_mode: text("permission_mode")
            .or_else(|| session.permission_mode.clone())
            .and_then(|mode| serde_json::from_value(Value::String(mode)).ok()),
        effort: text("effort").or_else(|| session.effort.clone()),
        model: text("model").or_else(|| session.model.clone()),
        service_tier: text("service_tier"),
        env: BTreeMap::new(),
        account: text("account").or_else(|| session.account_name.clone()),
        provider: text("provider"),
        no_account: draft.get("no_account").and_then(Value::as_bool).unwrap_or(false),
        auto_account: draft.get("auto_account").and_then(Value::as_bool).unwrap_or(false),
        pool: text("pool"),
        save_draft: false,
        auto_archive: false,
        env_keys: Vec::new(),
        attachment_names: Vec::new(),
        label_ids: label_ids(session),
        spawn_capability: None,
        relation: None,
        parent_session_id: None,
        context: None,
        profile_id: draft
            .get("profile_id")
            .and_then(Value::as_str)
            .and_then(|id| uuid::Uuid::parse_str(id).ok()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SpawnDraftAction, is_draft};
    use crate::app::action::Effect;
    use crate::app::state::App;
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn draft_row(id: &str, draft: &serde_json::Value) -> cctui_proto::api::SessionListItem {
        let mut row = session(id, "alpha", "draft", "working");
        row.status = cctui_proto::models::SessionStatus::Draft;
        row.metadata = json!({ "draft": draft });
        row
    }

    /// Draft rows are hidden until the Drafts section is on, so a test that
    /// acts on one turns it on exactly as the user does.
    fn app_with(rows: Vec<cctui_proto::api::SessionListItem>) -> App {
        let mut app = App::new();
        app.sessions = rows;
        if !app.list_shape.sections.has(crate::app::list_view::Section::Drafts) {
            app.list_shape.sections.toggle(crate::app::list_view::Section::Drafts);
        }
        app.update_aggregates();
        app
    }

    fn act(app: &mut App, action: SpawnDraftAction) -> Vec<Effect> {
        reduce(app, Action::SpawnDrafts(action))
    }

    #[test]
    fn typing_in_the_dialog_autosaves_it_as_a_draft_without_its_env() {
        let mut app = app_with(vec![session("s-a", "alpha", "active", "working")]);
        reduce(&mut app, Action::Spawn(crate::app::spawn::SpawnAction::Open));
        let form = app.spawn.as_mut().expect("the dialog");
        form.fields.machine_id.clear();
        form.fields.working_dir.clear();
        assert!(
            super::autosave(&app).is_empty(),
            "a spawn with nowhere to run is not a draft worth keeping"
        );

        let form = app.spawn.as_mut().expect("the dialog");
        form.fields.machine_id = "m-1".to_owned();
        form.fields.working_dir = "/w".to_owned();
        form.fields.prompt = "half a plan".to_owned();
        form.env = vec![("TOKEN".to_owned(), "secret".to_owned()), (" ".to_owned(), String::new())];

        match super::autosave(&app).as_slice() {
            [Effect::AutosaveDraft { session_id, request }] => {
                assert!(session_id.is_none(), "the first save mints the row");
                assert!(request.save_draft, "it is a draft, not a launch");
                assert_eq!(request.prompt.as_deref(), Some("half a plan"));
                assert!(request.env.is_empty(), "env values are never stored");
                assert_eq!(request.env_keys, vec!["TOKEN".to_owned()], "only their names");
            }
            _ => panic!("expected an autosave"),
        }

        app.spawn_drafts.editing = Some("d-9".to_owned());
        match super::autosave(&app).as_slice() {
            [Effect::AutosaveDraft { session_id, .. }] => {
                assert_eq!(session_id.as_deref(), Some("d-9"), "later saves replace that row");
            }
            _ => panic!("expected an autosave"),
        }
    }

    /// The dialog, open on a spawn that is addressable enough to autosave.
    fn app_with_open_dialog() -> App {
        let mut app = app_with(vec![session("s-a", "alpha", "active", "working")]);
        reduce(&mut app, Action::Spawn(crate::app::spawn::SpawnAction::Open));
        let form = app.spawn.as_mut().expect("the dialog");
        form.fields.machine_id = "m-1".to_owned();
        form.fields.working_dir = "/w".to_owned();
        form.fields.prompt = "half a plan".to_owned();
        app
    }

    fn autosave_target(app: &App) -> Option<String> {
        match super::autosave(app).as_slice() {
            [Effect::AutosaveDraft { session_id, .. }] => session_id.clone(),
            other => panic!("expected one autosave, got {}", other.len()),
        }
    }

    #[test]
    fn the_first_autosave_mints_a_row_and_every_later_one_replaces_it() {
        let mut app = app_with_open_dialog();
        assert_eq!(autosave_target(&app), None, "the first save has no row yet");

        act(&mut app, SpawnDraftAction::DraftCreated { session_id: "d-7".to_owned() });
        assert_eq!(
            autosave_target(&app),
            Some("d-7".to_owned()),
            "a pause must not mint a second draft session"
        );

        // A second reply (a save that was already in flight) must not move the
        // dialog onto a different row.
        act(&mut app, SpawnDraftAction::DraftCreated { session_id: "d-8".to_owned() });
        assert_eq!(autosave_target(&app), Some("d-7".to_owned()));
    }

    #[test]
    fn a_created_row_landing_after_the_dialog_closed_is_not_adopted() {
        let mut app = app_with_open_dialog();
        reduce(&mut app, Action::Spawn(crate::app::spawn::SpawnAction::Close));
        act(&mut app, SpawnDraftAction::DraftCreated { session_id: "d-7".to_owned() });
        assert_eq!(app.spawn_drafts.editing, None, "nothing is open to own that row");
    }

    #[test]
    fn editing_an_existing_draft_keeps_its_row_over_a_created_one() {
        let mut app = app_with(vec![draft_row("d-1", &json!({ "prompt": "half" }))]);
        act(&mut app, SpawnDraftAction::Edit);
        reduce(&mut app, Action::Spawn(crate::app::spawn::SpawnAction::Open));
        act(&mut app, SpawnDraftAction::DraftCreated { session_id: "d-9".to_owned() });
        assert_eq!(app.spawn_drafts.editing.as_deref(), Some("d-1"));
    }

    #[test]
    fn launching_the_dialog_discards_the_draft_row_it_autosaved() {
        let mut app = app_with_open_dialog();
        act(&mut app, SpawnDraftAction::DraftCreated { session_id: "d-7".to_owned() });

        let command_id = uuid::Uuid::new_v4();
        app.spawn.as_mut().expect("the dialog").launching = Some(command_id);
        let effects = reduce(
            &mut app,
            Action::Spawn(crate::app::spawn::SpawnAction::Launched {
                command_id,
                ok: true,
                error: None,
                session_id: Some("s-new".to_owned()),
            }),
        );
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::DiscardDraftSession { session_id } if session_id == "d-7"
            )),
            "the draft became a real session; the row must not be left behind"
        );
        assert_eq!(app.spawn_drafts.editing, None);
    }

    #[test]
    fn a_failed_launch_keeps_the_draft_row() {
        let mut app = app_with_open_dialog();
        act(&mut app, SpawnDraftAction::DraftCreated { session_id: "d-7".to_owned() });

        let command_id = uuid::Uuid::new_v4();
        app.spawn.as_mut().expect("the dialog").launching = Some(command_id);
        let effects = reduce(
            &mut app,
            Action::Spawn(crate::app::spawn::SpawnAction::Launched {
                command_id,
                ok: false,
                error: Some("nope".to_owned()),
                session_id: None,
            }),
        );
        assert!(!effects.iter().any(|e| matches!(e, Effect::DiscardDraftSession { .. })));
        assert_eq!(
            app.spawn_drafts.editing.as_deref(),
            Some("d-7"),
            "the work is still only in the draft"
        );
    }

    #[test]
    fn a_draft_without_env_launches_straight_away() {
        let mut app = app_with(vec![draft_row("d-1", &json!({ "prompt": "do it" }))]);
        assert!(is_draft(app.selected_session().expect("a row")));
        match act(&mut app, SpawnDraftAction::Launch).as_slice() {
            [Effect::LaunchDraft { session_id, env }] => {
                assert_eq!(session_id, "d-1");
                assert!(env.is_empty());
            }
            _ => panic!("expected a launch"),
        }
    }

    #[test]
    fn a_draft_with_env_names_asks_for_the_values_first() {
        let mut app = app_with(vec![draft_row("d-1", &json!({ "env_keys": ["TOKEN", "HOST"] }))]);
        assert!(act(&mut app, SpawnDraftAction::Launch).is_empty(), "nothing is sent yet");
        let prompt = app.spawn_drafts.env_prompt.as_ref().expect("an env prompt");
        assert_eq!(prompt.keys, vec!["TOKEN".to_owned(), "HOST".to_owned()]);
        assert_eq!(prompt.key(), "TOKEN");

        type_env(&mut app, "abc");
        act(&mut app, SpawnDraftAction::EnvNext);
        type_env(&mut app, "box");
        let effects = act(&mut app, SpawnDraftAction::EnvCommit);
        match effects.as_slice() {
            [Effect::LaunchDraft { session_id, env }] => {
                assert_eq!(session_id, "d-1");
                assert_eq!(env.get("TOKEN").map(String::as_str), Some("abc"));
                assert_eq!(env.get("HOST").map(String::as_str), Some("box"));
            }
            _ => panic!("expected a launch with the entered env"),
        }
        assert!(app.spawn_drafts.env_prompt.is_none());
    }

    fn type_env(app: &mut App, text: &str) {
        for c in text.chars() {
            act(
                app,
                SpawnDraftAction::EnvKey(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
        }
    }

    #[test]
    fn an_env_value_left_blank_is_not_sent() {
        let mut app = app_with(vec![draft_row("d-1", &json!({ "env_keys": ["TOKEN", "HOST"] }))]);
        act(&mut app, SpawnDraftAction::Launch);
        type_env(&mut app, "abc");
        match act(&mut app, SpawnDraftAction::EnvCommit).as_slice() {
            [Effect::LaunchDraft { env, .. }] => {
                assert_eq!(env.len(), 1, "only what was filled in");
                assert!(env.contains_key("TOKEN"));
            }
            _ => panic!("expected a launch"),
        }
    }

    #[test]
    fn escaping_the_env_prompt_launches_nothing() {
        let mut app = app_with(vec![draft_row("d-1", &json!({ "env_keys": ["TOKEN"] }))]);
        act(&mut app, SpawnDraftAction::Launch);
        assert!(act(&mut app, SpawnDraftAction::EnvCancel).is_empty());
        assert!(app.spawn_drafts.env_prompt.is_none());
    }

    #[test]
    fn launch_and_discard_refuse_a_live_row() {
        let mut app = app_with(vec![session("s-a", "alpha", "active", "working")]);
        assert!(act(&mut app, SpawnDraftAction::Launch).is_empty());
        assert!(act(&mut app, SpawnDraftAction::Discard).is_empty());
        assert!(app.spawn_drafts.confirm.is_none());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn discard_waits_for_a_confirm_and_drops_the_row() {
        let mut app = app_with(vec![draft_row("d-1", &json!({}))]);
        act(&mut app, SpawnDraftAction::Discard);
        assert_eq!(app.spawn_drafts.confirm.as_deref(), Some("d-1"));
        act(&mut app, SpawnDraftAction::DiscardCancel);
        assert!(act(&mut app, SpawnDraftAction::DiscardConfirm).is_empty());

        act(&mut app, SpawnDraftAction::Discard);
        match act(&mut app, SpawnDraftAction::DiscardConfirm).as_slice() {
            [Effect::DiscardDraftSession { session_id }] => assert_eq!(session_id, "d-1"),
            _ => panic!("expected a discard"),
        }
        act(&mut app, SpawnDraftAction::Discarded { session_id: "d-1".to_owned() });
        assert!(app.sessions.is_empty(), "the row goes");
    }

    #[test]
    fn editing_a_draft_prefills_the_dialog_from_its_payload() {
        let mut app = app_with(vec![draft_row(
            "d-1",
            &json!({
                "machine_id": "m-9",
                "working_dir": "/w/elsewhere",
                "prompt": "half a plan",
                "adapter_id": "codex",
                "model": "gpt-5.6",
                "effort": "high",
                "env_keys": ["TOKEN"],
                "label_ids": ["l1"],
            }),
        )]);
        assert!(act(&mut app, SpawnDraftAction::Edit).is_empty());
        assert_eq!(app.spawn_drafts.editing.as_deref(), Some("d-1"), "the autosave replaces it");
        let prefill = app.spawn_drafts.prefill.take().expect("a prefill");
        assert_eq!(prefill.machine_id, "m-9");
        assert_eq!(prefill.working_dir, "/w/elsewhere");
        assert_eq!(prefill.prompt.as_deref(), Some("half a plan"));
        assert_eq!(prefill.adapter_id.as_deref(), Some("codex"));
        assert_eq!(prefill.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(prefill.effort.as_deref(), Some("high"));
        assert_eq!(prefill.env_keys, vec!["TOKEN".to_owned()], "the names, never the values");
        assert!(prefill.env.is_empty());
        assert_eq!(prefill.label_ids, vec!["l1".to_owned()], "a launched draft keeps its labels");
        assert!(app.spawn_drafts.prefill.take().is_none(), "the prefill is taken once");
    }

    #[test]
    fn a_new_spawn_from_a_session_takes_the_config_and_not_the_prompt() {
        let mut app = app_with(vec![session("s-a", "alpha", "active", "working")]);
        act(&mut app, SpawnDraftAction::NewFromConfig);
        let prefill = app.spawn_drafts.prefill.take().expect("a prefill");
        assert_eq!(prefill.machine_id, "orion");
        assert_eq!(prefill.working_dir, "/home/dev/alpha");
        assert_eq!(prefill.prompt, None, "a new spawn writes its own prompt");
        assert_eq!(prefill.name, None);
        assert!(!prefill.save_draft);
        assert_eq!(app.spawn_drafts.editing, None, "it is a new row, not that one");
    }

    #[test]
    fn running_a_macro_prefills_a_whole_spawn() {
        let mut app = app_with(vec![session("s-a", "alpha", "active", "working")]);
        app.macros = crate::app::macros::from_settings(&json!({
            "macros": {
                "enabled": true,
                "items": [{
                    "id": "m1", "title": "Triage", "prompt": "triage the inbox",
                    "adapter": "codex", "machine_id": "m-1", "working_dir": "/w",
                    "model": "gpt-5.6", "effort": "high",
                }],
            }
        }));
        reduce(&mut app, Action::Macros(crate::app::macros::MacroAction::Open));
        assert!(act(&mut app, SpawnDraftAction::RunSelectedMacro).is_empty());
        let prefill = app.spawn_drafts.prefill.take().expect("a prefill");
        assert_eq!(prefill.machine_id, "m-1");
        assert_eq!(prefill.prompt.as_deref(), Some("triage the inbox"));
        assert!(prefill.auto_archive, "a macro session files itself away");
    }

    #[test]
    fn a_macro_missing_its_target_says_what_it_needs() {
        let mut app = app_with(vec![session("s-a", "alpha", "active", "working")]);
        app.macros = crate::app::macros::from_settings(&json!({
            "macros": {
                "enabled": true,
                "items": [{ "id": "m1", "title": "Triage", "prompt": "do it" }],
            }
        }));
        reduce(&mut app, Action::Macros(crate::app::macros::MacroAction::Open));
        assert!(act(&mut app, SpawnDraftAction::RunSelectedMacro).is_empty());
        assert!(app.spawn_drafts.prefill.is_none());
        let toast = app.toasts.latest().expect("a toast");
        assert!(toast.text.contains("machine"), "{}", toast.text);
        assert!(toast.text.contains("cwd"), "{}", toast.text);
    }
}
