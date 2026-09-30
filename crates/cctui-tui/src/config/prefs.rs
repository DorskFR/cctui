//! Local preferences: the `[preferences]` table of `tui.toml`.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefsFile {
    pub timestamps: Option<bool>,
    pub compact_rows: Option<bool>,
    pub notifications: Option<bool>,
    pub ascii_glyphs: Option<bool>,
}

// The knobs are independent switches, not a state machine, so grouping them
// into enums would only obscure what each one does.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    /// Conversation timestamps shown from the start (`t` still toggles).
    pub timestamps: bool,
    /// One line per session row instead of the roomy two-line row.
    pub compact_rows: bool,
    /// Desktop/terminal notifications for sessions that want attention.
    pub notifications: bool,
    /// Plain ASCII instead of emoji, for a terminal or font without them.
    pub ascii_glyphs: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { timestamps: false, compact_rows: false, notifications: true, ascii_glyphs: false }
    }
}

impl Prefs {
    /// The server's settings are a weaker default than the local file: a knob
    /// written in `tui.toml` wins on this machine.
    pub const fn apply_server(&mut self, server: super::server::ServerPrefs) {
        if let Some(v) = server.compact_rows {
            self.compact_rows = v;
        }
        if let Some(v) = server.notifications {
            self.notifications = v;
        }
    }

    pub const fn apply_file(&mut self, file: PrefsFile) {
        if let Some(v) = file.timestamps {
            self.timestamps = v;
        }
        if let Some(v) = file.compact_rows {
            self.compact_rows = v;
        }
        if let Some(v) = file.notifications {
            self.notifications = v;
        }
        if let Some(v) = file.ascii_glyphs {
            self.ascii_glyphs = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Prefs, PrefsFile};

    #[test]
    fn the_local_file_wins_over_the_server() {
        let server = super::super::server::ServerPrefs {
            compact_rows: Some(true),
            notifications: Some(false),
            theme: None,
        };
        let mut prefs = Prefs::default();
        prefs.apply_server(server);
        prefs.apply_file(PrefsFile { notifications: Some(true), ..PrefsFile::default() });
        assert!(prefs.compact_rows);
        assert!(prefs.notifications);
    }

    #[test]
    fn an_absent_key_leaves_the_default_alone() {
        let mut prefs = Prefs::default();
        prefs.apply_file(PrefsFile { compact_rows: Some(true), ..PrefsFile::default() });
        assert!(prefs.compact_rows);
        assert!(prefs.notifications);
        assert!(!prefs.timestamps);
    }
}
