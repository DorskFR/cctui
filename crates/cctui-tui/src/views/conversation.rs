use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::conversation_store::ConversationStore;
use crate::app::{
    App, ConversationLine, LineKind, LineStatus, ToolCategory, TurnFooter, send, transcript,
};
use crate::theme;
use crate::ui::{diff_render, markdown_render};

/// Width the vendored diff renderer is given inside the transcript.
const DIFF_WIDTH: usize = 120;

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub fn draw(frame: &mut Frame, app: &mut App) {
    let Some(session) = app.selected_session().cloned() else {
        frame.render_widget(Paragraph::new("No session selected"), frame.area());
        return;
    };

    let project = session
        .metadata
        .get("project_name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .or_else(|| session.working_dir.rsplit('/').find(|part| !part.is_empty()))
        .unwrap_or("unknown");
    let branch =
        session.metadata.get("git_branch").and_then(serde_json::Value::as_str).unwrap_or("");
    // `session.model` is what set-model writes, so it is what the header must
    // read; the spawn metadata is only the fallback for a row that has none.
    let model = session
        .model
        .as_deref()
        .filter(|m| !m.is_empty())
        .or_else(|| session.metadata.get("model").and_then(serde_json::Value::as_str))
        .unwrap_or("");
    let cost = cctui_clientcore::usage::money(session.token_usage.cost_usd);
    // The name as the info panel reads it: an id is the fallback, not the label.
    let machine = session
        .machine_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&session.machine_id);

    // The sidebar takes a column off the right and the vertical layout below
    // runs on what is left, so nothing else moves when it opens.
    let full_area = frame.area();
    let sidebar_open = super::sidebar::visible(app, full_area.width);
    let (main_area, sidebar_area) = if sidebar_open {
        let [main, side] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(crate::app::sidebar::WIDTH),
        ])
        .areas(full_area);
        (main, Some(side))
    } else {
        (full_area, None)
    };

    let input_lines = app.message_input.lines().len().max(1);
    let max_input = (main_area.height as usize / 2).max(1);
    let input_height = input_lines.clamp(1, 12_usize.min(max_input)) as u16;

    // One stack, one slot: `cards::card_lines` collects the permission cards
    // and the ask/plan card, so nothing reserves rows twice.
    let cards = super::cards::card_lines(
        app,
        &session.id,
        main_area.width as usize,
        (main_area.height as usize / 2).max(1),
    );
    let card_height = u16::try_from(cards.len()).unwrap_or(u16::MAX);

    let chip_height = super::attach::height(app, &session.id);

    let [header_area, content_area, card_area, separator_area, banner_area, chip_area, input_area] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(card_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(chip_height),
            Constraint::Length(input_height),
        ])
        .areas(main_area);

    if chip_height > 0 {
        super::attach::draw_chips(frame, app, &session.id, chip_area);
    }
    let popup_anchor = if chip_height > 0 { chip_area } else { input_area };

    if card_height > 0 {
        frame.render_widget(Paragraph::new(cards), card_area);
    }

    if let Some(area) = sidebar_area {
        super::sidebar::draw(frame, area, app, &session, app.view() == crate::app::View::Sidebar);
    }

    // Header
    let auto = if session.auto_approve { " ── ✓ auto-approve" } else { "" };
    let waiting = app.prompt_marker(&session.id).map_or_else(String::new, |m| format!(" ── {m}"));
    let pins = match app.pins.count(&session.id) {
        0 => String::new(),
        n => format!(" ── ⚑ {n}"),
    };
    let mode = crate::app::controls::permission_badge(&session)
        .map_or_else(String::new, |m| format!(" ── {m}"));
    let effort = session.effort.as_deref().filter(|e| !e.is_empty()).unwrap_or_default();
    let dials = if effort.is_empty() { model.to_owned() } else { format!("{model}·{effort}") };
    let header_text = if branch.is_empty() {
        format!(" {project} on {machine} ── {dials} ── {cost}{mode}{auto}{pins}{waiting}")
    } else {
        format!(
            " {project} ({branch}) on {machine} ── {dials} ── {cost}{mode}{auto}{pins}{waiting}"
        )
    };
    let mut header_spans = vec![Span::styled(header_text, theme::header_bg())];
    if let Some(cost) = super::spend::langfuse_span(app, &session.id) {
        header_spans.push(cost);
    }
    if let Some(filter) = app.filter.summary() {
        header_spans.push(Span::styled(format!(" ── ⛛ {filter}"), theme::dim()));
    }
    if let Some(hits) = app.find.position() {
        header_spans.push(Span::styled(format!(" ── /{} {hits}", app.find.query), theme::dim()));
    }
    header_spans.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header_area);

    // Conversation
    let pending = send::pending_lines(app, &session.id);
    let store = app.conversations.get(&session.id).filter(|s| !s.is_empty());
    if store.is_some() || !pending.is_empty() {
        let visible_height = content_area.height as usize;
        let entries = store.map_or(&[][..], ConversationStore::entries);
        let epoch = store.map_or(0, ConversationStore::epoch);

        // The cache may only be appended to: any entry that landed earlier than
        // the end bumps `epoch` and forces a rebuild.
        if app.render_cache_session != session.id
            || app.render_cache_epoch != epoch
            || app.render_cache_timestamps != app.show_timestamps
            || app.render_cache_pins != app.pins.epoch
            || app.render_cache_filter != app.filter.cache_key()
            || app.render_cache_entries > entries.len()
        {
            app.render_cache.clear();
            app.render_cache_starts.clear();
            app.render_cache_session.clone_from(&session.id);
            app.render_cache_epoch = epoch;
            app.render_cache_timestamps = app.show_timestamps;
            app.render_cache_pins = app.pins.epoch;
            app.render_cache_filter = app.filter.cache_key();
            app.render_cache_entries = 0;
        }
        if app.render_cache_entries < entries.len() {
            let pinned: Vec<bool> = entries[app.render_cache_entries..]
                .iter()
                .map(|entry| entry.sequenced && app.pins.pinned(&session.id, entry.seq))
                .collect();
            for (entry, pinned) in entries[app.render_cache_entries..].iter().zip(pinned) {
                // A filtered-out entry still takes a slot, so every other
                // index — the line cursor's included — keeps pointing at its
                // own line; it simply contributes no rows.
                app.render_cache_starts.push(app.render_cache.len());
                if !app.filter.visible(&entry.line) {
                    continue;
                }
                let opts =
                    RenderOpts { show_timestamps: app.show_timestamps, expanded: entry.expanded };
                let rows = render_line(&entry.line, opts);
                app.render_cache.extend(if pinned { with_pin_marker(rows) } else { rows });
            }
            app.render_cache_entries = entries.len();
        }

        // An optimistic send is never collapsible, so it needs no expand state.
        let pending_opts = RenderOpts { show_timestamps: app.show_timestamps, expanded: false };
        let pending_lines: Vec<Line<'static>> =
            pending.iter().flat_map(|line| render_line(line, pending_opts)).collect();

        let previous_total = app.total_display_lines;
        let total = app.render_cache.len() + pending_lines.len();
        app.viewport_height = visible_height;
        app.total_display_lines = total;

        // Older lines were prepended: hold the viewport on what the reader was
        // looking at rather than letting it slide down by the page's height.
        if std::mem::take(&mut app.pending_prepend) && !app.follow_tail {
            app.scroll_offset =
                app.scroll_offset.saturating_add(total.saturating_sub(previous_total));
        }

        let max_offset = total.saturating_sub(visible_height);
        let focus = focused_rows(
            app.line_cursor,
            entries.len(),
            &app.render_cache_starts,
            app.render_cache.len(),
        );
        if let Some((start, end)) = focus {
            // The cursor, not the tail, decides what is on screen in line-select.
            if start < app.scroll_offset {
                app.scroll_offset = start;
            } else if end > app.scroll_offset + visible_height {
                app.scroll_offset = end.saturating_sub(visible_height);
            }
        }
        let offset = if app.follow_tail { max_offset } else { app.scroll_offset.min(max_offset) };

        let mut display_lines: Vec<Line<'static>> = app
            .render_cache
            .iter()
            .chain(pending_lines.iter())
            .skip(offset)
            .take(visible_height)
            .cloned()
            .collect();
        if let Some((start, end)) = focus {
            for (row, line) in display_lines.iter_mut().enumerate() {
                let absolute = offset + row;
                if absolute >= start && absolute < end {
                    highlight(line);
                }
            }
        }
        if app.find.is_active() {
            for line in &mut display_lines {
                mark_hits(line, &app.find.terms);
            }
        }

        frame.render_widget(Paragraph::new(display_lines).wrap(Wrap { trim: false }), content_area);

        // Scrollbar overlay on right edge
        if total > visible_height {
            render_scrollbar(frame, content_area, offset, total, visible_height);
        }
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled("No conversation data", theme::dim())),
            content_area,
        );
    }

    // Separator, or the `/` `:` prompt while one is open.
    if let Some(mode) = app.cmdline.mode() {
        let prompt = format!("{}{}", mode.sigil(), app.cmdline.input);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prompt, theme::bold()),
                Span::styled("▏", theme::border_focused()),
            ])),
            separator_area,
        );
        draw_input_row(frame, app, input_area, popup_anchor, &session);
        super::filters::draw(frame, app);
        return;
    }
    let marker = if app.drafts.has_draft(&session.id) { " draft " } else { "" };
    let rule = (separator_area.width as usize).saturating_sub(marker.chars().count());
    let mut separator_spans = vec![Span::styled("─".repeat(rule), theme::border_focused())];
    if !marker.is_empty() {
        separator_spans.push(Span::styled(marker, theme::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(separator_spans)), separator_area);

    super::banner::draw(frame, banner_area, app, &session);
    draw_input_row(frame, app, input_area, popup_anchor, &session);
    super::filters::draw(frame, app);
}

