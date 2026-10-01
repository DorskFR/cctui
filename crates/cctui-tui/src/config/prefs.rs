//! Local preferences: the `[preferences]` table of `tui.toml`.

use serde::Deserialize;

use crate::termnotify::Mode;

/// `notifications` takes `off` / `bell` / `osc`, and the booleans it used to
/// take before it had modes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum NotifyPref {
    Switch(bool),
    Named(String),
}

impl NotifyPref {
    fn mode(&self) -> Option<Mode> {
        match self {
            Self::Switch(true) => Some(Mode::Bell),
            Self::Switch(false) => Some(Mode::Off),
            Self::Named(text) => Mode::parse(text),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefsFile {
    pub timestamps: Option<bool>,
    pub compact_rows: Option<bool>,
    pub notifications: Option<NotifyPref>,
    pub ascii_glyphs: Option<bool>,
}

// The knobs are independent switches, not a state machine, so grouping them
// into enums would only obscure what each one does.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Prefs {
    /// Conversation timestamps shown from the start (`t` still toggles).
    pub timestamps: bool,
    /// One line per session row instead of the roomy two-line row.
    pub compact_rows: bool,
    /// What the terminal is told when a session starts waiting.
    pub notify: Mode,
    /// Plain ASCII instead of emoji, for a terminal or font without them.
    pub ascii_glyphs: bool,
}

impl Prefs {
    /// The server's settings are a weaker default than the local file: a knob
    /// written in `tui.toml` wins on this machine.
    pub const fn apply_server(&mut self, server: super::server::ServerPrefs) {
        if let Some(v) = server.compact_rows {
            self.compact_rows = v;
        }
        if let Some(v) = server.notifications {
            self.notify = if v { Mode::Bell } else { Mode::Off };
        }
    }

    pub fn apply_file(&mut self, file: &PrefsFile) {
        if let Some(v) = file.timestamps {
            self.timestamps = v;
        }
        if let Some(v) = file.compact_rows {
            self.compact_rows = v;
        }
        if let Some(mode) = file.notifications.as_ref().and_then(NotifyPref::mode) {
            self.notify = mode;
        }
        if let Some(v) = file.ascii_glyphs {
            self.ascii_glyphs = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, NotifyPref, Prefs, PrefsFile};

    fn named(text: &str) -> NotifyPref {
        NotifyPref::Named(text.to_owned())
    }

    #[test]
    fn the_local_file_wins_over_the_server() {
        let server = super::super::server::ServerPrefs {
            compact_rows: Some(true),
            notifications: Some(false),
            theme: None,
        };
        let mut prefs = Prefs::default();
        prefs.apply_server(server);
        assert_eq!(prefs.notify, Mode::Off);
        prefs.apply_file(&PrefsFile {
            notifications: Some(NotifyPref::Switch(true)),
            ..PrefsFile::default()
        });
        assert!(prefs.compact_rows);
        assert_eq!(prefs.notify, Mode::Bell);
    }

    #[test]
    fn an_absent_key_leaves_the_default_alone() {
        let mut prefs = Prefs::default();
        prefs.apply_file(&PrefsFile { compact_rows: Some(true), ..PrefsFile::default() });
        assert!(prefs.compact_rows);
        assert_eq!(prefs.notify, Mode::Bell);
        assert!(!prefs.timestamps);
    }

    /// The knob used to be a bool, so both spellings have to keep working.
    #[test]
    fn the_notify_knob_takes_a_mode_name_or_the_old_boolean() {
        for (pref, want) in [
            (Some(named("off")), Mode::Off),
            (Some(named("bell")), Mode::Bell),
            (Some(named("osc")), Mode::Osc),
            (Some(NotifyPref::Switch(false)), Mode::Off),
            (Some(NotifyPref::Switch(true)), Mode::Bell),
        ] {
            let mut prefs = Prefs::default();
            prefs.apply_file(&PrefsFile { notifications: pref.clone(), ..PrefsFile::default() });
            assert_eq!(prefs.notify, want, "{pref:?}");
        }
    }

    /// A typo must not silently turn notifications off.
    #[test]
    fn an_unknown_mode_name_leaves_the_previous_setting() {
        let mut prefs = Prefs::default();
        prefs.apply_file(&PrefsFile { notifications: Some(named("osc")), ..PrefsFile::default() });
        prefs.apply_file(&PrefsFile {
            notifications: Some(named("sparkles")),
            ..PrefsFile::default()
        });
        assert_eq!(prefs.notify, Mode::Osc);
    }

    #[test]
    fn the_toml_spelling_parses_both_ways() {
        let from_bool: PrefsFile =
            toml::from_str("notifications = false").expect("a boolean parses");
        assert_eq!(from_bool.notifications, Some(NotifyPref::Switch(false)));
        let from_name: PrefsFile =
            toml::from_str("notifications = \"osc\"").expect("a name parses");
        assert_eq!(from_name.notifications, Some(named("osc")));
    }
}
