//! The in-session `AskUserQuestion` and plan-approval cards.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::app::prompt::{AskCard, PLAN_OPTIONS, PlanCard};
use crate::theme;
use crate::ui::markdown_render;

const MIN_HEIGHT: usize = 4;

/// The card to draw at the transcript tail, already clipped to `max_height`.
/// Empty when the session has no live prompt.
pub fn card_lines(
    app: &App,
    session_id: &str,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if width < 8 || max_height < MIN_HEIGHT {
        return Vec::new();
    }
    let selected = app.selected_session_id().as_deref() == Some(session_id);
    let focused = selected && app.prompt_focus().is_some();
    if let Some(card) = app.asks.get(session_id) {
        return ask_lines(card, width, max_height, focused);
    }
    app.plans
        .get(session_id)
        .map(|card| plan_lines(card, width, max_height, focused))
        .unwrap_or_default()
}

fn border_style(focused: bool) -> Style {
    if focused { theme::border_focused() } else { theme::border_dim() }
}

fn top(title: &str, width: usize, focused: bool) -> Line<'static> {
    let head = format!("┌ {title} ");
    let fill = width.saturating_sub(head.chars().count() + 1);
    Line::from(Span::styled(format!("{head}{}┐", "─".repeat(fill)), border_style(focused)))
}

fn bottom(width: usize, focused: bool) -> Line<'static> {
    Line::from(Span::styled(
        format!("└{}┘", "─".repeat(width.saturating_sub(2))),
        border_style(focused),
    ))
}

fn divider(width: usize, focused: bool) -> Line<'static> {
    Line::from(Span::styled(
        format!("├{}┤", "─".repeat(width.saturating_sub(2))),
        border_style(focused),
    ))
}

/// One bordered row: the body is truncated, never wrapped, so the card's height
/// stays exactly what the layout reserved for it.
fn row(spans: Vec<Span<'static>>, width: usize, focused: bool) -> Line<'static> {
    let inner = width.saturating_sub(4);
    let mut used = 0;
    let mut out = vec![Span::styled("│ ", border_style(focused))];
    for span in spans {
        if used >= inner {
            break;
        }
        let room = inner - used;
        let text: String = span.content.chars().take(room).collect();
        used += text.chars().count();
        out.push(Span::styled(text, span.style));
    }
    out.push(Span::raw(" ".repeat(inner - used)));
    out.push(Span::styled(" │", border_style(focused)));
    Line::from(out)
}

fn hint(text: &str, width: usize, focused: bool) -> Line<'static> {
    row(vec![Span::styled(text.to_owned(), theme::dim())], width, focused)
}

/// The assistant prose leading up to the prompt, so the choice is not blind.
fn preamble_rows(preamble: Option<&str>, width: usize, focused: bool) -> Vec<Line<'static>> {
    preamble
        .map(|text| {
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| row(vec![Span::styled(l.to_owned(), theme::dim())], width, focused))
                .collect()
        })
        .unwrap_or_default()
}