/// The composer, or the "nothing more can be sent" notice for an ended session.
/// `popup_anchor` is the row the `#` popup sits above: the chip row when the
/// composer has attachments, else the composer itself. The chips are state the
/// user needs while typing a mention, so the popup must not cover them.
fn draw_input_row(
    frame: &mut Frame,
    app: &App,
    input_area: Rect,
    popup_anchor: Rect,
    session: &cctui_proto::api::SessionListItem,
) {
    if session.end_reason.is_some() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" this session has ended — ", theme::dim()),
                Span::styled("r", theme::hotkey()),
                Span::styled(" resumes it", theme::dim()),
            ])),
            input_area,
        );
        return;
    }

    // Input: [❯][textarea]
    let [prompt_area, textarea_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(2), Constraint::Fill(1)])
        .areas(input_area);

    let prompt_style = if app.input_active {
        theme::border_focused().add_modifier(Modifier::BOLD)
    } else {
        theme::border_dim()
    };
    frame.render_widget(Paragraph::new(Span::styled("❯", prompt_style)), prompt_area);

    let mut textarea_widget = app.message_input.clone();
    textarea_widget.set_block(Block::default());
    if !app.input_active {
        // Hide cursor when not in input mode.
        textarea_widget.set_cursor_style(Style::default());
        textarea_widget.set_cursor_line_style(Style::default());
    }
    frame.render_widget(&textarea_widget, textarea_area);

    if let Some(popup) = app.mentions.popup.as_ref() {
        crate::views::mentions::draw(frame, popup, popup_anchor);
    }
}

