//! The core section: machine, dir, name, harness, model, effort, tier, mode,
//! prompt. Every other section is registered after this one.

use cctui_proto::api::SpawnRequest;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};

use cctui_clientcore::spawn::SpawnFields;

use super::SpawnSection;
use crate::app::action::Effect;
use crate::theme;

/// Rows the core section always shows, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Machine,
    Dir,
    Name,
    Harness,
    Model,
    Effort,
    ServiceTier,
    Mode,
    Prompt,
}

/// Which rendered line the Dir row is, so the view can slot the git badge and
/// the dropdown under it. Dir comes before Mode, the only row that adds a line
/// of its own, so the row index and the line index agree.
#[must_use]
pub fn dir_line_index(adapter: &str) -> usize {
    rows_for(adapter).iter().position(|r| *r == Row::Dir).unwrap_or(0)
}

/// Which rendered line the Model row is, for the same reason as
/// [`dir_line_index`]: Model also comes before Mode.
#[must_use]
pub fn model_line_index(adapter: &str) -> usize {
    rows_for(adapter).iter().position(|r| *r == Row::Model).unwrap_or(0)
}

/// Service tier is codex-only, so the row list depends on the harness.
#[must_use]
pub fn rows_for(adapter: &str) -> Vec<Row> {
    let mut rows = vec![Row::Machine, Row::Dir, Row::Name, Row::Harness, Row::Model, Row::Effort];
    if adapter == "codex" {
        rows.push(Row::ServiceTier);
    }
    rows.push(Row::Mode);
    rows.push(Row::Prompt);
    rows
}

/// What each mode does, shown under the picker so the choice is not a guess.
pub const MODES: &[(&str, &str, &str)] = &[
    ("", "default", "the account's own setting"),
    ("ask", "ask", "asks before each tool"),
    ("auto", "auto", "approves reads, asks to write"),
    ("yolo", "yolo", "skips prompts and sandbox"),
    ("whip", "whip", "yolo, and keeps going unprompted"),
];

#[must_use]
pub fn mode_hint(mode: &str) -> &'static str {
    MODES.iter().find(|(v, _, _)| *v == mode).map_or("", |(_, _, hint)| *hint)
}

/// Steps a radio-style choice, clamping at both ends rather than wrapping: a
/// left/right key should not jump from the safest mode to the most dangerous.
#[must_use]
pub fn step_choice(values: &[&str], current: &str, delta: i32) -> String {
    let at = values.iter().position(|v| *v == current).unwrap_or(0);
    let next = if delta < 0 {
        at.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (at + delta as usize).min(values.len().saturating_sub(1))
    };
    values.get(next).copied().unwrap_or_default().to_owned()
}

/// Holds only the lists its pickers step through; every value it shows lives in
/// the form's `fields`.
#[derive(Debug, Default)]
pub struct CoreSection {
    pub options: Options,
}

impl SpawnSection for CoreSection {
    fn title(&self) -> &'static str {
        "New session"
    }

    fn rows(&self, fields: &SpawnFields) -> usize {
        rows_for(&fields.adapter_id).len()
    }

    fn lines(
        &self,
        focused: Option<usize>,
        width: u16,
        fields: &SpawnFields,
    ) -> Vec<Line<'static>> {
        let rows = rows_for(&fields.adapter_id);
        let mut out = Vec::with_capacity(rows.len() + 1);
        for (index, row) in rows.iter().enumerate() {
            out.push(row_line(fields, *row, focused == Some(index), width));
            if *row == Row::Mode {
                let hint: String = format!("           {}", mode_hint(&fields.permission_mode))
                    .chars()
                    .take(usize::from(width))
                    .collect();
                out.push(Line::from(Span::styled(hint, theme::dim())));
            }
        }
        out
    }

    fn set_options(&mut self, options: Options) {
        self.options = options;
    }

    fn handle(&mut self, row: usize, key: KeyEvent, fields: &mut SpawnFields) -> Vec<Effect> {
        let rows = rows_for(&fields.adapter_id);
        if let Some(row) = rows.get(row).copied() {
            edit(fields, row, key, &self.options);
        }
        Vec::new()
    }

    fn apply(&self, _request: &mut SpawnRequest) {
        // The core fields reach the request through the shared builder, which
        // reads `fields` directly; there is nothing to add on top.
    }

    fn problems(&self) -> Vec<String> {
        Vec::new()
    }
}

const fn label(row: Row) -> &'static str {
    match row {
        Row::Machine => "Machine",
        Row::Dir => "Dir",
        Row::Name => "Name",
        Row::Harness => "Harness",
        Row::Model => "Model",
        Row::Effort => "Effort",
        Row::ServiceTier => "Tier",
        Row::Mode => "Mode",
        Row::Prompt => "Prompt",
    }
}

