//! The spawn dialog. Draws each registered section in order, so adding one
//! needs no change here.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::app::spawn::SpawnForm;
use crate::theme;

/// Widest the dialog grows. The 80-column budget is the row budget, and the
/// dialog is centred inside it with a border on each side.
const MAX_WIDTH: u16 = 72;

fn area(frame_area: Rect, rows: u16) -> Rect {
    let height = (rows + 2).min(frame_area.height);
    let width = MAX_WIDTH.min(frame_area.width);
    Rect {
        x: frame_area.x + (frame_area.width.saturating_sub(width)) / 2,
        y: frame_area.y + (frame_area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(form) = app.spawn.as_ref() else { return };
    let outer = frame.area();
    let inner_width = MAX_WIDTH.min(outer.width).saturating_sub(2);
    let body = body_lines(form, inner_width);
    let rows = u16::try_from(body.len()).unwrap_or(u16::MAX);
    let area = area(outer, rows);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" New session ");
    frame.render_widget(Paragraph::new(body).block(block), area);
}

/// Clips to the dialog's inner width: a narrow terminal must not push a row
/// past the border.
fn clip(text: &str, width: u16) -> String {
    text.chars().take(usize::from(width)).collect()
}

/// The hint under the Model row: why a model is annotated, and whether the
/// catalog is being re-read.
fn model_lines(form: &SpawnForm, width: u16) -> Vec<Line<'static>> {
    use cctui_proto::harness_models::ModelHint;

    let mut out = Vec::new();
    if form.refreshing_models {
        out.push(Line::from(Span::styled(
            clip("           re-reading the catalog…", width),
            theme::dim(),
        )));
        return out;
    }
    let current = form.model().to_owned();
    let options = form.model_options();
    let Some(option) = options.iter().find(|o| o.v == current) else { return out };
    let Some(hint) = option.hint.as_ref() else { return out };
    let text = match hint {
        ModelHint::Gated { version, current } => {
            format!("needs codex \u{2265} {version}, catalog is {current}")
        }
        ModelHint::NeedsVersion { version } => format!("needs codex \u{2265} {version}"),
    };
    let style = if option.disabled { theme::error() } else { theme::dim() };
    out.push(Line::from(Span::styled(clip(&format!("           {text}"), width), style)));
    out
}

/// The badge beside the Dir row, and the dropdown under it while it is open.
fn cwd_lines(form: &SpawnForm, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if let Some(badge) = form.cwd.badge_text() {
        let style = if form.cwd.not_a_dir { theme::error() } else { theme::branch() };
        out.push(Line::from(Span::styled(clip(&format!("           {badge}"), width), style)));
    }
    if !form.on_dir_row() || form.cwd.picking.is_none() {
        return out;
    }
    for (index, dir) in form.cwd.offered().iter().enumerate().take(6) {
        let on = form.cwd.picking == Some(index);
        out.push(Line::from(Span::styled(
            clip(&format!("      {} {dir}", if on { '❯' } else { ' ' }), width),
            if on { theme::selected() } else { theme::dim() },
        )));
    }
    out
}

/// Every section's rows, then the errors, then the key hints.
fn body_lines(form: &SpawnForm, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let core = form.core_index();
    for (index, section) in form.sections.iter().enumerate() {
        let focused = (form.focus.section == index).then_some(form.focus.row);
        if Some(index) != core {
            out.push(Line::from(Span::styled(
                clip(&format!(" {}", section.title()), width),
                theme::section_title(),
            )));
        }
        let mut lines = section.lines(focused, width, &form.fields);
        // The badge and the dropdown belong to the cwd state, not to the core
        // section, which only knows the field's text — so they are slotted in
        // under the Dir row rather than appended after the section.
        if Some(index) == core {
            // Slot each row's own extra lines in from the bottom up, so an
            // earlier insert cannot move a later index.
            let model_at =
                crate::app::spawn::core_section::model_line_index(&form.fields.adapter_id);
            let model_extra = model_lines(form, width);
            let at = (model_at + 1).min(lines.len());
            lines.splice(at..at, model_extra);

            let dir_at = crate::app::spawn::core_section::dir_line_index(&form.fields.adapter_id);
            let dir_extra = cwd_lines(form, width);
            let at = (dir_at + 1).min(lines.len());
            lines.splice(at..at, dir_extra);
        }
        out.extend(lines);
    }
    for problem in &form.errors {
        out.push(Line::from(Span::styled(clip(&format!(" ✗ {problem}"), width), theme::error())));
    }
    let footer = if form.submitting {
        "  launching…".to_owned()
    } else {
        "  Ctrl-S launch   Tab next   Esc cancel".to_owned()
    };
    out.push(Line::from(Span::styled(clip(&footer, width), theme::dim())));
    out
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{MAX_WIDTH, body_lines};
    use crate::app::spawn::SpawnForm;

    fn form() -> SpawnForm {
        let mut form = SpawnForm::new();
        form.fields.machine_id = "cyberia".to_owned();
        form.fields.working_dir = "/home/dev/cctui".to_owned();
        form
    }

    #[test]
    fn the_dialog_is_centred_and_clipped_to_the_frame() {
        let frame = Rect { x: 0, y: 0, width: 100, height: 40 };
        let a = super::area(frame, 12);
        assert_eq!(a.width, MAX_WIDTH);
        assert_eq!(a.height, 14);
        assert_eq!(a.x, (100 - MAX_WIDTH) / 2);

        let tiny = Rect { x: 0, y: 0, width: 30, height: 8 };
        let clipped = super::area(tiny, 12);
        assert_eq!(clipped.width, 30);
        assert_eq!(clipped.height, 8);
    }

    #[test]
    fn the_body_shows_every_core_row_and_the_key_hints() {
        let form = form();
        let text: Vec<String> = body_lines(&form, 70)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        for label in ["Machine", "Dir", "Name", "Harness", "Model", "Effort", "Mode", "Prompt"] {
            assert!(text.iter().any(|l| l.contains(label)), "{label} is missing");
        }
        assert!(text.last().expect("a hint row").contains("Ctrl-S launch"));
        assert!(text.iter().any(|l| l.contains("cyberia")));
    }

    #[test]
    fn an_error_from_the_last_submit_is_shown_above_the_hints() {
        let mut form = form();
        form.errors = vec!["machine offline".to_owned()];
        let text: Vec<String> = body_lines(&form, 70)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let at = text.iter().position(|l| l.contains("machine offline")).expect("the error");
        assert!(text[at].contains('✗'));
        assert!(text[at + 1].contains("Ctrl-S"), "the hints stay last");
    }

    #[test]
    fn an_in_flight_launch_replaces_the_hints() {
        let mut form = form();
        form.submitting = true;
        let text: Vec<String> = body_lines(&form, 70)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text.last().expect("a row").contains("launching"));
    }

    #[test]
    fn no_row_overflows_the_dialog() {
        let mut form = form();
        form.fields.working_dir = "/".to_owned() + &"x".repeat(300);
        for width in [30_u16, 50, 70] {
            for line in body_lines(&form, width) {
                let cols: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                assert!(cols <= usize::from(width), "{width}: a row ran to {cols}");
            }
        }
    }
}