// -- Styles: muted/subdued palette --

// Role labels: soft, not shouting
const LABEL_YOU: Style = Style::new().fg(Color::Rgb(130, 170, 200)); // soft blue
const LABEL_ASSISTANT: Style = Style::new().fg(Color::Rgb(180, 140, 100)); // warm muted orange
const LABEL_PEER: Style = Style::new().fg(Color::Rgb(150, 190, 160)); // muted green
const THINKING: Style = Style::new().fg(Color::Rgb(120, 115, 150)); // dim violet

// Tool badges: dark background tints, light text — subtle not aggressive
const TOOL_READ: Style = Style::new().fg(Color::Rgb(140, 160, 180)).bg(Color::Rgb(30, 40, 55)); // slate
const TOOL_WRITE: Style = Style::new().fg(Color::Rgb(200, 180, 130)).bg(Color::Rgb(50, 45, 25)); // dark amber
const TOOL_MCP: Style = Style::new().fg(Color::Rgb(170, 140, 180)).bg(Color::Rgb(45, 30, 50)); // dark plum
const TOOL_SERVER: Style = Style::new().fg(Color::Rgb(150, 170, 150)).bg(Color::Rgb(30, 45, 35)); // dark moss
const TOOL_DETAIL: Style = Style::new().fg(Color::Rgb(100, 100, 100)); // muted gray
const TOOL_RESULT_STYLE: Style = Style::new().fg(Color::Rgb(90, 90, 90)); // dimmer gray
const ARROW: Style = Style::new().fg(Color::Rgb(80, 80, 80));
const HINT: Style = Style::new().fg(Color::Rgb(85, 85, 95));

/// Rows a collapsed result keeps before it hides the rest behind the hint.
const RESULT_PREVIEW_ROWS: usize = 1;

const fn tool_badge_style(category: ToolCategory) -> Style {
    match category {
        ToolCategory::Write => TOOL_WRITE,
        ToolCategory::Mcp => TOOL_MCP,
        ToolCategory::Server => TOOL_SERVER,
        ToolCategory::Read | ToolCategory::Other => TOOL_READ,
    }
}

fn collapse_hint(hidden: usize) -> Span<'static> {
    Span::styled(format!("  ({hidden} more · o)"), HINT)
}

/// Integer math on purpose: a float cast here trips `cast_precision_loss`.
fn format_tokens(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    format!("{}.{}k", value / 1_000, (value % 1_000) / 100)
}

