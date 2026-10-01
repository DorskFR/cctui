//! Env secrets for the spawned worker.
//!
//! Values live here and nowhere else: they are not written to the draft, the
//! `tui.toml`, the UI state file or any log, and [`EnvSection`]'s own `Debug`
//! masks them so a stray trace cannot print one.

use cctui_clientcore::spawn::SpawnFields;
use cctui_clientcore::spawn_accounts::env_key_valid;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};

use super::SpawnSection;
use crate::app::action::Effect;
use crate::theme;

#[derive(Default, Clone, PartialEq, Eq)]
pub struct EnvRow {
    pub key: String,
    pub value: String,
}

impl std::fmt::Debug for EnvRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvRow").field("key", &self.key).field("value", &"<redacted>").finish()
    }
}

/// Which half of a row the keyboard is editing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    #[default]
    Key,
    Value,
}

#[derive(Debug, Default)]
pub struct EnvSection {
    pub rows: Vec<EnvRow>,
    pub cursor: usize,
    pub field: Field,
}

impl EnvSection {
    pub fn add_row(&mut self) {
        self.rows.push(EnvRow::default());
        self.cursor = self.rows.len() - 1;
        self.field = Field::Key;
    }

    pub fn delete_row(&mut self) {
        if self.cursor < self.rows.len() {
            self.rows.remove(self.cursor);
            self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        }
    }

    /// Keys that fail the shell-variable shape; the server rejects them, so the
    /// dialog says so before the spawn.
    #[must_use]
    pub fn bad_keys(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| r.key.trim())
            .filter(|k| !k.is_empty() && !env_key_valid(k))
            .map(str::to_owned)
            .collect()
    }

    #[must_use]
    pub fn own_problems(&self) -> Vec<String> {
        let bad = self.bad_keys();
        if bad.is_empty() {
            return Vec::new();
        }
        vec![format!("env names must match ^[A-Z_][A-Z0-9_]*$: {}", bad.join(", "))]
    }

    /// Complete rows only: a half-typed row carries nothing.
    fn fill_env(&self, req: &mut cctui_proto::api::SpawnRequest) {
        for row in &self.rows {
            let key = row.key.trim();
            if key.is_empty() || row.value.is_empty() || !env_key_valid(key) {
                continue;
            }
            req.env.insert(key.to_owned(), row.value.clone());
        }
    }

    fn own_lines(&self, focused: Option<usize>) -> Vec<Line<'static>> {
        let marker = if focused.is_some() { "›" } else { " " };
        let mut out = vec![Line::from(vec![
            Span::styled(format!(" {marker} "), theme::section_title()),
            Span::styled(
                if self.rows.is_empty() {
                    "none  (a add)".to_owned()
                } else {
                    format!("{} set  (a add · d delete)", self.rows.len())
                },
                theme::dim(),
            ),
        ])];
        for (i, row) in self.rows.iter().enumerate() {
            let on = focused.is_some() && i == self.cursor;
            let key_style =
                if on && self.field == Field::Key { theme::selected() } else { theme::dim() };
            let value_style =
                if on && self.field == Field::Value { theme::selected() } else { theme::dim() };
            out.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(format!("{:<24}", row.key), key_style),
                Span::raw(" = "),
                Span::styled(mask(&row.value), value_style),
            ]));
        }
        for problem in self.own_problems() {
            out.push(Line::from(Span::styled(format!("      ! {problem}"), theme::error())));
        }
        out
    }

    fn own_handle(&mut self, key: KeyEvent) -> Vec<Effect> {
        let editing = self.rows.get_mut(self.cursor);
        match key.code {
            KeyCode::Tab => {
                self.field = if self.field == Field::Key { Field::Value } else { Field::Key }
            }
            KeyCode::Down | KeyCode::Enter => {
                self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1));
            }
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Backspace => {
                if let Some(row) = editing {
                    let buffer =
                        if self.field == Field::Key { &mut row.key } else { &mut row.value };
                    buffer.pop();
                }
            }
            KeyCode::Char(c) => {
                let Some(row) = editing else {
                    if c == 'a' {
                        self.add_row();
                    }
                    return Vec::new();
                };
                match self.field {
                    Field::Key => row.key.push(c.to_ascii_uppercase()),
                    Field::Value => row.value.push(c),
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// `a` and `d` are the section's own keys, claimed before the row editor
    /// sees them, so they cannot be typed into a value.
    pub fn handle_command(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('a') if self.field == Field::Key => {
                self.add_row();
                true
            }
            KeyCode::Char('d') if self.field == Field::Key => {
                self.delete_row();
                true
            }
            _ => false,
        }
    }
}