fn value(f: &SpawnFields, row: Row) -> String {
    let codex = f.adapter_id == "codex";
    match row {
        Row::Machine => f.machine_id.clone(),
        Row::Dir => f.working_dir.clone(),
        Row::Name => f.name.clone(),
        Row::Harness => f.adapter_id.clone(),
        Row::Model => {
            if codex {
                f.model_codex.clone()
            } else {
                f.model_claude.clone()
            }
        }
        Row::Effort => {
            if codex {
                f.effort_codex.clone()
            } else {
                f.effort_claude.clone()
            }
        }
        Row::ServiceTier => f.service_tier.clone(),
        Row::Mode => {
            let mode = &f.permission_mode;
            MODES
                .iter()
                .find(|(v, _, _)| v == mode)
                .map_or_else(|| mode.clone(), |(_, label, _)| (*label).to_owned())
        }
        Row::Prompt => f.prompt.lines().next().unwrap_or_default().to_owned(),
    }
}

fn row_line(f: &SpawnFields, row: Row, focused: bool, width: u16) -> Line<'static> {
    let value = value(f, row);
    let shown = if value.is_empty() { "—".to_owned() } else { value };
    let budget = usize::from(width).saturating_sub(12);
    let shown: String = shown.chars().take(budget.max(1)).collect();
    let marker = if focused { "❯ " } else { "  " };
    let style = if focused { theme::selected() } else { theme::dim() };
    Line::from(vec![
        Span::styled(marker, style),
        Span::styled(format!("{:<9}", label(row)), theme::dim()),
        Span::styled(shown, if focused { theme::bold() } else { theme::dim() }),
    ])
}

/// Text rows take characters; choice rows step with left/right.
/// The lists the Model and Effort rows step through, derived by the form from
/// the machine's live catalog.
#[derive(Debug, Default)]
pub struct Options {
    pub models: Vec<cctui_proto::harness_models::ModelOption>,
    pub efforts: Vec<String>,
}

fn edit(f: &mut SpawnFields, row: Row, key: KeyEvent, options: &Options) {
    let codex = f.adapter_id == "codex";
    let delta = match key.code {
        KeyCode::Left => -1,
        KeyCode::Right => 1,
        _ => 0,
    };
    match row {
        Row::Harness if delta != 0 => {
            f.adapter_id = step_choice(&["claude-code", "codex"], &f.adapter_id.clone(), delta);
        }
        Row::Mode if delta != 0 => {
            let values: Vec<&str> = MODES.iter().map(|(v, _, _)| *v).collect();
            f.permission_mode = step_choice(&values, &f.permission_mode.clone(), delta);
        }
        Row::Machine => type_into(&mut f.machine_id, key),
        Row::Dir => type_into(&mut f.working_dir, key),
        Row::Name => type_into(&mut f.name, key),
        // Left/Right steps the catalog, skipping a gated entry so it cannot be
        // picked; typing still works, because every picker also takes a
        // free-text id.
        Row::Model if delta != 0 && !options.models.is_empty() => {
            let pickable: Vec<&str> =
                options.models.iter().filter(|o| !o.disabled).map(|o| o.v.as_str()).collect();
            let current = if codex { f.model_codex.clone() } else { f.model_claude.clone() };
            let next = step_choice(&pickable, &current, delta);
            if codex {
                f.model_codex = next;
            } else {
                f.model_claude = next;
            }
        }
        Row::Effort if delta != 0 && !options.efforts.is_empty() => {
            let levels: Vec<&str> = options.efforts.iter().map(String::as_str).collect();
            let current = if codex { f.effort_codex.clone() } else { f.effort_claude.clone() };
            let next = step_choice(&levels, &current, delta);
            if codex {
                f.effort_codex = next;
            } else {
                f.effort_claude = next;
            }
        }
        Row::Model => {
            let target = if codex { &mut f.model_codex } else { &mut f.model_claude };
            type_into(target, key);
        }
        Row::Effort => {
            let target = if codex { &mut f.effort_codex } else { &mut f.effort_claude };
            type_into(target, key);
        }
        Row::ServiceTier if delta != 0 => {
            f.service_tier = step_choice(&["", "fast"], &f.service_tier.clone(), delta);
        }
        Row::Prompt => type_into(&mut f.prompt, key),
        Row::Harness | Row::Mode | Row::ServiceTier => {}
    }
}