fn format_duration_ms(ms: u64) -> String {
    let secs = ms / 1_000;
    if secs >= 60 { format!("{}m{:02}s", secs / 60, secs % 60) } else { format!("{secs}s") }
}

fn footer_text(footer: &TurnFooter) -> String {
    let mut parts = Vec::new();
    if let Some(ms) = footer.duration_ms {
        parts.push(format_duration_ms(ms));
    }
    match (footer.tokens_in, footer.tokens_out) {
        (Some(input), Some(output)) => {
            parts.push(format!("{} in / {} out", format_tokens(input), format_tokens(output)));
        }
        (Some(input), None) => parts.push(format!("{} in", format_tokens(input))),
        (None, Some(output)) => parts.push(format!("{} out", format_tokens(output))),
        (None, None) => {}
    }
    parts.join(" · ")
}

/// What the view needs beyond the line itself. New per-line decorations other
/// lanes add belong here rather than as another positional argument.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOpts {
    pub show_timestamps: bool,
    /// Only meaningful for a [`ConversationLine::collapsible`] line.
    pub expanded: bool,
}

fn markdown_lines(text: &str) -> Vec<Line<'static>> {
    let md_text = markdown_render::render_markdown_text(text);
    if md_text.lines.is_empty() {
        return text.lines().map(|l| Line::from(Span::raw(l.to_string()))).collect();
    }
    md_text
        .lines
        .iter()
        .map(|md_line| {
            Line::from(
                md_line
                    .spans
                    .iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn render_thinking(
    line: &ConversationLine,
    ts: String,
    redacted: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    if redacted {
        return vec![Line::from(vec![
            Span::raw(ts),
            Span::styled("∴ thinking (redacted)", THINKING),
        ])];
    }
    let body: Vec<&str> = line.text.lines().collect();
    let mut header =
        vec![Span::raw(ts), Span::styled(format!("∴ thinking ({} lines)", body.len()), THINKING)];
    if !expanded {
        header.push(collapse_hint(body.len()));
        return vec![Line::from(header)];
    }
    let mut out = vec![Line::from(header)];
    out.extend(
        body.iter().map(|l| {
            Line::from(Span::styled(format!("  {l}"), THINKING.add_modifier(Modifier::DIM)))
        }),
    );
    out
}

fn render_tool(line: &ConversationLine, ts: String, category: ToolCategory) -> Vec<Line<'static>> {
    let tool = line.tool.as_deref().unwrap_or_default();
    let mut out = vec![Line::from(vec![
        Span::raw(ts),
        Span::styled(
            format!(" {} ", transcript::display_tool_name(tool)),
            tool_badge_style(category),
        ),
        Span::raw(" "),
        Span::styled(line.text.clone(), TOOL_DETAIL),
        Span::styled(format!("  {}", category.as_str()), HINT),
    ])];
    if let Some(diff_lines) = line.tool_input.as_ref().and_then(|input| match tool {
        "Edit" => edit_diff(input, &line.text, DIFF_WIDTH),
        "Write" => write_diff(input, &line.text, DIFF_WIDTH),
        _ => None,
    }) {
        out.extend(diff_lines);
    }
    out
}

fn render_result(
    line: &ConversationLine,
    ts: String,
    error: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    let marker = if error { "└ ✗ " } else { "└ " };
    let marker_style = if error { theme::error() } else { ARROW };
    let body_style = if error { theme::error() } else { TOOL_RESULT_STYLE };

    if line.text.trim().is_empty() {
        return vec![Line::from(vec![
            Span::raw(ts),
            Span::styled(format!("{marker}(empty)"), marker_style),
        ])];
    }
    if looks_like_diff(&line.text) {
        let lang = detect_diff_lang(&line.text);
        let diff_lines = diff_render::render_unified_diff(&line.text, lang.as_deref(), DIFF_WIDTH);
        if diff_lines.is_empty() {
            return vec![Line::from(vec![
                Span::raw(ts),
                Span::styled(format!("{marker}(empty diff)"), marker_style),
            ])];
        }
        let mut header =
            vec![Span::raw(ts), Span::styled(marker.trim_end().to_owned(), marker_style)];
        if !expanded {
            header.push(collapse_hint(diff_lines.len()));
            return vec![Line::from(header)];
        }
        let mut out = vec![Line::from(header)];
        out.extend(diff_lines);
        return out;
    }

    let body: Vec<&str> = line.text.lines().collect();
    let shown = if expanded { body.len() } else { RESULT_PREVIEW_ROWS.min(body.len()) };
    let mut first = vec![
        Span::raw(ts),
        Span::styled(marker.to_owned(), marker_style),
        Span::styled(body[0].to_owned(), body_style),
    ];
    if shown < body.len() {
        first.push(collapse_hint(body.len() - shown));
    }
    let mut out = vec![Line::from(first)];
    out.extend(
        body[1..shown].iter().map(|l| Line::from(Span::styled(format!("    {l}"), body_style))),
    );
    out
}

fn render_peer(line: &ConversationLine, ts: String) -> Vec<Line<'static>> {
    let who = line.peer_from.as_deref().unwrap_or("peer");
    let label = line
        .peer_room
        .as_deref()
        .map_or_else(|| format!("◆ {who}"), |room| format!("◆ {who} in {room}"));
    let mut out =
        vec![Line::from(""), Line::from(vec![Span::raw(ts), Span::styled(label, LABEL_PEER)])];
    out.extend(markdown_lines(&line.text));
    out
}

/// Mark a pinned entry on its first row that carries text, so the blank
/// separators a turn starts with stay blank.
fn with_pin_marker(mut rows: Vec<Line<'static>>) -> Vec<Line<'static>> {
    if let Some(row) = rows.iter_mut().find(|row| !row.spans.is_empty()) {
        row.spans.insert(0, Span::styled("⚑ ", theme::hotkey()));
    }
    rows
}

/// Everything on screen for a line, so the render cache holds exactly that.
#[allow(clippy::too_many_lines)]
fn render_line(line: &ConversationLine, opts: RenderOpts) -> Vec<Line<'static>> {
    let ts = if opts.show_timestamps {
        format!("{} ", format_timestamp(line.timestamp))
    } else {
        String::new()
    };

    match line.kind {
        LineKind::User => {
            // Two blank lines before user message — clear turn separator
            let mut head = vec![Span::raw(ts), Span::styled("❯ You", LABEL_YOU)];
            head.extend(status_span(line.status.as_ref()));
            let mut out = vec![Line::from(""), Line::from(""), Line::from(head)];
            out.extend(line.text.lines().map(|text_line| {
                Line::from(Span::styled(
                    text_line.to_string(),
                    Style::default().fg(Color::Rgb(210, 210, 210)),
                ))
            }));
            out.push(Line::from(""));
            out
        }
        LineKind::Assistant => {
            let mut out = vec![
                Line::from(""),
                Line::from(vec![Span::raw(ts), Span::styled("● Assistant", LABEL_ASSISTANT)]),
            ];
            out.extend(markdown_lines(&line.text));
            if let Some(footer) = line.footer.as_ref().filter(|f| !f.is_empty()) {
                out.push(Line::from(Span::styled(format!("⏱ {}", footer_text(footer)), HINT)));
            }
            out
        }
        LineKind::Thinking { redacted } => render_thinking(line, ts, redacted, opts.expanded),
        LineKind::Tool { category } => render_tool(line, ts, category),
        LineKind::Result { error } => render_result(line, ts, error, opts.expanded),
        LineKind::Peer => render_peer(line, ts),
        LineKind::Image => super::images::transcript_lines(line, &ts),
        LineKind::Marker => {
            vec![Line::from(vec![
                Span::raw(ts),
                Span::styled(format!("· {}", line.text), theme::dim()),
            ])]
        }
        LineKind::Reset => {
            vec![Line::from(vec![
                Span::raw(ts),
                Span::styled(format!("⟳ {}", line.text), theme::dim()),
            ])]
        }
        LineKind::Compact => {
            let body = markdown_lines(&line.text);
            let mut header = vec![Span::raw(ts), Span::styled("⟳ context compacted", theme::dim())];
            if !opts.expanded {
                header.push(collapse_hint(body.len()));
                return vec![Line::from(header)];
            }
            let mut out = vec![Line::from(header)];
            out.extend(body);
            out
        }
        LineKind::Summary => {
            let detail = line.footer.as_ref().map(footer_text).unwrap_or_default();
            let text = if detail.is_empty() {
                format!("⏱ {}", line.text)
            } else if line.text.is_empty() {
                format!("⏱ {detail}")
            } else {
                format!("⏱ {detail} · {}", line.text)
            };
            let style = if line.footer.as_ref().is_some_and(|f| f.needs_action) {
                theme::error()
            } else {
                HINT
            };
            vec![Line::from(vec![Span::raw(ts), Span::styled(text, style)])]
        }
        LineKind::System => {
            if line.status == Some(LineStatus::Removed) {
                let body = line.text.lines().next().unwrap_or_default();
                let note = if body.is_empty() {
                    "⧗ removed from queue".to_owned()
                } else {
                    format!("⧗ removed from queue: {body}")
                };
                vec![Line::from(Span::styled(note, theme::dim()))]
            } else if line.text.is_empty() {
                Vec::new()
            } else {
                vec![Line::from(Span::styled(line.text.clone(), theme::dim()))]
            }
        }
        LineKind::Reply => {
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw(ts),
                    Span::styled("◁ Reply ", LABEL_ASSISTANT),
                    Span::raw(line.text.clone()),
                ]),
            ]
        }
    }
}

