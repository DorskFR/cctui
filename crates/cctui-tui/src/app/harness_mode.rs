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
            let current =
                app.settings_blob.as_ref().map_or(HarnessMode::Bg, HarnessMode::from_settings);
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

/// Writes the one key it owns. The "already set" shortcut needs a settings row
/// we have actually read: with none, the write goes ahead and the merge decides.
fn set(app: &mut App, mode: HarnessMode) -> Vec<Effect> {
    let known = app.settings_blob.as_ref().map(HarnessMode::from_settings);
    if known == Some(mode) {
        app.toast(super::toast::Level::Info, format!("harness mode is already {}", mode.as_str()));
        return Vec::new();
    }
    app.toast(
        super::toast::Level::Info,
        format!("harness mode {} — every connected daemon reconciles", mode.as_str()),
    );
    vec![super::settings_write::save(serde_json::json!({"harnessMode": mode.as_str()}))]
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
            [Effect::SaveSettings { patch }] => patch.clone(),
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

    /// The write names only the key this view owns; keeping the rest is the
    /// merge's job, against a read taken at write time.
    #[test]
    fn a_write_carries_only_the_key_it_owns() {
        let mut app = App::new();
        app.settings_blob = Some(json!({
            "harnessMode": "bg",
            "sessionList": {"sort": "cost", "groupBy": "machine"},
            "somethingOnlyTheWebUiKnows": {"nested": [1, 2, 3]},
            "theme": "dark",
        }));
        let effects = dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Sdk));
        assert_eq!(saved(&effects), json!({"harnessMode": "sdk"}));
        assert_eq!(
            app.settings_blob.as_ref().expect("known")["harnessMode"],
            json!("bg"),
            "the cached row still says what the server holds until it confirms"
        );
    }

    /// The picker reads the cached row, so showing the new mode before the
    /// server stored it would survive a refused write as a lie.
    #[test]
    fn the_mode_shown_only_moves_once_the_server_confirms() {
        let mut app = App::new();
        app.settings_blob = Some(json!({"harnessMode": "bg", "theme": "dark"}));
        dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Sdk));

        dispatch(&mut app, HarnessModeAction::Open);
        assert_eq!(
            app.harness_picker.expect("open").current,
            HarnessMode::Bg,
            "still bg: nothing was stored yet"
        );
        dispatch(&mut app, HarnessModeAction::Close);

        crate::app::reduce(
            &mut app,
            crate::app::Action::SettingsSaved(Box::new(
                json!({"harnessMode": "sdk", "theme": "dark"}),
            )),
        );
        dispatch(&mut app, HarnessModeAction::Open);
        assert_eq!(app.harness_picker.expect("open").current, HarnessMode::Sdk);
    }

    /// The F3 case: the startup read failed, so there is no row to merge onto.
    /// The write must still be a patch of one key — never a replacement built
    /// from an empty map, which would drop every web-UI setting.
    #[test]
    fn a_write_with_no_settings_row_read_is_still_only_a_patch() {
        let mut app = App::new();
        assert_eq!(app.settings_blob, None, "nothing has been read");
        let effects = dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Oneshot));
        assert_eq!(saved(&effects), json!({"harnessMode": "oneshot"}));
        assert_eq!(app.settings_blob, None, "an unread row stays unread");
    }

    #[test]
    fn setting_the_mode_it_is_already_on_writes_nothing() {
        let mut app = App::new();
        app.settings_blob = Some(json!({"harnessMode": "sdk"}));
        assert!(dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Sdk)).is_empty());
    }

    /// With no row read, "already on it" is a guess: the mode may well differ
    /// server-side, so the write goes ahead and the merge resolves it.
    #[test]
    fn an_unread_row_does_not_shortcut_the_default_mode() {
        let mut app = App::new();
        let effects = dispatch(&mut app, HarnessModeAction::Set(HarnessMode::Bg));
        assert_eq!(saved(&effects), json!({"harnessMode": "bg"}));
    }

    #[test]
    fn the_picker_opens_focused_on_what_the_server_has() {
        let mut app = App::new();
        app.settings_blob = Some(json!({"harnessMode": "sdk"}));
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
        app.settings_blob = Some(json!({"harnessMode": "bg"}));
        dispatch(&mut app, HarnessModeAction::Open);
        dispatch(&mut app, HarnessModeAction::SelectNext);
        assert!(dispatch(&mut app, HarnessModeAction::Close).is_empty());
        assert_eq!(app.settings_blob, Some(json!({"harnessMode": "bg"})));
        assert!(app.harness_picker.is_none());
    }
}