fn ask_lines(card: &AskCard, width: usize, max_height: usize, focused: bool) -> Vec<Line<'static>> {
    let total = card.questions.len();
    let index = card.current.min(total.saturating_sub(1));
    let Some(question) = card.questions.get(index) else { return Vec::new() };

    let title = format!("Question {}/{total} ─ {}", index + 1, question.question);
    let mut body: Vec<Line<'static>> = preamble_rows(card.preamble.as_deref(), width, focused);
    if let Some(header) = &question.header {
        body.push(row(vec![Span::styled(header.clone(), theme::section_title())], width, focused));
    }
    for (oi, option) in question.options.iter().enumerate() {
        let picked = card.chosen[index].contains(&oi);
        let cursor = focused && !card.editing_other && card.cursor[index] == oi;
        let mut spans = vec![
            Span::styled(if cursor { "▸ " } else { "  " }.to_owned(), theme::hotkey()),
            Span::styled(
                if picked { "[x] ".to_owned() } else { "[ ] ".to_owned() },
                if picked { theme::active() } else { theme::dim() },
            ),
            Span::styled(format!("{} ", oi + 1), theme::hotkey()),
            Span::styled(option.label.clone(), if picked { theme::bold() } else { Style::new() }),
        ];
        if let Some(description) = &option.description {
            spans.push(Span::styled(format!("  ({description})"), theme::dim()));
        }
        body.push(row(spans, width, focused));
    }

    let other = &card.other[index];
    let editing = focused && card.editing_other;
    body.push(row(
        vec![
            Span::styled(if editing { "▸ " } else { "  " }.to_owned(), theme::hotkey()),
            Span::styled(
                if other.trim().is_empty() { "[ ] ".to_owned() } else { "[x] ".to_owned() },
                theme::dim(),
            ),
            Span::styled("o ".to_owned(), theme::hotkey()),
            Span::styled("Other: ".to_owned(), theme::dim()),
            Span::styled(if editing { format!("{other}_") } else { other.clone() }, Style::new()),
        ],
        width,
        focused,
    ));

    let footer = if editing {
        "type an answer  Enter accept  Esc clear"
    } else if card.answered_all() {
        "↑↓/1-9 pick  space toggle  o other  Tab next  Enter send  Esc later"
    } else {
        "↑↓/1-9 pick  space toggle  o other  Tab next  Esc later"
    };

    assemble(
        top(&title, width, focused),
        body,
        None,
        hint(footer, width, focused),
        width,
        focused,
        max_height,
    )
}

fn plan_lines(
    card: &PlanCard,
    width: usize,
    max_height: usize,
    focused: bool,
) -> Vec<Line<'static>> {
    let rendered = markdown_render::render_markdown_text(&card.plan);
    let source: Vec<Line<'static>> = if rendered.lines.is_empty() {
        card.plan.lines().map(|l| Line::from(Span::raw(l.to_owned()))).collect()
    } else {
        rendered
            .lines
            .iter()
            .map(|l| {
                Line::from(
                    l.spans
                        .iter()
                        .map(|s| Span::styled(s.content.to_string(), s.style))
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    };

    let mut body = preamble_rows(card.preamble.as_deref(), width, focused);
    body.extend(source.into_iter().skip(card.scroll).map(|line| row(line.spans, width, focused)));

    let footer = if card.refining {
        row(
            vec![
                Span::styled("refine: ".to_owned(), theme::hotkey()),
                Span::raw(format!("{}_", card.refine)),
                Span::styled("   Enter send  Esc cancel".to_owned(), theme::dim()),
            ],
            width,
            focused,
        )
    } else {
        let mut spans = Vec::new();
        for (index, label) in PLAN_OPTIONS.iter().enumerate() {
            spans.push(Span::styled(format!("{} ", index + 1), theme::hotkey()));
            spans.push(Span::raw(format!("{label}  ")));
        }
        spans.push(Span::styled("r ".to_owned(), theme::hotkey()));
        spans.push(Span::styled("Refine…  j/k scroll  Esc later".to_owned(), theme::dim()));
        row(spans, width, focused)
    };

    assemble(
        top("Plan ready for review", width, focused),
        body,
        Some(divider(width, focused)),
        footer,
        width,
        focused,
        max_height,
    )
}

/// `max_height` is what the transcript can spare, so the body is what gives.
fn assemble(
    top: Line<'static>,
    body: Vec<Line<'static>>,
    divider: Option<Line<'static>>,
    footer: Line<'static>,
    width: usize,
    focused: bool,
    max_height: usize,
) -> Vec<Line<'static>> {
    let chrome = 3 + usize::from(divider.is_some());
    let room = max_height.saturating_sub(chrome);
    let mut out = vec![top];
    out.extend(body.into_iter().take(room.max(1)));
    if let Some(divider) = divider {
        out.push(divider);
    }
    out.push(footer);
    out.push(bottom(width, focused));
    out
}
