//! The Bookmarks slice: the switcher's chrome around the saved-message list,
//! its search and the preview of the selected bookmark.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use cctui_clientcore::search::match_ranges;
use cctui_proto::api::bookmarks::Bookmark;

use crate::app::App;
use crate::app::bookmarks::{Prompt, age_label};
use crate::theme;
use crate::ui::markdown_render;

/// Fixed cells, so the meta and note columns line up down the list. Four for
/// the marker plus these fit the 80-column budget with room for a note.
const TITLE_WIDTH: usize = 32;
const META_WIDTH: usize = 18;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, tabs_area, body_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut status = vec![
        Span::styled(" cctui ", theme::status_bar_bg()),
        Span::raw(" "),
        Span::styled(format!("v{}", app.version), theme::dim()),
        Span::raw("  "),
    ];
    status.extend(crate::widgets::tabs::summary_spans(app, usize::from(status_area.width)));
    status.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(status)), status_area);

    frame.render_widget(
        Paragraph::new(crate::widgets::tabs::tab_line(app, usize::from(tabs_area.width))),
        tabs_area,
    );

    draw_body(frame, app, body_area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" 1-9 ", theme::hotkey()),
            Span::raw("Switch view  "),
            Span::styled("? ", theme::hotkey()),
            Span::raw("Help  "),
            Span::styled("q ", theme::hotkey()),
            Span::raw("Quit"),
        ])),
        hotkeys_area,
    );
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    let prompt_height =
        u16::from(app.bookmarks.prompt.is_some() || app.bookmarks.confirm.is_some());
    let [header_area, list_area, rule_area, preview_area, prompt_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Percentage(45),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(prompt_height),
    ])
    .areas(area);

    let terms = app.bookmarks.terms();
    let rows = app.bookmarks.visible();

    frame.render_widget(Paragraph::new(header(app, rows.len())), header_area);

    if rows.is_empty() {
        let text = if app.bookmarks.loading {
            " loading…"
        } else if app.bookmarks.query.is_empty() && terms.is_empty() {
            " nothing saved yet — bookmark a message from the web UI"
        } else {
            " no bookmark matches"
        };
        frame.render_widget(Paragraph::new(Span::styled(text, theme::dim())), list_area);
    } else {
        let height = list_area.height as usize;
        let first = app.bookmarks.selected.saturating_sub(height.saturating_sub(1));
        let width = list_area.width as usize;
        let lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(first)
            .take(height)
            .map(|(index, b)| row(b, index == app.bookmarks.selected, width, &terms, app.clock_ms))
            .collect();
        frame.render_widget(Paragraph::new(lines), list_area);
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(rule_area.width as usize),
            theme::border_dim(),
        ))),
        rule_area,
    );

    let preview = rows.get(app.bookmarks.selected).map_or_else(Vec::new, |b| preview_lines(b));
    let scroll = app.bookmarks.preview_scroll.min(preview.len().saturating_sub(1));
    frame.render_widget(
        Paragraph::new(preview.into_iter().skip(scroll).collect::<Vec<_>>())
            .wrap(Wrap { trim: false }),
        preview_area,
    );

    if prompt_height > 0 {
        frame.render_widget(Paragraph::new(prompt_line(app)), prompt_area);
    }
}

fn header(app: &App, shown: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(format!(" {shown} saved "), theme::header_bg())];
    if !app.bookmarks.query.is_empty() {
        spans.push(Span::styled(format!(" /{} ", app.bookmarks.query), theme::hotkey()));
    }
    if app.bookmarks.loading {
        spans.push(Span::styled("  …", theme::dim()));
    }
    Line::from(spans)
}

/// `◈ title            session · age   > note`, with the search terms picked
/// out of the title and the note.
fn row(b: &Bookmark, selected: bool, width: usize, terms: &[String], now_ms: i64) -> Line<'static> {
    let mut spans = vec![Span::styled(
        if selected { "❯ ◈ " } else { "  ◈ " },
        if selected { theme::border_focused() } else { theme::dim() },
    )];
    let dead = b.session_id.is_none();
    let title = if dead { format!("(dead) {}", b.title) } else { b.title.clone() };
    let title_style = if selected { theme::bold() } else { theme::active() };
    spans.extend(highlighted(&cell(&title, TITLE_WIDTH), terms, title_style));

    let meta = format!(
        "{} · {}",
        b.session_name.as_deref().unwrap_or("—"),
        age_label(b.created_at, now_ms)
    );
    spans.push(Span::styled(format!(" {}", cell(&meta, META_WIDTH)), theme::dim()));

    if let Some(note) = b.note.as_deref().filter(|n| !n.is_empty()) {
        let room = width.saturating_sub(TITLE_WIDTH + META_WIDTH + 8);
        spans.push(Span::styled(" > ", theme::dim()));
        spans.extend(highlighted(&clip(note, room), terms, theme::dim()));
    }
    Line::from(spans)
}