const CURSOR_BG: Color = Color::Rgb(38, 42, 52);
const HIT_STYLE: Style = Style::new().fg(Color::Rgb(20, 20, 24)).bg(Color::Rgb(210, 180, 90));

/// Repaints the search terms inside a row, splitting spans at the match
/// boundaries `cctui_clientcore` reports so the TUI and the webui agree on what
/// counts as a hit.
fn mark_hits(line: &mut Line<'static>, terms: &[String]) {
    if terms.is_empty() {
        return;
    }
    let mut out: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    for span in line.spans.drain(..) {
        let content = span.content.to_string();
        let hits = cctui_clientcore::search::match_ranges(&content, terms);
        if hits.is_empty() {
            out.push(span);
            continue;
        }
        let chars: Vec<char> = content.chars().collect();
        let mut at = 0;
        for (start, end) in hits {
            if start > at {
                out.push(Span::styled(chars[at..start].iter().collect::<String>(), span.style));
            }
            out.push(Span::styled(chars[start..end].iter().collect::<String>(), HIT_STYLE));
            at = end;
        }
        if at < chars.len() {
            out.push(Span::styled(chars[at..].iter().collect::<String>(), span.style));
        }
    }
    line.spans = out;
}

/// Tints a row without discarding the per-span colours the cache baked in.
fn highlight(line: &mut Line<'static>) {
    line.style = line.style.bg(CURSOR_BG);
    for span in &mut line.spans {
        span.style = span.style.bg(CURSOR_BG);
    }
}

