//! The machines table.
//!
//! Columns are fixed width so the table stays a table; the name is the only
//! elastic one and is cut rather than allowed to push the rest off an 80-column
//! terminal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::app::machines::MachineRow;
use crate::theme;

/// Width of each fixed column, header and row alike, so the two cannot drift
/// apart. Each is one wider than its longest value, which is the gap.
const W_STATE: usize = 8;
const W_SEEN: usize = 6;
const W_SESS: usize = 5;
const W_CPU: usize = 5;
const W_MEM: usize = 9;

/// Everything but the elastic name column, plus its trailing gap.
const FIXED_COLS: usize = W_STATE + W_SEEN + W_SESS + W_CPU + W_MEM + 2;

pub fn draw(frame: &mut Frame, app: &App) {
    let [title_area, header_area, list_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let (online, total) = app.machines.counts();
    let mut title = vec![
        Span::styled(" Machines", theme::section_title()),
        Span::styled(format!("  {online}/{total} online"), theme::dim()),
    ];
    if app.machines.loading {
        title.push(Span::styled("  …", theme::dim()));
    }
    title.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(title)), title_area);

    let name_width = name_width(list_area.width);
    frame.render_widget(Paragraph::new(header(name_width)), header_area);

    if let Some(error) = &app.machines.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            list_area,
        );
        draw_hotkeys(frame, hotkeys_area);
        return;
    }
    if app.machines.rows.is_empty() {
        let text = if app.machines.loaded { " no machines enrolled" } else { " loading…" };
        frame
            .render_widget(Paragraph::new(Line::from(Span::styled(text, theme::dim()))), list_area);
        draw_hotkeys(frame, hotkeys_area);
        return;
    }

    let items: Vec<ListItem> =
        app.machines.rows.iter().map(|row| line(row, app.clock_ms, name_width)).collect();
    let list = List::new(items).highlight_style(theme::selected()).highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.machines.selected));
    frame.render_stateful_widget(list, list_area, &mut state);
    draw_hotkeys(frame, hotkeys_area);
}

/// Whatever the fixed columns and the cursor gutter leave, within reason.
fn name_width(area_width: u16) -> usize {
    let usable = usize::from(area_width).saturating_sub(FIXED_COLS + 2);
    usable.clamp(8, 22)
}

fn header(name_width: usize) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "   {:<name_width$}  {:<W_STATE$}{:<W_SEEN$}{:<W_SESS$}{:<W_CPU$}{}",
            "NAME", "STATE", "SEEN", "SESS", "CPU", "MEM"
        ),
        theme::dim(),
    ))
}

fn line(row: &MachineRow, now_ms: i64, name_width: usize) -> ListItem<'static> {
    // An offline daemon is not a spawn target, so the whole row reads as spent
    // rather than only its state cell.
    let dim = !row.reachable();
    let name_style = if dim { theme::dim() } else { theme::hue_style(row.hue) };
    let rest_style = if dim { theme::dim() } else { theme::bold() };
    let state_style = if dim { theme::dim() } else { state_tint(row) };

    let name = crate::app::session_status::truncate(&row.name, name_width);
    let sessions = if row.sessions == 0 { "-".to_owned() } else { row.sessions.to_string() };
    ListItem::new(Line::from(vec![
        Span::styled(format!(" {name:<name_width$}  "), name_style),
        Span::styled(format!("{:<W_STATE$}", row.state_text()), state_style),
        Span::styled(format!("{:<W_SEEN$}", row.seen_text(now_ms)), theme::dim()),
        Span::styled(format!("{sessions:<W_SESS$}"), rest_style),
        Span::styled(format!("{:<W_CPU$}", row.cpu_text()), theme::dim()),
        Span::styled(row.mem_text(), theme::dim()),
    ]))
}

fn state_tint(row: &MachineRow) -> ratatui::style::Style {
    match row.liveness {
        cctui_proto::models::MachineLiveness::Online => theme::active(),
        cctui_proto::models::MachineLiveness::Stale => theme::stale(),
        cctui_proto::models::MachineLiveness::Offline => theme::error(),
    }
}

fn draw_hotkeys(frame: &mut Frame, area: Rect) {
    let spans = vec![
        Span::styled(" j/k", theme::hotkey()),
        Span::styled(":move  ", theme::hotkey_desc()),
        Span::styled("Enter", theme::hotkey()),
        Span::styled(":spawn here  ", theme::hotkey_desc()),
        Span::styled("r", theme::hotkey()),
        Span::styled(":refresh  ", theme::hotkey_desc()),
        Span::styled("Esc", theme::hotkey()),
        Span::styled(":back", theme::hotkey_desc()),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::{FIXED_COLS, name_width};

    #[test]
    fn the_name_column_takes_what_is_left_within_bounds() {
        assert_eq!(name_width(200), 22, "capped so the table does not sprawl");
        assert_eq!(name_width(40), 8, "floored so a name is still readable");
        assert!((8..=22).contains(&name_width(80)));
    }

    #[test]
    fn the_fixed_columns_leave_room_for_a_name_at_eighty() {
        const { assert!(FIXED_COLS + 2 + 8 <= 80, "the floor name width has to fit in 80 columns") };
    }
}
