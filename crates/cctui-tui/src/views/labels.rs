//! The `l` label picker and the `L` label filter, both on the one picker widget.

use crate::app::App;
use crate::app::labels::{LabelPicker, PickerMode};
use crate::widgets::picker::{Picker, PickerRow, draw as draw_picker};

/// The swatch a chip's hue paints; the glyph is the same for every label so the
/// colour is the only thing that varies.
const SWATCH: &str = "■";

const fn checkbox(attached: bool) -> &'static str {
    if attached { "[x]" } else { "[ ]" }
}

pub fn draw(frame: &mut ratatui::Frame, app: &App) {
    let Some(picker) = app.labels.picker.as_ref() else { return };
    let attached = attached_ids(app, &picker.session_id);
    let matches = app.labels.matching(&picker.filter);

    let rows: Vec<PickerRow> = match &picker.mode {
        PickerMode::Hue { index, .. } => hue_rows(*index),
        PickerMode::ConfirmDelete { name, .. } => {
            vec![PickerRow::plain(format!("delete \"{name}\"? y to confirm, n to keep it"))]
        }
        PickerMode::Browse | PickerMode::Create { .. } | PickerMode::Rename { .. } => matches
            .iter()
            .map(|label| PickerRow {
                lead: format!("{} {SWATCH}", checkbox(attached.contains(&label.id))),
                text: label.name.clone(),
            })
            .collect(),
    };

    let title = title_for(app, picker);
    // The hue ring and the delete confirmation take no text, so they get no
    // input line rather than an empty one that looks broken.
    let filter = filter_line(picker);
    let selected = match &picker.mode {
        PickerMode::Hue { index, .. } => *index,
        _ => picker.selected,
    };
    draw_picker(
        frame,
        &Picker {
            title: &title,
            prompt: prompt_label(&picker.mode),
            filter: filter.as_deref(),
            rows: &rows,
            selected,
            empty: empty_text(picker),
            hotkeys: hotkeys_for(&picker.mode),
        },
    );
}

/// The row's own name for a session: what the list shows, so the picker's title
/// matches the row it was opened from.
fn session_label(s: &cctui_proto::api::SessionListItem) -> String {
    if let Some(name) = s.name.clone().filter(|n| !n.trim().is_empty()) {
        return name;
    }
    s.metadata.get("project_name").and_then(serde_json::Value::as_str).map_or_else(
        || s.working_dir.rsplit('/').next().unwrap_or("session").to_owned(),
        str::to_owned,
    )
}

fn attached_ids(app: &App, session_id: &str) -> Vec<String> {
    app.sessions
        .iter()
        .find(|s| s.id == session_id)
        .map(|s| s.labels.iter().map(|l| l.id.clone()).collect())
        .unwrap_or_default()
}

/// One row per preset, plus Auto; the hue number is the lead so the choice is
/// legible without colour.
fn hue_rows(_selected: usize) -> Vec<PickerRow> {
    let mut rows: Vec<PickerRow> = cctui_clientcore::labels::LABEL_HUES
        .iter()
        .map(|hue| PickerRow { lead: format!("{SWATCH} {hue:>3}"), text: String::new() })
        .collect();
    rows.push(PickerRow { lead: "  auto".to_owned(), text: "hue from the name".to_owned() });
    rows
}

/// What the input line is asking for in this mode.
const fn prompt_label(mode: &PickerMode) -> &'static str {
    match mode {
        PickerMode::Create { .. } | PickerMode::Rename { .. } => "name",
        _ => "filter",
    }
}

fn title_for(app: &App, picker: &LabelPicker) -> String {
    let session = app
        .sessions
        .iter()
        .find(|s| s.id == picker.session_id)
        .map_or_else(|| picker.session_id.chars().take(8).collect(), session_label);
    match &picker.mode {
        PickerMode::Browse => format!("Labels: {session}"),
        PickerMode::Create { .. } => "New label".to_owned(),
        PickerMode::Rename { .. } => "Rename label".to_owned(),
        PickerMode::Hue { .. } => "Pick a hue".to_owned(),
        PickerMode::ConfirmDelete { .. } => "Delete label".to_owned(),
    }
}

/// The prompt line: the fuzzy query while browsing, the name being typed
/// otherwise.
fn filter_line(picker: &LabelPicker) -> Option<String> {
    match &picker.mode {
        PickerMode::Browse => Some(picker.filter.clone()),
        PickerMode::Create { name } | PickerMode::Rename { name, .. } => Some(name.clone()),
        PickerMode::Hue { .. } | PickerMode::ConfirmDelete { .. } => None,
    }
}

const fn empty_text(picker: &LabelPicker) -> &'static str {
    if picker.filter.is_empty() {
        "no labels yet — Ctrl+c makes one"
    } else {
        "nothing matches — Ctrl+c creates it"
    }
}

const fn hotkeys_for(mode: &PickerMode) -> &'static [(&'static str, &'static str)] {
    match mode {
        PickerMode::Browse => &[
            ("space", "attach"),
            ("Ctrl+c", "new"),
            ("Ctrl+e", "edit"),
            ("Ctrl+d", "delete"),
            ("esc", "close"),
        ],
        PickerMode::Create { .. } | PickerMode::Rename { .. } => {
            &[("enter", "next"), ("esc", "back")]
        }
        PickerMode::Hue { .. } => &[("←/→", "hue"), ("enter", "save"), ("esc", "back")],
        PickerMode::ConfirmDelete { .. } => &[("y", "delete"), ("n", "keep")],
    }
}

/// The `L` filter: every label with a mark on the ones narrowing the list.
pub fn draw_filter(frame: &mut ratatui::Frame, app: &App) {
    let rows: Vec<PickerRow> = app
        .labels
        .all
        .iter()
        .map(|label| PickerRow {
            lead: format!("{} {SWATCH}", checkbox(app.labels.filter.contains(&label.id))),
            text: label.name.clone(),
        })
        .collect();
    draw_picker(
        frame,
        &Picker {
            title: "Filter by label (any of)",
            prompt: "filter",
            filter: None,
            rows: &rows,
            selected: app.labels.filter_selected,
            empty: "no labels yet",
            hotkeys: &[("space", "toggle"), ("a", "show all"), ("esc", "close")],
        },
    );
}