/// Display rows the focused entry occupies, or `None` outside line-select.
///
/// Takes the pieces rather than `&App`: the caller still holds a borrow of the
/// conversation map. The cache may lag the store by a frame, so an index it has
/// not reached is treated as unfocused rather than clamped onto another entry.
fn focused_rows(
    cursor: Option<usize>,
    entry_count: usize,
    starts: &[usize],
    total: usize,
) -> Option<(usize, usize)> {
    let cursor = cursor?;
    if cursor >= entry_count || cursor >= starts.len() {
        return None;
    }
    let start = starts[cursor];
    let end = starts.get(cursor + 1).copied().unwrap_or(total);
    // A filtered-out entry renders no rows, so there is nothing to focus.
    (end > start).then_some((start, end))
}

/// The delivery or queue badge a user line carries, if any.
fn status_span(status: Option<&LineStatus>) -> Option<Span<'static>> {
    let (text, style) = match status? {
        LineStatus::Sending => ("  … sending".to_owned(), theme::dim()),
        LineStatus::Retrying { attempt, max } => {
            (format!("  ⟳ retrying ({attempt}/{max})"), theme::dim())
        }
        LineStatus::Delivered => ("  ✓ sent".to_owned(), theme::dim()),
        LineStatus::Failed(reason) => {
            (format!("  ✗ failed: {reason} — R retry  e edit  x drop"), theme::error())
        }
        LineStatus::Queued => ("  ⧗ queued".to_owned(), theme::dim()),
        LineStatus::Removed => ("  ⧗ removed from queue".to_owned(), theme::dim()),
    };
    Some(Span::styled(text, style))
}

/// Render a scrollbar overlay on the right edge of the content area.
fn render_scrollbar(frame: &mut Frame, area: Rect, offset: usize, total: usize, visible: usize) {
    if area.height == 0 || total == 0 {
        return;
    }
    let track_height = area.height as usize;
    let thumb_size = ((visible * track_height) / total).max(1).min(track_height);
    let max_offset = total.saturating_sub(visible);
    let thumb_top = (offset * (track_height - thumb_size)).checked_div(max_offset).unwrap_or(0);

    let rail_x = area.right().saturating_sub(1);
    let buf = frame.buffer_mut();
    for row in 0..track_height {
        let y = area.y + row as u16;
        if y >= area.bottom() || rail_x >= buf.area().right() {
            continue;
        }
        let in_thumb = row >= thumb_top && row < thumb_top + thumb_size;
        let cell = &mut buf[(rail_x, y)];
        if in_thumb {
            cell.set_char('▐');
            cell.set_style(Style::default().fg(Color::Rgb(80, 80, 90)));
        } else {
            cell.set_char('▕');
            cell.set_style(Style::default().fg(Color::Rgb(40, 40, 45)));
        }
    }
}

