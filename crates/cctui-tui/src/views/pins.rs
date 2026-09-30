use ratatui::Frame;

use crate::app::pins::PinsList;
use crate::widgets::picker::{Picker, PickerRow, draw as draw_picker};

const HOTKEYS: &[(&str, &str)] =
    &[("↑↓", "Pick"), ("Enter", "Jump"), ("m/d", "Unpin"), ("Esc", "Close")];

pub fn draw(frame: &mut Frame, list: &PinsList) {
    let rows: Vec<PickerRow> = list
        .rows
        .iter()
        .map(|row| PickerRow {
            lead: format!("#{} {}", row.seq, row.role),
            text: row.excerpt.clone(),
        })
        .collect();
    draw_picker(
        frame,
        &Picker {
            title: "Pinned messages",
            filter: None,
            rows: &rows,
            selected: list.selected,
            empty: " no pinned messages",
            hotkeys: HOTKEYS,
        },
    );
}
