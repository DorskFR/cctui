//! The context browser overlay.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::context_items::{Browser, Row, Tab};
use crate::theme;

fn row(row: &Row, focused: bool, armed: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled(if focused { "❯ " } else { "  " }.to_owned(), theme::border_focused()),
        Span::styled(format!("{:<7}", row.kind), theme::dim()),
        Span::styled(
            format!("{:<20}", row.name),
            if focused { theme::bold() } else { theme::dim() },
        ),
        Span::styled(
            format!("{:<24}", row.version_cell()),
            if row.newer.is_some() { theme::attention() } else { theme::dim() },
        ),
    ];
    for flag in row.flags() {
        let style = match flag {
            "pending" => theme::attention(),
            "disabled" => theme::stale(),
            _ => theme::active(),
        };
        spans.push(Span::styled(format!("{flag} "), style));
    }
    if armed {
        spans.push(Span::styled("d again to delete".to_owned(), theme::error()));
    }
    Line::from(spans)
}

pub fn draw(frame: &mut Frame, browser: &Browser) {
    let area = frame.area().inner(Margin { horizontal: 4, vertical: 3 });
    frame.render_widget(Clear, area);

    let title = match browser.tab {
        Tab::Catalogue => " Context · catalogue ".to_owned(),
        Tab::Session => " Context · this session ".to_owned(),
    };
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [list_area, note_area, hotkeys_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(inner);

    let rows: Vec<Line> = if browser.loading {
        vec![Line::from(Span::styled("loading…".to_owned(), theme::dim()))]
    } else if browser.rows.is_empty() {
        let empty = match browser.tab {
            Tab::Catalogue => "no context items yet",
            Tab::Session => "this session was given no rules or memories",
        };
        vec![Line::from(Span::styled(empty.to_owned(), theme::dim()))]
    } else {
        let height = usize::from(list_area.height.max(1));
        let first = browser.focused.saturating_sub(height - 1);
        browser
            .rows
            .iter()
            .enumerate()
            .skip(first)
            .take(height)
            .map(|(index, r)| {
                row(
                    r,
                    index == browser.focused,
                    browser.armed_delete.as_deref() == Some(r.id.as_str()),
                )
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(rows), list_area);

    let note = if browser.busy {
        Span::styled("working…".to_owned(), theme::dim())
    } else if let Some(error) = browser.error.as_ref() {
        Span::styled(error.clone(), theme::error())
    } else {
        let pending = browser.rows.iter().filter(|r| r.pending).count();
        if pending > 0 {
            Span::styled(format!("{pending} pending proposal(s)"), theme::attention())
        } else {
            Span::raw("")
        }
    };
    frame.render_widget(Paragraph::new(Line::from(note)), note_area);

    let mut hotkeys = vec![
        Span::styled(" ↑↓ ", theme::hotkey()),
        Span::raw("Pick  "),
        Span::styled("a ", theme::hotkey()),
        Span::raw("Approve  "),
        Span::styled("x ", theme::hotkey()),
        Span::raw("Reject  "),
        Span::styled("d ", theme::hotkey()),
        Span::raw("Delete  "),
    ];
    if browser.session_id.is_some() {
        hotkeys.push(Span::styled("Enter ", theme::hotkey()));
        hotkeys.push(Span::raw("Attach  "));
        hotkeys.push(Span::styled("tab ", theme::hotkey()));
        hotkeys.push(Span::raw("Tab  "));
    }
    hotkeys.push(Span::styled("r ", theme::hotkey()));
    hotkeys.push(Span::raw("Reload  "));
    hotkeys.push(Span::styled("Esc ", theme::hotkey()));
    hotkeys.push(Span::raw("Close"));
    frame.render_widget(Paragraph::new(Line::from(hotkeys)), hotkeys_area);
}
