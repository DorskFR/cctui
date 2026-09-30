use ratatui::Frame;

use crate::app::drafts::Picker as HistoryPicker;
use crate::widgets::picker::{Picker, PickerRow, draw as draw_picker};

const HOTKEYS: &[(&str, &str)] = &[("↑↓", "Pick"), ("Enter", "Recall"), ("Esc", "Cancel")];

pub fn draw(frame: &mut Frame, picker: &HistoryPicker) {
    let rows: Vec<PickerRow> = picker.matches().into_iter().map(PickerRow::plain).collect();
    let empty = if picker.filter.is_empty() { " no prompts yet" } else { " no match" };
    draw_picker(
        frame,
        &Picker {
            title: "Prompt history",
            filter: Some(&picker.filter),
            rows: &rows,
            selected: picker.selected,
            empty,
            hotkeys: HOTKEYS,
        },
    );
}
