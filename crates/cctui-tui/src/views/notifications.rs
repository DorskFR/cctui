use cctui_proto::api::notifications::Notification;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::notifications::{NotificationState, Panel, actions, is_unread, outcome};
use crate::theme;

const HOTKEYS: &[(&str, &str)] =
    &[("↑↓", "Pick"), ("1-9", "Answer"), ("d", "Dismiss"), ("R", "All read"), ("Esc", "Close")];

const DETAIL_HEIGHT: u16 = 6;

fn row(n: &Notification, focused: bool) -> Line<'static> {
    let title = n.title.replace('\n', " ");
    let mut spans = vec![
        Span::styled(if focused { "❯ " } else { "  " }.to_owned(), theme::border_focused()),
        Span::styled(if is_unread(n) { "● " } else { "  " }.to_owned(), theme::unread()),
        Span::styled(format!("{:<12} ", n.kind), theme::dim()),
        Span::styled(title, if focused { theme::bold() } else { theme::dim() }),
    ];
    if n.repeat_count > 1 {
        spans.push(Span::styled(format!(" ×{}", n.repeat_count), theme::dim()));
    }
    if let Some(done) = outcome(n) {
        spans.push(Span::styled(format!("  {done}"), theme::active()));
    }
    Line::from(spans)
}

fn detail(n: &Notification) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = n
        .body
        .as_deref()
        .unwrap_or_default()
        .lines()
        .map(|l| Line::from(Span::raw(l.to_owned())))
        .collect();
    let offered = actions(n);
    if !offered.is_empty() {
        let mut spans = Vec::new();
        for (i, a) in offered.iter().enumerate() {
            spans.push(Span::styled(format!(" {} ", i + 1), theme::hotkey()));
            spans.push(Span::raw(format!("{} ", a.label)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn list(panel: &Panel, items: &[Notification], height: usize) -> Vec<Line<'static>> {
    if panel.loading && items.is_empty() {
        return vec![Line::from(Span::styled("loading…".to_owned(), theme::dim()))];
    }
    if items.is_empty() {
        return vec![Line::from(Span::styled("no open notifications".to_owned(), theme::dim()))];
    }
    let first = panel.selected.saturating_sub(height.saturating_sub(1));
    items
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(i, n)| row(n, i == panel.selected))
        .collect()
}

pub fn draw(frame: &mut Frame, state: &NotificationState) {
    let Some(panel) = state.panel.as_ref() else { return };
    let area = frame.area().inner(Margin { horizontal: 4, vertical: 3 });
    frame.render_widget(Clear, area);

    let title = format!(" Notifications · {} unread ", state.unread);
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [list_area, detail_area, note_area, hotkeys_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(DETAIL_HEIGHT),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let rows = list(panel, &state.items, usize::from(list_area.height));
    frame.render_widget(Paragraph::new(rows), list_area);

    if let Some(n) = state.focused() {
        let pane = Block::default().borders(Borders::TOP).border_style(theme::dim());
        let text = Paragraph::new(detail(n)).block(pane).wrap(Wrap { trim: false });
        frame.render_widget(text, detail_area);
    }

    let note = if panel.busy { Some("working…".to_owned()) } else { panel.error.clone() };
    if let Some(note) = note {
        frame.render_widget(Paragraph::new(Span::styled(note, theme::dim())), note_area);
    }

    let mut footer = Vec::new();
    for (keys, label) in HOTKEYS {
        footer.push(Span::styled(format!(" {keys} "), theme::hotkey()));
        footer.push(Span::raw(format!("{label} ")));
    }
    frame.render_widget(Paragraph::new(Line::from(footer)), hotkeys_area);
}
