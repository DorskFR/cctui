use ratatui::Frame;

use crate::app::macros::MacroState;
use crate::widgets::picker::{Picker, PickerRow, draw as draw_picker};

const HOTKEYS: &[(&str, &str)] = &[("↑↓", "Pick"), ("Enter", "Insert"), ("Esc", "Cancel")];

pub fn draw(frame: &mut Frame, macros: &MacroState) {
    let Some(picker) = macros.picker.as_ref() else { return };
    let rows: Vec<PickerRow> = macros
        .matches(&picker.filter)
        .into_iter()
        .map(|mac| PickerRow { lead: mac.title.clone(), text: mac.prompt.clone() })
        .collect();
    draw_picker(
        frame,
        &Picker {
            title: "Macros",
            prompt: "filter",
            filter: Some(&picker.filter),
            rows: &rows,
            selected: picker.selected,
            empty: " no match",
            hotkeys: HOTKEYS,
        },
    );
}