/// A diff from an `Edit` tool input's `old_string`/`new_string`.
pub fn edit_diff(
    input: &serde_json::Value,
    file_path: &str,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let old = input.get("old_string")?.as_str()?;
    let new = input.get("new_string")?.as_str()?;
    if old == new {
        return None;
    }

    let unified = diffy::DiffOptions::new()
        .set_context_len(2)
        .set_original_filename(format!("a/{file_path}"))
        .set_modified_filename(format!("b/{file_path}"))
        .create_patch(old, new)
        .to_string();

    if unified.is_empty() {
        return None;
    }

    let lang = file_path.rsplit('.').next();
    let lines = diff_render::render_unified_diff(&unified, lang, width);
    if lines.is_empty() { None } else { Some(lines) }
}

/// An all-add diff for a `Write` tool input's new file content.
pub fn write_diff(
    input: &serde_json::Value,
    file_path: &str,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let content = input.get("content")?.as_str()?;
    if content.is_empty() {
        return None;
    }
    let lang = file_path.rsplit('.').next();
    let lines =
        diff_render::render_full_file(content, diff_render::DiffLineKind::Insert, lang, width);
    if lines.is_empty() { None } else { Some(lines) }
}

/// Heuristic: does this text look like a unified diff?
fn looks_like_diff(text: &str) -> bool {
    let mut has_marker = false;
    for line in text.lines().take(10) {
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ") {
            has_marker = true;
            break;
        }
    }
    has_marker
}

/// Try to detect the language from diff header lines (e.g. `--- a/foo.rs`).
fn detect_diff_lang(text: &str) -> Option<String> {
    for line in text.lines().take(5) {
        if let Some(path) = line.strip_prefix("+++ b/").or_else(|| line.strip_prefix("+++ "))
            && let Some(dot) = path.rfind('.')
        {
            return Some(path[dot + 1..].to_string());
        }
    }
    None
}

fn format_timestamp(ts: i64) -> String {
    if ts == 0 {
        return "     ".to_string();
    }
    chrono::DateTime::from_timestamp(ts, 0).map_or_else(
        || "??:??".to_string(),
        |dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::{RenderOpts, render_line};
    use crate::app::{ConversationLine, LineKind, ToolCategory, TurnFooter};

    fn rows(line: &ConversationLine, expanded: bool) -> Vec<String> {
        render_line(line, RenderOpts { show_timestamps: false, expanded })
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    fn result(error: bool, text: &str) -> ConversationLine {
        let mut line = ConversationLine::new(LineKind::Result { error }, text, 0);
        line.tool = Some("Bash".to_owned());
        line
    }

    #[test]
    fn a_collapsed_result_keeps_one_row_and_says_what_it_hid() {
        let line = result(false, "120 lines\nsecond\nthird");
        let collapsed = rows(&line, false);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].contains("120 lines"));
        assert!(collapsed[0].contains("2 more"), "{collapsed:?}");

        let expanded = rows(&line, true);
        assert_eq!(expanded.len(), 3);
        assert!(!expanded[0].contains("more"));
    }

    #[test]
    fn a_failing_result_is_marked_and_a_short_one_needs_no_hint() {
        let rendered = rows(&result(true, "exit 101 · 3 failed"), false);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains('✗'), "{rendered:?}");
        assert!(!rendered[0].contains("more"));
    }

    #[test]
    fn thinking_collapses_to_a_single_summary_row() {
        let line =
            ConversationLine::new(LineKind::Thinking { redacted: false }, "one\ntwo\nthree", 0);
        let collapsed = rows(&line, false);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].contains("thinking (3 lines)"), "{collapsed:?}");
        assert_eq!(rows(&line, true).len(), 4);
    }

    #[test]
    fn redacted_thinking_has_no_body_to_expand() {
        let line = ConversationLine::new(LineKind::Thinking { redacted: true }, "\u{fffd}", 0);
        assert_eq!(rows(&line, true).len(), 1);
        assert!(rows(&line, true)[0].contains("redacted"));
    }

    #[test]
    fn a_tool_row_carries_its_name_detail_and_category_badge() {
        let mut line =
            ConversationLine::new(LineKind::Tool { category: ToolCategory::Read }, "src/a.rs", 0);
        line.tool = Some("Read".to_owned());
        let rendered = rows(&line, false);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains("Read"));
        assert!(rendered[0].contains("src/a.rs"));
        assert!(rendered[0].contains("read"));
    }

    #[test]
    fn an_mcp_tool_row_shows_the_short_name() {
        let mut line =
            ConversationLine::new(LineKind::Tool { category: ToolCategory::Mcp }, "{}", 0);
        line.tool = Some("mcp__cctui__CctuiUsage".to_owned());
        assert!(rows(&line, false)[0].contains("cctui:CctuiUsage"));
    }

    #[test]
    fn an_assistant_turn_with_usage_gets_a_footer_row() {
        let mut line = ConversationLine::new(LineKind::Assistant, "done", 0);
        line.footer = Some(TurnFooter {
            duration_ms: None,
            tokens_in: Some(12_400),
            tokens_out: Some(1_100),
            needs_action: false,
        });
        let rendered = rows(&line, false);
        let footer = rendered.last().expect("a footer row");
        assert!(footer.contains("12.4k in"), "{rendered:?}");
        assert!(footer.contains("1.1k out"));
    }

    #[test]
    fn a_duration_only_summary_renders_as_a_clock_row() {
        let mut line = ConversationLine::new(LineKind::Summary, String::new(), 0);
        line.footer = Some(TurnFooter { duration_ms: Some(98_000), ..TurnFooter::default() });
        let rendered = rows(&line, false);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains("1m38s"), "{rendered:?}");
    }

    #[test]
    fn a_peer_line_names_its_sender_and_room() {
        let mut line = ConversationLine::new(LineKind::Peer, "rebased", 0);
        line.peer_from = Some("lane-b".to_owned());
        line.peer_room = Some("wave-3".to_owned());
        let rendered = rows(&line, false);
        assert!(rendered.iter().any(|r| r.contains("lane-b in wave-3")), "{rendered:?}");
        assert!(rendered.iter().any(|r| r.contains("rebased")));
    }

    #[test]
    fn an_empty_system_line_renders_nothing() {
        let line = ConversationLine::new(LineKind::System, String::new(), 0);
        assert!(rows(&line, false).is_empty());
    }
}

