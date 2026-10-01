//! The fork dialog, over the conversation it forks.

use ratatui::Frame;
use ratatui::layout::Margin;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::forkform::{Field, ForkForm};
use crate::theme;

pub fn draw(frame: &mut Frame, form: &ForkForm) {
    let area = frame.area().inner(Margin { horizontal: 8, vertical: 4 });
    frame.render_widget(Clear, area);

    let title = if form.codex { " Fork (codex) " } else { " Fork " };
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(rows(form, inner.width)), inner);
}

fn rows(form: &ForkForm, width: u16) -> Vec<Line<'static>> {
    if form.loading {
        return vec![Line::from(Span::styled("  reading the model list…", theme::dim()))];
    }
    let mut out: Vec<Line<'static>> =
        form.fields().into_iter().map(|field| row(form, field, width)).collect();
    out.push(Line::from(""));
    out.push(Line::from(hints()));
    out
}

fn row(form: &ForkForm, field: Field, width: u16) -> Line<'static> {
    let here = form.focused() == field;
    let marker = if here { "▸" } else { " " };
    let value = match field {
        Field::Model => picker(form.models.get(form.model_index).map_or("", |m| m.label.as_str())),
        Field::Effort => picker(effort_label(form.effort())),
        Field::Extract => picker(form.extract.label()),
        Field::Name => text_box(&form.name, width),
        Field::Prompt => text_box(&form.prompt, width),
    };
    let mut spans = vec![
        Span::styled(format!(" {marker} "), theme::hotkey()),
        Span::styled(format!("{:<8}", field.label()), theme::dim()),
        value,
    ];
    // A slice needs a line to anchor to, so say why the row will not move.
    if field == Field::Extract && !form.can_slice() {
        spans.push(Span::styled("  (no line cursor)", theme::dim()));
    }
    Line::from(spans)
}

const fn effort_label(effort: &str) -> &str {
    if effort.is_empty() { "default" } else { effort }
}

fn picker(label: &str) -> Span<'static> {
    Span::styled(format!("◂ {label} ▸"), theme::bold())
}

fn text_box(value: &str, width: u16) -> Span<'static> {
    let room = (width as usize).saturating_sub(16);
    let shown = if value.chars().count() <= room {
        value.to_owned()
    } else {
        let head: String = value.chars().take(room.saturating_sub(1)).collect();
        format!("{head}…")
    };
    Span::styled(format!("[{shown:<room$}]"), theme::border_dim())
}

fn hints() -> Vec<Span<'static>> {
    vec![
        Span::styled("  Tab ", theme::hotkey()),
        Span::styled("field  ", theme::hotkey_desc()),
        Span::styled("←→ ", theme::hotkey()),
        Span::styled("choose  ", theme::hotkey_desc()),
        Span::styled("Ctrl+s ", theme::hotkey()),
        Span::styled("fork  ", theme::hotkey_desc()),
        Span::styled("Esc ", theme::hotkey()),
        Span::styled("cancel", theme::hotkey_desc()),
    ]
}

#[cfg(test)]
mod tests {
    use super::rows;
    use crate::app::forkform::{Extract, ForkForm};

    fn form(codex: bool) -> ForkForm {
        use cctui_proto::harness_models::ModelOption;
        ForkForm {
            session_id: "s-a".to_owned(),
            codex,
            models: vec![ModelOption {
                v: "opus".to_owned(),
                label: "opus (parent)".to_owned(),
                hint: None,
                disabled: false,
            }],
            efforts: vec![String::new(), "high".to_owned()],
            model_index: 0,
            effort_index: 1,
            extract: Extract::Full,
            name: "alpha (fork)".to_owned(),
            prompt: String::new(),
            focus: 0,
            anchor_message_id: None,
            loading: false,
            submitting: false,
        }
    }

    fn text(form: &ForkForm, width: u16) -> String {
        rows(form, width)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_dialog_shows_the_parent_model_the_name_and_the_keys() {
        let rendered = text(&form(false), 70);
        assert!(rendered.contains("opus (parent)"), "{rendered}");
        assert!(rendered.contains("high"), "{rendered}");
        assert!(rendered.contains("alpha (fork)"), "{rendered}");
        assert!(rendered.contains("Ctrl+s fork"), "{rendered}");
    }

    #[test]
    fn a_codex_fork_has_no_from_row() {
        let rendered = text(&form(true), 70);
        assert!(!rendered.contains("From"), "{rendered}");
        assert!(text(&form(false), 70).contains("From"), "claude has one");
    }

    #[test]
    fn the_from_row_says_why_it_cannot_move_without_a_cursor() {
        let rendered = text(&form(false), 70);
        assert!(rendered.contains("no line cursor"), "{rendered}");

        let mut with_anchor = form(false);
        with_anchor.anchor_message_id = Some("m-42".to_owned());
        assert!(!text(&with_anchor, 70).contains("no line cursor"));
    }

    #[test]
    fn the_loading_state_says_so_instead_of_an_empty_picker() {
        let mut loading = form(false);
        loading.loading = true;
        assert!(text(&loading, 70).contains("reading the model list"));
    }

    #[test]
    fn no_row_overflows_the_dialog() {
        let mut long = form(false);
        long.name = "x".repeat(400);
        long.prompt = "y".repeat(400);
        for line in rows(&long, 64) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(width <= 64, "a fork row overflowed: {width}");
        }
    }
}