/// Split `text` so every term match gets the highlight style; the matcher is
/// the shared one, so a hit here is a hit in the web list.
fn highlighted(text: &str, terms: &[String], base: ratatui::style::Style) -> Vec<Span<'static>> {
    let ranges = match_ranges(text, terms);
    if ranges.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut at = 0;
    for (start, end) in ranges {
        let (start, end) = (start.min(chars.len()), end.min(chars.len()));
        if start > at {
            spans.push(Span::styled(chars[at..start].iter().collect::<String>(), base));
        }
        spans.push(Span::styled(chars[start..end].iter().collect::<String>(), theme::hotkey()));
        at = end;
    }
    if at < chars.len() {
        spans.push(Span::styled(chars[at..].iter().collect::<String>(), base));
    }
    spans
}

/// `text` clipped and padded to `width`, so every row's columns start at the
/// same offset whatever the title's length.
fn cell(text: &str, width: usize) -> String {
    let clipped = clip(text, width);
    let pad = width.saturating_sub(clipped.chars().count());
    format!("{clipped}{}", " ".repeat(pad))
}

fn clip(text: &str, width: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if width == 0 || flat.chars().count() <= width {
        return flat;
    }
    let head: String = flat.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

fn preview_lines(b: &Bookmark) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(format!("# {}", b.title), theme::bold()))];
    if let Some(note) = b.note.as_deref().filter(|n| !n.is_empty()) {
        lines.push(Line::from(Span::styled(format!("> {note}"), theme::dim())));
    }
    lines.push(Line::from(""));
    lines.extend(markdown_render::render_markdown_text(&b.body).lines);
    lines
}

fn prompt_line(app: &App) -> Line<'static> {
    if let Some(prompt) = app.bookmarks.prompt.as_ref() {
        let hint = match prompt {
            Prompt::Search { .. } => "  Enter search · Esc cancel",
            Prompt::Edit { .. } => "  Tab title/note · Enter save · Esc cancel",
        };
        return Line::from(vec![
            Span::styled(format!(" {} ", prompt.label()), theme::hotkey()),
            Span::raw(prompt.buffer().to_owned()),
            Span::styled("▏", theme::border_focused()),
            Span::styled(hint, theme::dim()),
        ]);
    }
    if app.bookmarks.confirm.is_some() {
        return Line::from(vec![
            Span::styled(" delete this bookmark? ", theme::error()),
            Span::styled("y", theme::hotkey()),
            Span::raw(" / "),
            Span::styled("n", theme::hotkey()),
        ]);
    }
    Line::from("")
}

#[cfg(test)]
mod tests {
    use super::{cell, clip, highlighted};
    use crate::theme;

    /// Snapshots drop styles, so the highlight is asserted on the spans.
    #[test]
    fn a_term_is_split_out_of_the_text_with_the_highlight_style() {
        let terms = vec!["auth".to_owned()];
        let spans = highlighted("the auth rollout", &terms, theme::dim());
        let texts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(texts, vec!["the ", "auth", " rollout"]);
        assert_eq!(spans[1].style, theme::hotkey(), "the match is the one picked out");
        assert_eq!(spans[0].style, theme::dim());
    }

    #[test]
    fn text_without_a_match_stays_one_span() {
        let spans = highlighted("nothing here", &["auth".to_owned()], theme::dim());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, theme::dim());
    }

    #[test]
    fn every_match_is_highlighted_case_insensitively() {
        let spans = highlighted("Auth and auth", &["auth".to_owned()], theme::dim());
        let marked: Vec<&str> = spans
            .iter()
            .filter(|s| s.style == theme::hotkey())
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(marked, vec!["Auth", "auth"], "the shared matcher ignores case");
    }

    #[test]
    fn a_cell_is_padded_and_a_long_one_is_clipped() {
        assert_eq!(cell("abc", 6), "abc   ");
        assert_eq!(cell("abcdefgh", 4), "abc…");
        assert_eq!(clip("  two   words ", 20), "two words");
        assert_eq!(clip("anything", 0), "anything", "no room means no clip");
    }
}
