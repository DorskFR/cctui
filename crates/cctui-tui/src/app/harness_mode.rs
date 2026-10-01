//! The `claude-code` harness mode: which driver the daemon runs.
//!
//! A per-user server setting the web UI writes too, so a write patches the
//! settings blob the server last gave us rather than replacing it. Changing it
//! reconciles every owned daemon live.

use super::action::Effect;
use super::state::{App, View};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HarnessMode {
    #[default]
    Bg,
    Oneshot,
    Sdk,
}

/// In the order the picker offers them, the default first.
pub const MODES: [HarnessMode; 3] = [HarnessMode::Bg, HarnessMode::Oneshot, HarnessMode::Sdk];

impl HarnessMode {
    /// The wire value stored in `user_settings.data.harnessMode`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bg => "bg",
            Self::Oneshot => "oneshot",
            Self::Sdk => "sdk",
        }
    }

    /// One line per mode, from `docs/harness-modes.md`.
    pub const fn blurb(self) -> &'static str {
        match self {
            Self::Bg => {
                "claude daemon drives a background PTY worker. Richest transcript, \
                 and the only mode with a PTY fallback for prompts — but it depends \
                 on state.json and the on-disk JSONL being there to read."
            }
            Self::Oneshot => {
                "A fresh `claude -p --resume` child per turn. No state.json \
                 dependency and events come straight off the frames, but every \
                 turn respawns and an interrupt is a kill."
            }
            Self::Sdk => {
                "One persistent stream-json child. Structured events off stdout, \
                 interrupt leaves the child alive, and the only mode that can set \
                 a model in place instead of forking."
            }
        }
    }

    /// The default is what the server clamps an unknown or missing value to, so
    /// the TUI must agree rather than show a blank.
    pub fn parse(text: &str) -> Self {
        MODES.into_iter().find(|m| m.as_str() == text).unwrap_or_default()
    }

    #[must_use]
    pub fn from_settings(blob: &serde_json::Value) -> Self {
        blob.get("harnessMode")
            .and_then(serde_json::Value::as_str)
            .map_or_else(Self::default, Self::parse)
    }
}

/// The picker's own state; `None` when it is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picker {
    pub focused: usize,
    /// What the server currently has, so the picker can mark it.
    pub current: HarnessMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessModeAction {
    Open,
    Close,
    SelectNext,
    SelectPrev,
    /// Apply the focused row.
    Commit,
    /// Apply a mode named outright, from `:harness <mode>`.
    Set(HarnessMode),
}

pub fn reduce_harness_mode(app: &mut App, action: HarnessModeAction) -> Vec<Effect> {
    match action {
        HarnessModeAction::Open => {
            let current = HarnessMode::from_settings(&app.settings_blob);
            let focused = MODES.iter().position(|m| *m == current).unwrap_or(0);
            app.harness_picker = Some(Picker { focused, current });
            app.router.push(View::HarnessMode);
            Vec::new()
        }
        HarnessModeAction::Close => {
            close(app);
            Vec::new()
        }
        HarnessModeAction::SelectNext => {
            if let Some(picker) = app.harness_picker.as_mut() {
                picker.focused = (picker.focused + 1) % MODES.len();
            }
            Vec::new()
        }
        HarnessModeAction::SelectPrev => {
            if let Some(picker) = app.harness_picker.as_mut() {
                picker.focused = picker.focused.checked_sub(1).unwrap_or(MODES.len() - 1);
            }
            Vec::new()
        }
        HarnessModeAction::Commit => {
            let Some(picker) = app.harness_picker else { return Vec::new() };
            let Some(mode) = MODES.get(picker.focused).copied() else { return Vec::new() };
            close(app);
            set(app, mode)
        }
        HarnessModeAction::Set(mode) => set(app, mode),
    }
}

fn close(app: &mut App) {
    app.harness_picker = None;
    if app.view() == View::HarnessMode {
        app.router.pop();
    }
}