fn type_into(target: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => target.push(c),
        KeyCode::Backspace => {
            target.pop();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{CoreSection, MODES, Row, mode_hint, rows_for, step_choice};
    use cctui_clientcore::spawn::SpawnFields;

    use crate::app::spawn::SpawnSection;

    /// `handle` needs the fields; the section itself is stateless.
    trait HandleAt {
        fn handle_at(&self, row: usize, key: KeyEvent, f: &mut SpawnFields);
    }
    impl HandleAt for CoreSection {
        fn handle_at(&self, row: usize, key: KeyEvent, f: &mut SpawnFields) {
            let mut me = Self::default();
            SpawnSection::handle(&mut me, row, key, f);
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn fields() -> SpawnFields {
        SpawnFields { adapter_id: "claude-code".to_owned(), ..SpawnFields::default() }
    }

    #[test]
    fn the_tier_row_appears_only_for_codex() {
        assert!(!rows_for("claude-code").contains(&Row::ServiceTier));
        assert!(rows_for("codex").contains(&Row::ServiceTier));
        assert_eq!(rows_for("codex").len(), rows_for("claude-code").len() + 1);
    }

    #[test]
    fn switching_harness_changes_the_row_count_the_dialog_tabs_through() {
        let s = CoreSection::default();
        let mut f = fields();
        let before = s.rows(&f);
        f.adapter_id = "codex".to_owned();
        assert_eq!(s.rows(&f), before + 1);
    }

    #[test]
    fn a_choice_row_clamps_rather_than_wrapping() {
        let modes: Vec<&str> = MODES.iter().map(|(v, _, _)| *v).collect();
        assert_eq!(step_choice(&modes, "", -1), "", "already at the safest");
        assert_eq!(step_choice(&modes, "", 1), "ask");
        assert_eq!(step_choice(&modes, "whip", 1), "whip", "and at the most dangerous");
        assert_eq!(step_choice(&modes, "whip", -1), "yolo");
    }

    #[test]
    fn every_mode_has_a_hint_and_an_unknown_one_has_none() {
        for (value, _, hint) in MODES {
            assert!(!hint.is_empty(), "{value} has no hint");
            assert_eq!(mode_hint(value), *hint);
        }
        assert_eq!(mode_hint("sideways"), "");
    }

    #[test]
    fn left_and_right_step_the_harness_and_the_mode() {
        let s = CoreSection::default();
        let mut f = fields();
        let harness_row = rows_for("claude-code").iter().position(|r| *r == Row::Harness).unwrap();
        s.handle_at(harness_row, key(KeyCode::Right), &mut f);
        assert_eq!(f.adapter_id, "codex");
        s.handle_at(harness_row, key(KeyCode::Left), &mut f);
        assert_eq!(f.adapter_id, "claude-code");

        let mode_row = rows_for("claude-code").iter().position(|r| *r == Row::Mode).unwrap();
        s.handle_at(mode_row, key(KeyCode::Right), &mut f);
        assert_eq!(f.permission_mode, "ask");
    }

    #[test]
    fn typing_lands_in_the_field_the_focused_row_names() {
        let s = CoreSection::default();
        let mut f = fields();
        let rows = rows_for("claude-code");
        let dir = rows.iter().position(|r| *r == Row::Dir).unwrap();
        for c in "/tmp".chars() {
            s.handle_at(dir, key(KeyCode::Char(c)), &mut f);
        }
        assert_eq!(f.working_dir, "/tmp");
        s.handle_at(dir, key(KeyCode::Backspace), &mut f);
        assert_eq!(f.working_dir, "/tm");
    }

    #[test]
    fn the_model_row_writes_the_field_the_harness_selects() {
        let s = CoreSection::default();
        let mut f = fields();
        let model = rows_for("claude-code").iter().position(|r| *r == Row::Model).unwrap();
        s.handle_at(model, key(KeyCode::Char('o')), &mut f);
        assert_eq!(f.model_claude, "o");
        assert!(f.model_codex.is_empty());

        f.adapter_id = "codex".to_owned();
        let model = rows_for("codex").iter().position(|r| *r == Row::Model).unwrap();
        s.handle_at(model, key(KeyCode::Char('g')), &mut f);
        assert_eq!(f.model_codex, "g");
        assert_eq!(f.model_claude, "o", "the other harness keeps its pick");
    }

    #[test]
    fn a_row_renders_its_label_its_value_and_a_dash_when_empty() {
        let s = CoreSection::default();
        let mut f = fields();
        f.machine_id = "cyberia".to_owned();
        let lines = s.lines(Some(0), 60, &f);
        let first: String = lines[0].spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(first.contains("Machine"));
        assert!(first.contains("cyberia"));
        assert!(first.starts_with('❯'), "the focused row is marked");

        let unfocused: String =
            s.lines(None, 60, &f)[2].spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(unfocused.contains('—'), "an empty field shows a dash: {unfocused}");
    }

    #[test]
    fn the_mode_row_is_followed_by_its_hint() {
        let s = CoreSection::default();
        let mut f = fields();
        f.permission_mode = "yolo".to_owned();
        let rendered: Vec<String> = s
            .lines(None, 60, &f)
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        // `Model` also contains `Mode`, so match the padded label.
        let mode_at = rendered.iter().position(|l| l.contains("Mode     ")).expect("a mode row");
        assert!(
            rendered[mode_at + 1].contains("skips prompts and sandbox"),
            "the hint follows the row: {:?}",
            rendered[mode_at + 1]
        );
    }

    #[test]
    fn a_long_value_is_clipped_to_the_dialog_width() {
        let s = CoreSection::default();
        let mut f = fields();
        f.working_dir = "/".to_owned() + &"x".repeat(200);
        for width in [40_u16, 60, 80] {
            let line = &s.lines(None, width, &f)[1];
            let cols: usize = line.spans.iter().map(|sp| sp.content.chars().count()).sum();
            assert!(cols <= usize::from(width), "{width} columns overflowed: {cols}");
        }
    }
}
