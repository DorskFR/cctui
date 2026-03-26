use cctui_proto::api::SessionListItem;
use cctui_proto::models::SessionStatus;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, StatefulWidget};

pub struct SessionTree<'a> {
    items: &'a [&'a SessionListItem],
}

impl<'a> SessionTree<'a> {
    pub const fn new(items: &'a [&'a SessionListItem]) -> Self {
        Self { items }
    }
}

const fn status_icon(status: &SessionStatus) -> (&'static str, Color) {
    match status {
        SessionStatus::Active => ("●", Color::Green),
        SessionStatus::Idle => ("○", Color::Yellow),
        SessionStatus::Registering => ("◌", Color::Cyan),
        SessionStatus::Terminated => ("✕", Color::Red),
        SessionStatus::Disconnected => ("◎", Color::DarkGray),
    }
}

pub fn format_uptime(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        format!("{tokens}")
    } else if tokens < 1_000_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    }
}

fn short_uuid(id: uuid::Uuid) -> String {
    id.to_string()[..8].to_string()
}

fn project_name(item: &SessionListItem) -> &str {
    item.working_dir.split('/').next_back().unwrap_or(&item.working_dir)
}

impl StatefulWidget for SessionTree<'_> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let list_items: Vec<ListItem> = self
            .items
            .iter()
            .map(|item| {
                let indent = if item.parent_id.is_some() { "  " } else { "" };
                let (icon, color) = status_icon(&item.status);
                let total_tokens = item.token_usage.tokens_in + item.token_usage.tokens_out;

                let line = Line::from(vec![
                    Span::raw(indent),
                    Span::styled(icon, Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(short_uuid(item.id), Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::styled(project_name(item), Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(
                        format_uptime(item.uptime_secs),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(" "),
                    Span::styled(format_tokens(total_tokens), Style::default().fg(Color::Blue)),
                ]);

                ListItem::new(line)
            })
            .collect();

        let list = List::new(list_items)
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));

        StatefulWidget::render(list, area, buf, state);
    }
}
