//! The spawn dialog's Dispatch section.
//!
//! Drawn into whatever rectangle the dialog hands it; [`height`] says how many
//! rows that needs, so the core dialog reserves exactly the section's size.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::dispatch::{DispatchFields, Field};
use crate::theme;

/// Header, three field rows, the pack header and its two rows.
pub const HEIGHT: u16 = 7;

#[must_use]
pub const fn height(_fields: &DispatchFields) -> u16 {
    HEIGHT
}

pub fn draw(frame: &mut Frame, area: Rect, fields: &DispatchFields, focused: bool) {
    frame.render_widget(Paragraph::new(lines(fields, focused, area.width)), area);
}

/// The tab on its own, until the spawn dialog reserves a section for it. The
/// dialog's version calls [`draw`] with the rectangle it allotted; this only
/// supplies a frame around it.
pub fn draw_panel(frame: &mut Frame, app: &crate::app::App) {
    use ratatui::layout::Margin;
    use ratatui::widgets::{Block, Borders, Clear};

    let area = frame.area().inner(Margin { horizontal: 4, vertical: 3 });
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Dispatch a job ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Rect { height: height(&app.dispatch).min(inner.height), ..inner };
    draw(frame, rows, &app.dispatch, true);
}

/// Pure so the layout is readable in a test without a terminal.
fn lines(fields: &DispatchFields, focused: bool, width: u16) -> Vec<Line<'static>> {
    let budget = width as usize;
    vec![
        Line::from(vec![
            Span::styled(format!(" {} ", Field::Dispatcher.label()), theme::section_title()),
            boxed(fields, Field::Dispatcher, focused, 22),
            Span::styled("  Harness ", theme::dim()),
            radio(!fields.is_codex(), "claude"),
            Span::raw(" "),
            radio(fields.is_codex(), "codex"),
        ]),
        Line::from(vec![
            label(Field::Repo.label()),
            boxed(fields, Field::Repo, focused, 18),
            label(Field::Ticket.label()),
            boxed(fields, Field::Ticket, focused, 14),
            label(Field::Timeout.label()),
            boxed(fields, Field::Timeout, focused, 5),
            Span::styled(" min", theme::dim()),
        ]),
        Line::from(vec![
            label(Field::Prompt.label()),
            boxed(fields, Field::Prompt, focused, budget.saturating_sub(12)),
        ]),
        Line::from(Span::styled(" Context pack", theme::section_title())),
        Line::from(vec![
            label(&format!("  {}", Field::PackUrl.label())),
            boxed(fields, Field::PackUrl, focused, budget.saturating_sub(12)),
        ]),
        Line::from(vec![
            label(&format!("  {}", Field::PackRef.label())),
            boxed(fields, Field::PackRef, focused, 12),
            label(Field::PackSubdir.label()),
            boxed(fields, Field::PackSubdir, focused, 16),
            label(Field::PackToken.label()),
            boxed(fields, Field::PackToken, focused, 10),
        ]),
        Line::from(hints(fields)),
    ]
}

fn label(text: &str) -> Span<'static> {
    Span::styled(format!(" {text} "), theme::dim())
}

fn radio(on: bool, text: &str) -> Span<'static> {
    let mark = if on { "(*)" } else { "( )" };
    Span::styled(format!("{mark} {text}"), if on { theme::bold() } else { theme::dim() })
}

/// The focused field carries the cursor; a secret is shown as dots however
/// long it is, so a shoulder cannot read its length either.
fn boxed(
    fields: &DispatchFields,
    which: Field,
    section_focused: bool,
    width: usize,
) -> Span<'static> {
    let raw = fields.read(which);
    let shown =
        if which.secret() && !raw.is_empty() { "********".to_owned() } else { clip(raw, width) };
    let here = section_focused && fields.focused() == which;
    let text = format!("[{shown:<width$}]");
    Span::styled(text, if here { theme::border_focused() } else { theme::border_dim() })
}

fn hints(fields: &DispatchFields) -> Vec<Span<'static>> {
    let submit = if fields.ready() { "dispatch" } else { "pick a dispatcher" };
    vec![
        Span::styled("  Tab ", theme::hotkey()),
        Span::styled("field  ", theme::hotkey_desc()),
        Span::styled("ctrl+d ", theme::hotkey()),
        Span::styled("dispatcher  ", theme::hotkey_desc()),
        Span::styled("ctrl+h ", theme::hotkey()),
        Span::styled("harness  ", theme::hotkey_desc()),
        Span::styled("ctrl+s ", theme::hotkey()),
        Span::styled(submit.to_owned(), theme::hotkey_desc()),
    ]
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::{HEIGHT, lines};
    use crate::app::dispatch::{DispatchFields, Field};

    fn text(fields: &DispatchFields, width: u16) -> String {
        lines(fields, true, width)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn filled() -> DispatchFields {
        let mut fields = DispatchFields::default();
        fields.dispatchers = vec!["k8s-cyberia".to_owned()];
        fields.form.dispatcher = "k8s-cyberia".to_owned();
        fields.form.repo = "cctui".to_owned();
        fields.form.ticket = "CCT-1102".to_owned();
        fields.form.timeout = "60".to_owned();
        fields.pack.url = "https://git/packs.git".to_owned();
        fields.pack.token = "hunter2".to_owned();
        fields
    }

    #[test]
    fn the_section_shows_the_fields_and_the_harness_radio() {
        let rendered = text(&filled(), 100);
        assert!(rendered.contains("k8s-cyberia"), "{rendered}");
        assert!(rendered.contains("cctui"), "{rendered}");
        assert!(rendered.contains("CCT-1102"), "{rendered}");
        assert!(rendered.contains("(*) claude"), "{rendered}");
        assert!(rendered.contains("( ) codex"), "{rendered}");
        assert!(rendered.contains("Context pack"), "{rendered}");
    }

    /// The token is a git credential: neither it nor its length is shown.
    #[test]
    fn the_token_is_masked() {
        let rendered = text(&filled(), 100);
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("********"), "{rendered}");
    }

    #[test]
    fn the_harness_radio_follows_the_adapter() {
        let mut fields = filled();
        fields.form.dispatch_adapter = "codex".to_owned();
        let rendered = text(&fields, 100);
        assert!(rendered.contains("( ) claude"), "{rendered}");
        assert!(rendered.contains("(*) codex"), "{rendered}");
    }

    #[test]
    fn the_hint_says_what_is_missing_until_a_dispatcher_is_picked() {
        assert!(text(&DispatchFields::default(), 100).contains("pick a dispatcher"));
        assert!(text(&filled(), 100).contains("ctrl+s dispatch"));
    }

    /// 80 columns is the budget every row in this TUI lives inside.
    #[test]
    fn no_row_overflows_eighty_columns() {
        let mut fields = filled();
        fields.form.prompt = "x".repeat(300);
        fields.pack.url = "y".repeat(300);
        for line in lines(&fields, true, 80) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(width <= 80, "a dispatch row overflowed: {width}");
        }
    }

    #[test]
    fn the_reserved_height_matches_what_is_drawn() {
        assert_eq!(lines(&filled(), true, 100).len(), HEIGHT as usize);
    }

    #[test]
    fn the_focused_field_is_the_one_the_cursor_is_on() {
        let mut fields = filled();
        fields.focus = Field::ORDER.iter().position(|f| *f == Field::Ticket).expect("a field");
        assert_eq!(fields.focused(), Field::Ticket);
    }
}
