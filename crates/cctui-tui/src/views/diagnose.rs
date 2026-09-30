//! The diagnose / session-info overlay.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::app::diagnose::{Row, Tone, panel_rows};
use crate::theme;

/// Widest label before the value column starts losing content; on a narrow
/// panel the label gives way so the value keeps half the row.
const LABEL_COLS: usize = 34;

/// Below this there is no room for the worded hotkey hints.
const COMPACT_HINTS_UNDER: usize = 72;

fn label_cols(width: usize) -> usize {
    LABEL_COLS.min(width / 2)
}

fn tone_style(tone: Tone) -> Style {
    match tone {
        Tone::Heading => theme::section_title(),
        Tone::Normal => Style::new(),
        Tone::Dim => theme::dim(),
        Tone::Warn => theme::stale(),
        Tone::Error => theme::error(),
    }
}

/// A heading spans the row; every other row is `label  value`, the value
/// truncated rather than wrapped so one long JSON fact cannot eat the panel.
fn row_line(row: &Row, width: usize) -> Line<'static> {
    if row.tone == Tone::Heading {
        let suffix = if row.label.ends_with('?') { "" } else { ":" };
        return Line::from(Span::styled(format!("{}{suffix}", row.label), theme::section_title()));
    }
    if row.label.is_empty() {
        return Line::from(Span::styled(
            crate::app::session_status::truncate(&row.value, width),
            tone_style(row.tone),
        ));
    }
    let label_cols = label_cols(width);
    let label = crate::app::session_status::truncate(&row.label, label_cols);
    let pad = label_cols.saturating_sub(label.chars().count()) + 1;
    let room = width.saturating_sub(label_cols + 1);
    vec![
        Span::styled(label, theme::dim()),
        Span::raw(" ".repeat(pad)),
        Span::styled(crate::app::session_status::truncate(&row.value, room), tone_style(row.tone)),
    ]
    .into()
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(panel) = app.diagnose.as_ref() else { return };
    let area = frame.area().inner(Margin { horizontal: 3, vertical: 2 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(panel.mode.title());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [head_area, body_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(panel.session_id.clone(), theme::bold()))),
        head_area,
    );

    let rows = panel_rows(app);
    let width = usize::from(body_area.width);
    let height = usize::from(body_area.height);
    // Clamped here rather than in the reducer: only the render knows the height.
    let max_scroll = rows.len().saturating_sub(height);
    let offset = panel.scroll.min(max_scroll);
    let lines: Vec<Line> =
        rows.iter().skip(offset).take(height).map(|row| row_line(row, width)).collect();
    frame.render_widget(Paragraph::new(lines), body_area);

    let compact = width < COMPACT_HINTS_UNDER;
    let mut hotkeys = Vec::with_capacity(10);
    for (keys, label) in [("j/k", "Scroll"), ("r", "Refresh"), ("Esc", "Close")] {
        hotkeys.push(Span::styled(format!(" {keys}"), theme::hotkey()));
        if !compact {
            hotkeys.push(Span::raw(format!(" {label} ")));
        }
    }
    if rows.len() > height {
        hotkeys.push(Span::styled(
            format!("   {}/{}", offset + height.min(rows.len()), rows.len()),
            theme::dim(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(hotkeys)), hotkeys_area);
}
