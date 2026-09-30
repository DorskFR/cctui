//! The `M` overlay: two lists, model and effort, applied together.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::controls::{ModelPicker, PickerColumn};
use crate::theme;

pub fn draw(frame: &mut Frame, picker: &ModelPicker) {
    let area = frame.area().inner(Margin { horizontal: 10, vertical: 4 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(format!(" Model · {} ", picker.harness));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [lists_area, hint_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    if picker.loading {
        frame.render_widget(
            Paragraph::new(Span::styled("  reading the model list…", theme::dim())),
            lists_area,
        );
    } else {
        let [models_area, efforts_area] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(lists_area);
        draw_models(frame, picker, models_area);
        draw_efforts(frame, picker, efforts_area);
    }

    frame.render_widget(Paragraph::new(Line::from(hints(picker))), hint_area);
}

fn draw_models(frame: &mut Frame, picker: &ModelPicker, area: ratatui::layout::Rect) {
    let focused = picker.column == PickerColumn::Model;
    let items: Vec<ListItem> = picker
        .models
        .iter()
        .map(|m| {
            let label =
                if m.v.is_empty() { m.label.clone() } else { format!("{} ({})", m.label, m.v) };
            let style = if m.disabled { theme::dim() } else { theme::bold() };
            let mut spans = vec![Span::styled(label, style)];
            if let Some(hint) = model_hint(m) {
                spans.push(Span::styled(format!("  {hint}"), theme::dim()));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    render_column(frame, area, "Model", items, picker.model_index, focused);
}

fn draw_efforts(frame: &mut Frame, picker: &ModelPicker, area: ratatui::layout::Rect) {
    let focused = picker.column == PickerColumn::Effort;
    if picker.efforts.is_empty() {
        let block = column_block("Effort", focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(Span::styled(" no effort dial", theme::dim())), inner);
        return;
    }
    let items: Vec<ListItem> = picker
        .efforts
        .iter()
        .map(|e| {
            let label = if e.is_empty() { "default".to_owned() } else { e.clone() };
            ListItem::new(Line::from(Span::styled(label, theme::bold())))
        })
        .collect();
    render_column(frame, area, "Effort", items, picker.effort_index, focused);
}

fn column_block(title: &str, focused: bool) -> Block<'static> {
    let style = if focused { theme::border_focused() } else { theme::border_dim() };
    Block::default().borders(Borders::ALL).border_style(style).title(format!(" {title} "))
}

fn render_column(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    title: &str,
    items: Vec<ListItem<'static>>,
    selected: usize,
    focused: bool,
) {
    let block = column_block(title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let list = List::new(items).highlight_style(theme::selected()).highlight_symbol(if focused {
        "▸ "
    } else {
        "  "
    });
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, inner, &mut state);
}

/// The domain says a model is gated; the wording is the client's.
fn model_hint(option: &cctui_proto::harness_models::ModelOption) -> Option<String> {
    use cctui_proto::harness_models::ModelHint;
    match option.hint.as_ref()? {
        ModelHint::Gated { version, current } => Some(format!("needs {version} · at {current}")),
        ModelHint::NeedsVersion { version } => Some(format!("needs {version}")),
    }
}

fn hints(picker: &ModelPicker) -> Vec<Span<'static>> {
    if picker.loading {
        return vec![Span::styled("  Esc cancel", theme::hotkey_desc())];
    }
    let apply = if picker.can_apply() { "apply" } else { "unavailable" };
    vec![
        Span::styled("  ↑↓ ", theme::hotkey()),
        Span::styled("pick  ", theme::hotkey_desc()),
        Span::styled("←→/Tab ", theme::hotkey()),
        Span::styled("column  ", theme::hotkey_desc()),
        Span::styled("Enter ", theme::hotkey()),
        Span::styled(format!("{apply}  "), theme::hotkey_desc()),
        Span::styled("Esc ", theme::hotkey()),
        Span::styled("cancel", theme::hotkey_desc()),
    ]
}
