//! The spawn dialog. Draws each registered section in order; a lane that adds a
//! section needs no change here.

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

/// Every section's rows, then the errors, then the key hints.
fn body_lines(form: &SpawnForm, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (index, section) in form.sections.iter().enumerate() {
        let focused = (form.focus.section == index).then_some(form.focus.row);
        if index > 0 {
            out.push(Line::from(Span::styled(
                clip(&format!(" {}", section.title()), width),
                theme::section_title(),
            )));
        }
        out.extend(section.lines(focused, width, &form.fields));
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