#[cfg(test)]
mod search_tests {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};

    use super::{HIT_STYLE, mark_hits};

    fn row(parts: &[&str]) -> Line<'static> {
        Line::from(parts.iter().map(|p| Span::raw((*p).to_string())).collect::<Vec<_>>())
    }

    fn painted(line: &Line<'static>) -> Vec<(String, bool)> {
        line.spans.iter().map(|s| (s.content.to_string(), s.style == HIT_STYLE)).collect()
    }

    #[test]
    fn a_term_is_repainted_and_the_rest_of_the_span_is_left_alone() {
        let mut line = row(&["the parser is done"]);
        mark_hits(&mut line, &["parser".to_owned()]);
        assert_eq!(
            painted(&line),
            [
                ("the ".to_owned(), false),
                ("parser".to_owned(), true),
                (" is done".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_hit_at_either_edge_does_not_produce_an_empty_span() {
        let mut line = row(&["parser"]);
        mark_hits(&mut line, &["parser".to_owned()]);
        assert_eq!(painted(&line), [("parser".to_owned(), true)]);

        let mut trailing = row(&["a parser"]);
        mark_hits(&mut trailing, &["parser".to_owned()]);
        assert_eq!(painted(&trailing), [("a ".to_owned(), false), ("parser".to_owned(), true)]);
    }

    #[test]
    fn every_span_of_a_row_is_searched_and_its_own_style_survives() {
        let styled = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
        let mut line = Line::from(vec![
            Span::styled("parse ".to_string(), styled),
            Span::raw("and parse again".to_string()),
        ]);
        mark_hits(&mut line, &["parse".to_owned()]);
        let hits = line.spans.iter().filter(|s| s.style == HIT_STYLE).count();
        assert_eq!(hits, 2, "both spans are marked");
        assert!(
            line.spans.iter().any(|s| s.style == styled && s.content == " "),
            "the unmatched tail keeps the span's own style"
        );
    }

    #[test]
    fn a_row_with_no_hit_and_an_empty_query_are_both_left_untouched() {
        let before = row(&["nothing here"]);
        let mut line = before.clone();
        mark_hits(&mut line, &["parser".to_owned()]);
        assert_eq!(painted(&line), painted(&before));

        let mut empty_query = before.clone();
        mark_hits(&mut empty_query, &[]);
        assert_eq!(painted(&empty_query), painted(&before));
    }

    #[test]
    fn marking_is_case_insensitive_but_keeps_the_original_casing() {
        let mut line = row(&["Parser and parser"]);
        mark_hits(&mut line, &["PARSER".to_owned()]);
        let marked: Vec<String> = line
            .spans
            .iter()
            .filter(|s| s.style == HIT_STYLE)
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(marked, ["Parser", "parser"]);
    }
}
