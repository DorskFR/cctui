//! Canned prompts from the user's server settings, recalled into the composer.

use serde_json::Value;

use super::action::Effect;
use super::state::{App, View};
use super::toast::Level;

/// One entry of `data.macros.items`. Only the fields a composer recall needs;
/// the knobs that spawn a session belong to the spawn form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub adapter: String,
}

#[derive(Debug, Default)]
pub struct MacroState {
    pub items: Vec<Macro>,
    /// `data.macros.enabled`: the web UI hides the menu when it is off.
    pub enabled: bool,
    pub picker: Option<MacrosPicker>,
}

#[derive(Debug, Default)]
pub struct MacrosPicker {
    pub filter: String,
    pub selected: usize,
}

impl MacroState {
    /// Rows the picker shows, filtered case-insensitively on title and prompt.
    pub fn matches(&self, filter: &str) -> Vec<&Macro> {
        let needle = filter.trim().to_lowercase();
        self.items
            .iter()
            .filter(|mac| {
                needle.is_empty()
                    || mac.title.to_lowercase().contains(&needle)
                    || mac.prompt.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn selected(&self) -> Option<&Macro> {
        let picker = self.picker.as_ref()?;
        self.matches(&picker.filter).get(picker.selected).copied()
    }
}

/// Read `data.macros` out of the settings blob, dropping entries the web UI
/// would also drop: an item needs an id, a title and a prompt.
#[must_use]
pub fn from_settings(data: &Value) -> MacroState {
    let block = data.get("macros");
    let enabled = block.and_then(|m| m.get("enabled")).and_then(Value::as_bool).unwrap_or(false);
    let items = block
        .and_then(|m| m.get("items"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_macro).collect())
        .unwrap_or_default();
    MacroState { items, enabled, picker: None }
}

fn parse_macro(raw: &Value) -> Option<Macro> {
    let text = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let id = text("id");
    let title = text("title");
    let prompt = raw.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
    if id.is_empty() || title.is_empty() || prompt.trim().is_empty() {
        return None;
    }
    let adapter = text("adapter");
    let adapter = if adapter.is_empty() { "claude-code".to_owned() } else { adapter };
    Some(Macro { id, title, prompt, adapter })
}

#[derive(Debug, Clone, Copy)]
pub enum MacroAction {
    Open,
    Close,
    SelectNext,
    SelectPrev,
    Insert,
    FilterKey(crossterm::event::KeyEvent),
}

pub fn reduce_macros(app: &mut App, action: MacroAction) -> Vec<Effect> {
    match action {
        MacroAction::Open => {
            open(app);
            Vec::new()
        }
        MacroAction::Close => {
            close(app);
            Vec::new()
        }
        MacroAction::SelectNext => {
            move_selection(app, 1);
            Vec::new()
        }
        MacroAction::SelectPrev => {
            move_selection(app, -1);
            Vec::new()
        }
        MacroAction::Insert => insert(app),
        MacroAction::FilterKey(key) => {
            filter_key(app, key);
            Vec::new()
        }
    }
}

fn open(app: &mut App) {
    if app.macros.items.is_empty() {
        let why = if app.macros.enabled {
            "no macros defined — add them in the web UI's settings"
        } else {
            "macros are turned off in settings"
        };
        app.toast(Level::Info, why);
        return;
    }
    app.macros.picker = Some(MacrosPicker::default());
    app.router.push(View::Macros);
}

fn close(app: &mut App) {
    app.macros.picker = None;
    if app.view() == View::Macros {
        app.router.pop();
    }
}

fn move_selection(app: &mut App, delta: i32) {
    let count = app.macros.picker.as_ref().map_or(0, |p| app.macros.matches(&p.filter).len());
    let Some(picker) = app.macros.picker.as_mut() else { return };
    if count == 0 {
        picker.selected = 0;
        return;
    }
    let end = count - 1;
    picker.selected = if delta < 0 {
        picker.selected.checked_sub(1).unwrap_or(end)
    } else if picker.selected >= end {
        0
    } else {
        picker.selected + 1
    };
}

fn filter_key(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let Some(picker) = app.macros.picker.as_mut() else { return };
    match key.code {
        KeyCode::Char(c) => picker.filter.push(c),
        KeyCode::Backspace => {
            picker.filter.pop();
        }
        _ => return,
    }
    picker.selected = 0;
}

/// Put the macro's prompt in the composer at the caret and leave it open: a
/// canned prompt is a starting point, not a send.
fn insert(app: &mut App) -> Vec<Effect> {
    let Some(prompt) = app.macros.selected().map(|mac| mac.prompt.clone()) else {
        close(app);
        return Vec::new();
    };
    close(app);
    let text = app.message_input.lines().join("\n");
    let caret = super::drafts::caret_offset(app);
    let chars: Vec<char> = text.chars().collect();
    let head: String = chars[..caret.min(chars.len())].iter().collect();
    let tail: String = chars[caret.min(chars.len())..].iter().collect();
    let caret = head.chars().count() + prompt.chars().count();
    app.set_input_text_at(&format!("{head}{prompt}{tail}"), caret);
    app.input_active = true;
    super::drafts::on_input(app)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MacroAction, from_settings};
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn settings() -> serde_json::Value {
        json!({
            "macros": {
                "enabled": true,
                "items": [
                    { "id": "m1", "title": "  Triage  ", "prompt": "triage the inbox" },
                    { "id": "m2", "title": "Release", "prompt": "cut a release", "adapter": "codex" },
                    { "id": "", "title": "no id", "prompt": "x" },
                    { "id": "m4", "title": "", "prompt": "no title" },
                    { "id": "m5", "title": "no prompt", "prompt": "   " },
                    { "id": "m6", "title": "wrong types", "prompt": 7 },
                ]
            }
        })
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app.macros = from_settings(&settings());
        reduce(&mut app, Action::OpenSelectedConversation);
        app
    }

    fn composer(app: &App) -> String {
        app.message_input.lines().join("\n")
    }

    #[test]
    fn only_complete_entries_survive_the_settings_blob() {
        let state = from_settings(&settings());
        assert!(state.enabled);
        let titles: Vec<&str> = state.items.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["Triage", "Release"], "id, title and prompt are all required");
        assert_eq!(state.items[0].adapter, "claude-code", "the default harness");
        assert_eq!(state.items[1].adapter, "codex");
    }

    #[test]
    fn a_missing_or_disabled_block_reads_as_no_macros() {
        for blob in [json!({}), json!({ "macros": {} }), json!({ "macros": { "items": [] } })] {
            let state = from_settings(&blob);
            assert!(state.items.is_empty());
            assert!(!state.enabled);
        }
    }

    #[test]
    fn the_picker_filters_and_inserts_the_prompt_at_the_caret() {
        let mut app = app();
        app.input_active = true;
        reduce(&mut app, Action::Macros(MacroAction::Open));
        assert_eq!(app.view(), View::Macros);

        reduce(&mut app, Action::Macros(MacroAction::SelectNext));
        assert_eq!(app.macros.selected().expect("a macro").title, "Release");
        reduce(&mut app, Action::Macros(MacroAction::Insert));

        assert_eq!(composer(&app), "cut a release");
        assert_eq!(app.view(), View::Conversation, "inserting closes the picker");
        assert!(app.input_active);
    }

    #[test]
    fn a_macro_lands_where_the_caret_is() {
        let mut app = app();
        app.input_active = true;
        app.set_input_text_at("before after", 7);
        reduce(&mut app, Action::Macros(MacroAction::Open));
        reduce(&mut app, Action::Macros(MacroAction::Insert));
        assert_eq!(composer(&app), "before triage the inboxafter");
    }

    #[test]
    fn an_empty_macro_list_says_so_instead_of_opening() {
        let mut app = app();
        app.macros = from_settings(&json!({}));
        assert!(reduce(&mut app, Action::Macros(MacroAction::Open)).is_empty());
        assert_eq!(app.view(), View::Conversation);
        assert!(app.macros.picker.is_none());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn the_filter_narrows_the_list_and_the_selection_wraps() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app();
        reduce(&mut app, Action::Macros(MacroAction::Open));
        for c in "rel".chars() {
            reduce(
                &mut app,
                Action::Macros(MacroAction::FilterKey(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                ))),
            );
        }
        assert_eq!(app.macros.matches("rel").len(), 1);
        assert_eq!(app.macros.selected().expect("a macro").id, "m2");

        reduce(
            &mut app,
            Action::Macros(MacroAction::FilterKey(KeyEvent::new(
                KeyCode::Backspace,
                KeyModifiers::NONE,
            ))),
        );
        assert_eq!(app.macros.picker.as_ref().expect("picker").filter, "re");

        reduce(&mut app, Action::Macros(MacroAction::SelectPrev));
        assert_eq!(app.macros.selected().expect("a macro").id, "m2", "one match, no wrap away");

        reduce(&mut app, Action::Macros(MacroAction::Close));
        assert!(app.macros.picker.is_none());
        assert_eq!(composer(&app), "", "closing inserts nothing");
    }
}