impl SpawnSection for EnvSection {
    fn title(&self) -> &'static str {
        "Env"
    }

    fn rows(&self, _fields: &SpawnFields) -> usize {
        1
    }

    fn lines(
        &self,
        focused: Option<usize>,
        width: u16,
        _fields: &SpawnFields,
    ) -> Vec<Line<'static>> {
        super::clamp_rows(self.own_lines(focused), width)
    }

    fn handle(&mut self, _row: usize, key: KeyEvent, _fields: &mut SpawnFields) -> Vec<Effect> {
        if self.handle_command(key) {
            return Vec::new();
        }
        self.own_handle(key)
    }

    /// The values never reach the fields, so they cannot reach a draft: the
    /// request carries them, in memory, and nothing else does.
    fn apply(&self, request: &mut cctui_proto::api::SpawnRequest) {
        self.fill_env(request);
    }

    fn problems(&self) -> Vec<String> {
        self.own_problems()
    }
}

fn mask(value: &str) -> String {
    "*".repeat(value.chars().count().min(12))
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::spawn::SpawnFields;

    use super::{EnvSection, Field, SpawnSection, mask};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn request() -> cctui_proto::api::SpawnRequest {
        serde_json::from_value(serde_json::json!({"machine_id": "m", "working_dir": "/w"}))
            .expect("a request")
    }

    fn typed(section: &mut EnvSection, text: &str) {
        let mut fields = SpawnFields::default();
        for c in text.chars() {
            section.handle(0, key(KeyCode::Char(c)), &mut fields);
        }
    }

    fn filled() -> EnvSection {
        let mut s = EnvSection::default();
        s.add_row();
        typed(&mut s, "gh_token");
        s.handle(0, key(KeyCode::Tab), &mut SpawnFields::default());
        typed(&mut s, "ghp_secret");
        s
    }

    #[test]
    fn a_row_takes_an_upper_cased_name_and_a_verbatim_value() {
        let s = filled();
        assert_eq!(s.rows[0].key, "GH_TOKEN", "names are shell vars, so typing is folded up");
        assert_eq!(s.rows[0].value, "ghp_secret");

        let mut req = request();
        s.fill_env(&mut req);
        assert_eq!(req.env.get("GH_TOKEN").map(String::as_str), Some("ghp_secret"));
    }

    #[test]
    fn the_value_never_renders_in_the_clear() {
        let s = filled();
        let rendered: String = s
            .own_lines(Some(0))
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.to_string()))
            .collect();
        assert!(rendered.contains("GH_TOKEN"));
        assert!(!rendered.contains("ghp_secret"), "{rendered}");
        assert!(rendered.contains("**********"), "{rendered}");
        assert_eq!(mask(""), "");
        assert_eq!(mask("0123456789abcdef").chars().count(), 12, "the mask reveals no length");
    }

    #[test]
    fn debug_output_redacts_the_value() {
        let s = filled();
        let shown = format!("{:?}", s.rows[0]);
        assert!(!shown.contains("ghp_secret"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(!format!("{s:?}").contains("ghp_secret"));
    }

    #[test]
    fn a_half_typed_row_carries_nothing() {
        let mut s = EnvSection::default();
        s.add_row();
        typed(&mut s, "KEY_ONLY");
        let mut req = request();
        s.fill_env(&mut req);
        assert!(req.env.is_empty(), "a value-less row is not sent");
        assert!(s.own_problems().is_empty(), "and it is not an error either");
    }

    #[test]
    fn a_name_the_server_would_reject_is_flagged_and_not_sent() {
        let mut s = EnvSection::default();
        s.add_row();
        s.rows[0].key = "1BAD".to_owned();
        s.rows[0].value = "x".to_owned();
        assert_eq!(s.bad_keys(), ["1BAD"]);
        assert!(s.own_problems()[0].contains("^[A-Z_][A-Z0-9_]*$"));

        let mut req = request();
        s.fill_env(&mut req);
        assert!(req.env.is_empty());
    }

    #[test]
    fn add_and_delete_are_claimed_before_the_row_editor() {
        let mut s = EnvSection::default();
        assert!(s.handle_command(key(KeyCode::Char('a'))));
        assert_eq!(s.rows.len(), 1);
        assert!(s.handle_command(key(KeyCode::Char('a'))));
        assert_eq!(s.rows.len(), 2);
        assert!(s.handle_command(key(KeyCode::Char('d'))));
        assert_eq!(s.rows.len(), 1);

        s.field = Field::Value;
        assert!(!s.handle_command(key(KeyCode::Char('a'))), "a value may contain an 'a'");
        typed(&mut s, "pa55");
        assert_eq!(s.rows[0].value, "pa55");
    }

    #[test]
    fn deleting_with_no_rows_is_harmless() {
        let mut s = EnvSection::default();
        s.delete_row();
        assert!(s.rows.is_empty());
    }
}