/// Writes the one key, keeping everything else the server last reported: the
/// web UI owns keys the TUI has never heard of and a replace would drop them.
fn set(app: &mut App, mode: HarnessMode) -> Vec<Effect> {
    if HarnessMode::from_settings(&app.settings_blob) == mode {
        app.toast(super::toast::Level::Info, format!("harness mode is already {}", mode.as_str()));
        return Vec::new();
    }
    let mut blob = match app.settings_blob.clone() {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    blob.insert("harnessMode".to_owned(), serde_json::Value::String(mode.as_str().to_owned()));
    app.settings_blob = serde_json::Value::Object(blob.clone());
    app.toast(
        super::toast::Level::Info,
        format!("harness mode {} — every connected daemon reconciles", mode.as_str()),
    );
    vec![Effect::SaveSettings { data: serde_json::Value::Object(blob) }]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{HarnessMode, HarnessModeAction, MODES, Picker};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};

    fn dispatch(app: &mut App, action: HarnessModeAction) -> Vec<Effect> {
        reduce(app, Action::HarnessMode(action))
    }

    fn saved(effects: &[Effect]) -> serde_json::Value {
        match effects {
            [Effect::SaveSettings { data }] => data.clone(),
            _ => panic!("expected one settings write, got {}", effects.len()),
        }
    }

    #[test]
    fn every_mode_the_server_accepts_has_a_wire_value_and_a_blurb() {
        for mode in MODES {
            assert!(!mode.as_str().is_empty());
            assert!(!mode.blurb().is_empty(), "{mode:?} has no explanation");
        }
        assert_eq!(
            MODES.map(HarnessMode::as_str).to_vec(),
            vec!["bg", "oneshot", "sdk"],
            "the wire values are the server's whitelist"
        );
    }

    /// The server clamps anything it does not recognise to `bg`; showing a
    /// different mode than the daemon will run would be a lie.
    #[test]
    fn an_unknown_or_missing_mode_reads_as_the_default() {
        assert_eq!(HarnessMode::from_settings(&json!({})), HarnessMode::Bg);
        assert_eq!(HarnessMode::from_settings(&serde_json::Value::Null), HarnessMode::Bg);
        assert_eq!(HarnessMode::from_settings(&json!({"harnessMode": "nope"})), HarnessMode::Bg);
        assert_eq!(HarnessMode::from_settings(&json!({"harnessMode": 7})), HarnessMode::Bg);
        assert_eq!(HarnessMode::from_settings(&json!({"harnessMode": "sdk"})), HarnessMode::Sdk);
    }

    #[test]
    fn a_write_keeps_every_other_settings_key() {
        let mut app = App::new();
        app.settings_blob = json!({
            "harnessMode": "bg",
            "sessionList": {"sort": "cost", "groupBy": "machine"},
            "somethingOnlyTheWebUiKnows": {"nested": [1, 2, 3]},
            "theme": "dark",
        });
        let effects = dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Sdk));
        let data = saved(&effects);
        assert_eq!(data["harnessMode"], json!("sdk"));
        assert_eq!(data["sessionList"], json!({"sort": "cost", "groupBy": "machine"}));
        assert_eq!(data["somethingOnlyTheWebUiKnows"], json!({"nested": [1, 2, 3]}));
        assert_eq!(data["theme"], json!("dark"));
        assert_eq!(app.settings_blob, data, "the local copy matches what went out");
    }

    #[test]
    fn a_write_onto_an_empty_blob_still_produces_an_object() {
        let mut app = App::new();
        let effects = dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Oneshot));
        assert_eq!(saved(&effects), json!({"harnessMode": "oneshot"}));
    }

    #[test]
    fn setting_the_mode_it_is_already_on_writes_nothing() {
        let mut app = App::new();
        app.settings_blob = json!({"harnessMode": "sdk"});
        assert!(dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Sdk)).is_empty());
    }

    #[test]
    fn the_picker_opens_focused_on_what_the_server_has() {
        let mut app = App::new();
        app.settings_blob = json!({"harnessMode": "sdk"});
        assert!(dispatch(&mut app, HarnessModeAction::Open).is_empty());
        assert_eq!(app.view(), View::HarnessMode);
        assert_eq!(
            app.harness_picker,
            Some(Picker { focused: 2, current: HarnessMode::Sdk }),
            "sdk is the third row"
        );
    }

    #[test]
    fn the_focus_wraps_both_ways() {
        let mut app = App::new();
        dispatch(&mut app, HarnessModeAction::Open);
        dispatch(&mut app, HarnessModeAction::SelectPrev);
        assert_eq!(app.harness_picker.expect("open").focused, 2);
        dispatch(&mut app, HarnessModeAction::SelectNext);
        assert_eq!(app.harness_picker.expect("open").focused, 0);
    }

    #[test]
    fn committing_applies_the_focused_row_and_closes() {
        let mut app = App::new();
        dispatch(&mut app, HarnessModeAction::Open);
        dispatch(&mut app, HarnessModeAction::SelectNext);
        let effects = dispatch(&mut app, HarnessModeAction::Commit);
        assert_eq!(saved(&effects)["harnessMode"], json!("oneshot"));
        assert!(app.harness_picker.is_none());
        assert_ne!(app.view(), View::HarnessMode);
    }

    #[test]
    fn closing_without_committing_changes_nothing() {
        let mut app = App::new();
        app.settings_blob = json!({"harnessMode": "bg"});
        dispatch(&mut app, HarnessModeAction::Open);
        dispatch(&mut app, HarnessModeAction::SelectNext);
        assert!(dispatch(&mut app, HarnessModeAction::Close).is_empty());
        assert_eq!(app.settings_blob, json!({"harnessMode": "bg"}));
        assert!(app.harness_picker.is_none());
    }
}
