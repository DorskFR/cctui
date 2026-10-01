//! Labels to attach once the session registers.

use cctui_clientcore::spawn::SpawnFields;
use cctui_proto::api::Label;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};

use super::SpawnSection;
use crate::app::action::Effect;
use crate::theme;

#[derive(Debug, Default)]
pub struct LabelsSection {
    /// The catalog, in the order `/labels` returned it.
    pub labels: Vec<Label>,
    /// Ids picked, in the order the request carries them.
    pub picked: Vec<String>,
    pub cursor: usize,
}

impl LabelsSection {
    #[must_use]
    pub fn is_picked(&self, id: &str) -> bool {
        self.picked.iter().any(|p| p == id)
    }

    pub fn toggle_at_cursor(&mut self) {
        let Some(label) = self.labels.get(self.cursor) else { return };
        if let Some(at) = self.picked.iter().position(|p| *p == label.id) {
            self.picked.remove(at);
        } else {
            self.picked.push(label.id.clone());
        }
    }

    fn own_lines(&self, focused: Option<usize>) -> Vec<Line<'static>> {
        let marker = if focused.is_some() { "›" } else { " " };
        let mut spans = vec![Span::styled(format!(" {marker} "), theme::section_title())];
        if self.labels.is_empty() {
            spans.push(Span::styled("no labels yet", theme::dim()));
        }
        for (i, label) in self.labels.iter().enumerate() {
            let box_ = if self.is_picked(&label.id) { "[x]" } else { "[ ]" };
            let style = if focused.is_some() && i == self.cursor {
                theme::selected()
            } else if self.is_picked(&label.id) {
                theme::hue_style(cctui_clientcore::labels::label_hue(&label.name, &label.color))
            } else {
                theme::dim()
            };
            spans.push(Span::styled(format!("{box_} {}  ", label.name), style));
        }
        if focused.is_some() && !self.labels.is_empty() {
            spans.push(Span::styled("(space toggle)", theme::dim()));
        }
        vec![Line::from(spans)]
    }

    fn own_handle(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle_at_cursor(),
            KeyCode::Char('l') | KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.labels.len().saturating_sub(1));
            }
            KeyCode::Char('h') | KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            _ => {}
        }
        Vec::new()
    }
}

impl SpawnSection for LabelsSection {
    fn title(&self) -> &'static str {
        "Labels"
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

    /// The picks go into the fields, not the request: the shared builder reads
    /// them from there, and a profile or a draft seeds the same place. A label
    /// the catalog has lost is dropped rather than re-attached.
    fn handle(&mut self, _row: usize, key: KeyEvent, fields: &mut SpawnFields) -> Vec<Effect> {
        if self.picked.is_empty() && !fields.labels.is_empty() {
            self.picked = fields
                .labels
                .iter()
                .filter(|id| self.labels.iter().any(|l| &l.id == *id))
                .cloned()
                .collect();
        }
        let effects = self.own_handle(key);
        fields.labels.clone_from(&self.picked);
        effects
    }

    fn apply(&self, _request: &mut cctui_proto::api::SpawnRequest) {}

    fn receive(&mut self, data: &super::SpawnData) {
        self.labels.clone_from(&data.labels);
    }
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::spawn::SpawnFields;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{LabelsSection, SpawnSection};

    fn label(id: &str) -> cctui_proto::api::Label {
        cctui_proto::api::Label { id: id.to_owned(), name: id.to_owned(), color: String::new() }
    }

    fn section() -> LabelsSection {
        LabelsSection {
            labels: vec![label("cct"), label("infra"), label("review")],
            ..LabelsSection::default()
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn space_toggles_the_label_under_the_cursor() {
        let mut fields = SpawnFields::default();
        let mut s = section();
        s.handle(0, key(KeyCode::Char(' ')), &mut fields);
        s.handle(0, key(KeyCode::Char('l')), &mut fields);
        s.handle(0, key(KeyCode::Char('l')), &mut fields);
        s.handle(0, key(KeyCode::Char(' ')), &mut fields);
        assert_eq!(fields.labels, ["cct", "review"], "the fields carry the picks");

        s.handle(0, key(KeyCode::Char(' ')), &mut fields);
        assert_eq!(fields.labels, ["cct"], "a second press takes it back off");
    }

    #[test]
    fn the_cursor_stops_at_both_ends() {
        let mut fields = SpawnFields::default();
        let mut s = section();
        for _ in 0..5 {
            s.handle(0, key(KeyCode::Char('l')), &mut fields);
        }
        assert_eq!(s.cursor, 2);
        for _ in 0..5 {
            s.handle(0, key(KeyCode::Char('h')), &mut fields);
        }
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn a_label_the_catalog_no_longer_has_is_dropped_from_the_seed() {
        let mut fields = SpawnFields {
            labels: vec!["review".to_owned(), "gone".to_owned()],
            ..SpawnFields::default()
        };
        let mut s = section();
        s.handle(0, key(KeyCode::Right), &mut fields);
        assert_eq!(s.picked, ["review"], "a deleted label cannot be re-attached");
        assert_eq!(fields.labels, ["review"]);
    }

    #[test]
    fn an_empty_catalog_toggles_nothing() {
        let mut fields = SpawnFields::default();
        let mut s = LabelsSection::default();
        s.handle(0, key(KeyCode::Char(' ')), &mut fields);
        assert!(fields.labels.is_empty());
    }
}
