use cctui_proto::api::SessionListItem;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::state::uptime_secs_at;
use crate::app::{App, session_list, session_live, session_status};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, tabs_area, list_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // Status bar
    draw_status_bar(frame, app, status_area);

    draw_title(frame, app, tabs_area);

    // Session list
    draw_session_list(frame, app, list_area);

    // One bottom line, in the order the keyboard resolves: a row-action prompt
    // is modal and owns it, then the search prompt, then the hotkeys.
    if !super::row_actions::draw_strip(frame, app, hotkeys_area) {
        if let Some(text) = app.spawn_drafts.strip() {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(ratatui::text::Line::from(
                    ratatui::text::Span::styled(text, crate::theme::attention()),
                )),
                hotkeys_area,
            );
        } else if app.list_search.is_active() {
            draw_search_prompt(frame, app, hotkeys_area);
        } else {
            crate::widgets::hotkeys::draw_session_hotkeys(frame, hotkeys_area, &app.config.keys);
        }
    }

    super::sections::draw(frame, app);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut spans = vec![
        Span::styled(" cctui ", theme::status_bar_bg()),
        Span::raw(" "),
        Span::styled(format!("v{}", app.version), theme::dim()),
        Span::raw("  "),
    ];
    spans.extend(crate::widgets::tabs::summary_spans(app, usize::from(area.width)));
    if app.ui.unread_only {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("unread only", theme::cost()));
    }
    if app.refresh.requested > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("⟳ {}/{}", app.refresh.sent, app.refresh.requested),
            theme::dim(),
        ));
    }
    spans.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The tab bar stands in for the static title, with the list's own shape after
/// it. On the title row, not the status bar: at 80 columns the status bar is
/// already carrying the attention chip, which outranks a shape summary.
fn draw_title(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let sections = crate::app::list_view::SECTIONS
        .iter()
        .filter(|s| app.list_shape.sections.has(**s))
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    let shape = format!("  {sections}  {}", app.list_shape.summary());
    let mut spans =
        crate::widgets::tabs::tab_spans(app, usize::from(area.width), shape.chars().count());
    spans.push(Span::styled(shape, theme::dim()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// `/ machine:cyberia "auth token"        [archived: off] 12 hits`
fn draw_search_prompt(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let search = &app.list_search;
    let mut spans =
        vec![Span::styled(" / ", theme::bold()), Span::styled(search.query.clone(), theme::bold())];
    if search.open {
        spans.push(Span::styled("▏", theme::border_focused()));
    }
    let archived = if search.include_archived { "on" } else { "off" };
    spans.push(Span::styled(format!("   [archived: {archived}]"), theme::dim()));
    if let Some(status) = search.status() {
        spans.push(Span::styled(format!("  {status}"), theme::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A result row: the session line, then its snippet on a dim second line with
/// the matched terms picked out.
fn result_items(app: &App, width: u16) -> Vec<ListItem<'static>> {
    let mut items = Vec::with_capacity(app.list_search.results.len() * 2);
    for s in &app.list_search.results {
        items.push(session_line(app, s, 0, width));
        if let Some(snippet) = s.match_snippet.as_deref().filter(|t| !t.trim().is_empty()) {
            items.push(ListItem::new(Line::from(snippet_spans(
                snippet,
                &app.list_search.terms,
                usize::from(width),
            ))));
        }
    }
    items
}

/// Dim snippet with the free-text terms highlighted, using the matcher the web
/// UI marks with so the two agree on what counts as a hit.
fn snippet_spans(snippet: &str, terms: &[String], width: usize) -> Vec<Span<'static>> {
    let flat = snippet.replace('\n', " ");
    let body: String = flat.chars().take(width.saturating_sub(6)).collect();
    let hits = cctui_clientcore::search::match_ranges(&body, terms);
    let chars: Vec<char> = body.chars().collect();
    let mut spans = vec![Span::styled("     …".to_owned(), theme::dim())];
    let mut at = 0;
    for (start, end) in hits {
        if start > at {
            spans.push(Span::styled(chars[at..start].iter().collect::<String>(), theme::dim()));
        }
        spans.push(Span::styled(chars[start..end].iter().collect::<String>(), theme::search_hit()));
        at = end;
    }
    if at < chars.len() {
        spans.push(Span::styled(chars[at..].iter().collect::<String>(), theme::dim()));
    }
    spans
}

fn draw_session_list(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    if app.list_search.is_active() {
        let items = result_items(app, content_width(area.width));
        if items.is_empty() {
            let text = app.list_search.status().unwrap_or_else(|| "type to search".to_owned());
            frame.render_widget(
                Paragraph::new(Span::styled(format!(" {text}"), theme::dim())),
                area,
            );
            return;
        }
        // Two lines per hit when it carries a snippet, so the cursor has to be
        // mapped onto the row the session itself drew.
        let selected = app
            .list_search
            .results
            .iter()
            .take(app.selected_index)
            .map(|s| {
                if s.match_snippet.as_deref().is_some_and(|t| !t.trim().is_empty()) { 2 } else { 1 }
            })
            .sum::<usize>();
        let offset = session_list::viewport_offset(items.len(), selected, area.height as usize);
        let list = List::new(items).highlight_style(theme::selected()).highlight_symbol(CURSOR);
        let mut state = ListState::default().with_offset(offset).with_selected(Some(selected));
        frame.render_stateful_widget(list, area, &mut state);
        return;
    }
    let rows = app.list_rows();
    let visible = session_list::sessions_of(&rows);
    let selected_flat = if app.selected_index < visible.len() { app.selected_index } else { 0 };
    let selected_row = session_list::selected_row(&rows, selected_flat);
    let offset = session_list::viewport_offset(rows.len(), selected_row, area.height as usize);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| match row {
            session_list::Row::Header { group, total, open } => group_header(*group, *total, *open),
            session_list::Row::DimHeader { key, total, open, machine_id } => {
                dim_header(app, key, *total, *open, machine_id.as_deref())
            }
            session_list::Row::SubHeader { label, total, running, open, depth, .. } => {
                sub_header(label, *total, *running, *open, *depth)
            }
            session_list::Row::Session { session, depth, .. } => {
                session_line(app, session, *depth, content_width(area.width))
            }
        })
        .collect();

    let list = List::new(items).highlight_style(theme::selected()).highlight_symbol(CURSOR);

    let mut state = ListState::default().with_offset(offset).with_selected(Some(selected_row));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The `List` widget reserves this on every row, selected or not, so a row's own
/// budget is the area minus its width.
const CURSOR: &str = "▸ ";

fn content_width(area_width: u16) -> u16 {
    area_width.saturating_sub(u16::try_from(CURSOR.chars().count()).unwrap_or(u16::MAX))
}

/// Folded and open arrows; the leftmost two columns belong to the selection
/// marker, so these sit inside the row.
const FOLDED: &str = "▸";
const OPEN: &str = "▾";

const fn arrow(open: bool) -> &'static str {
    if open { OPEN } else { FOLDED }
}

fn group_header(group: session_list::Group, total: usize, open: bool) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled(format!(" {} {} ", arrow(open), group.label()), theme::section_title()),
        Span::styled(format!("({total})"), theme::dim()),
    ]))
}

/// A machine that is not online is the thing worth noticing, so only those two
/// tiers get a loud colour.
/// `@host`, tinted by the machine's own hue when it has one, else by its name.
fn machine_seg(s: &SessionListItem) -> Seg {
    let name =
        s.machine_name.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| s.machine_id.clone());
    let hue = s
        .machine_hue
        .and_then(|h| u32::try_from(h).ok())
        .unwrap_or_else(|| cctui_clientcore::format::hash_hue(&name));
    Seg::new(5, theme::hue_style(hue), format!(" @{name}"))
}

/// A machine that is not online is the thing worth noticing, so only those two
/// tiers get a loud colour.
fn machine_style(tier: Option<cctui_proto::models::MachineLiveness>) -> ratatui::style::Style {
    match tier {
        Some(cctui_proto::models::MachineLiveness::Online) => theme::active(),
        Some(cctui_proto::models::MachineLiveness::Stale) => theme::stale(),
        Some(cctui_proto::models::MachineLiveness::Offline) => theme::error(),
        None => theme::dim(),
    }
}

const fn machine_word(tier: cctui_proto::models::MachineLiveness) -> &'static str {
    match tier {
        cctui_proto::models::MachineLiveness::Online => "online",
        cctui_proto::models::MachineLiveness::Stale => "stale",
        cctui_proto::models::MachineLiveness::Offline => "offline",
    }
}

/// Hue tinting a row under the active colour dimension, or `None` when the
/// dimension is off or the session carries nothing for it. A label uses the
/// label's own stored hue; every other key hashes, as the web UI does.
fn accent_hue(app: &App, s: &SessionListItem) -> Option<u32> {
    use crate::app::list_view::ColorBy;
    let key = app.list_shape.color_by.key_of(s)?;
    if app.list_shape.color_by == ColorBy::Label {
        let label = s.labels.first()?;
        return Some(cctui_clientcore::labels::label_hue(&label.name, &label.color));
    }
    Some(cctui_clientcore::format::hash_hue(&key))
}

/// Header of a group-by bucket. Tinted by the bucket's own hue when the accent
/// dimension matches the grouping, so the header and its rows read as one block.
fn dim_header(
    app: &App,
    key: &str,
    total: usize,
    open: bool,
    machine_id: Option<&str>,
) -> ListItem<'static> {
    let style = if app.list_shape.color_by == crate::app::list_view::ColorBy::None {
        theme::section_title()
    } else {
        theme::hue_style(cctui_clientcore::format::hash_hue(key))
    };
    let mut spans = vec![Span::styled(format!(" {} ", arrow(open)), theme::section_title())];
    if let Some(machine_id) = machine_id {
        // The machine's own liveness, not any session's: one dot coloured by the
        // tier, then the word, which is all a monochrome terminal has.
        let tier = session_live::machine_dot(app, machine_id);
        spans.push(Span::styled("● ", machine_style(tier)));
        spans.push(Span::styled(format!("{key} "), style));
        if let Some(tier) = tier {
            spans.push(Span::styled(format!("({}) ", machine_word(tier)), theme::dim()));
        }
    } else {
        spans.push(Span::styled(format!("{key} "), style));
    }
    spans.push(Span::styled(format!("({total})"), theme::dim()));
    ListItem::new(Line::from(spans))
}

/// `▸ subagents (2)` / `▾ wf: release-wave (4/9 running)`.
fn sub_header(
    label: &str,
    total: usize,
    running: usize,
    open: bool,
    depth: usize,
) -> ListItem<'static> {
    let count =
        if running > 0 { format!("({running}/{total} running)") } else { format!("({total})") };
    ListItem::new(Line::from(vec![
        Span::raw(indent(depth)),
        Span::styled(format!("{} {label} ", arrow(open)), theme::branch()),
        Span::styled(count, theme::dim()),
    ]))
}

/// Three columns for a top-level row, then two per nesting level.
fn indent(depth: usize) -> String {
    " ".repeat(3 + depth.saturating_sub(1) * 2)
}

/// One competing piece of a row. `priority` orders what goes when the row is
/// too narrow: the lowest survives the shortest.
struct Seg {
    text: String,
    style: ratatui::style::Style,
    priority: u8,
}

/// Segments at this priority are the row's identity and are never dropped.
const KEEP: u8 = u8::MAX;

impl Seg {
    const fn new(priority: u8, style: ratatui::style::Style, text: String) -> Self {
        Self { text, style, priority }
    }

    fn width(&self) -> usize {
        self.text.chars().count()
    }
}

fn total_width(segs: &[Seg]) -> usize {
    segs.iter().map(Seg::width).sum()
}

/// Drop the cheapest segments until the row fits `budget`, cheapest and
/// right-most first so the identity at the left survives.
fn shed(segs: &mut Vec<Seg>, budget: usize) {
    while total_width(segs) > budget {
        let Some(min) = segs.iter().map(|s| s.priority).filter(|p| *p != KEEP).min() else {
            return;
        };
        let Some(at) = segs.iter().rposition(|s| s.priority == min) else { return };
        segs.remove(at);
    }
}

fn spans_of(segs: Vec<Seg>) -> Vec<Span<'static>> {
    segs.into_iter().map(|s| Span::styled(s.text, s.style)).collect()
}

/// Empty outside select mode, so the row keeps its full width.
fn checkbox(app: &App, session_id: &str) -> String {
    super::row_actions::checkbox(app, session_id).unwrap_or_default().to_owned()
}

fn session_line(app: &App, s: &SessionListItem, depth: usize, width: u16) -> ListItem<'static> {
    ListItem::new(Line::from(session_line_spans(app, s, depth, width)))
}

/// `compact_rows` keeps a row to its identity — liveness, project, branch — and
/// drops the model, cost, cadence and activity column.
fn session_line_spans(
    app: &App,
    s: &SessionListItem,
    depth: usize,
    width: u16,
) -> Vec<Span<'static>> {
    let now = app.clock_ms;
    let stale = session_status::is_stale_working(s, now);
    let liveness = session_status::row_liveness(s, stale);
    let act = session_status::tool_activity(s, now);
    let badges = session_status::RowBadges::of(
        s,
        app.permissions.has(&s.id),
        app.prompt_marker(&s.id),
        app.soft_limited.contains(&s.id),
    );
    let compact = app.config.prefs.compact_rows;

    let project = s
        .metadata
        .get("project_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| basename(&s.working_dir));
    let branch = s.metadata.get("git_branch").and_then(serde_json::Value::as_str).unwrap_or("");
    let model = s.metadata.get("model").and_then(serde_json::Value::as_str).unwrap_or("");
    let adapter = s.adapter_id.as_ref().map_or("claude-code", |a| a.as_str());

    let is_subagent = depth > 0;
    let lead = if is_subagent { format!("{}↳ ", indent(depth + 1)) } else { indent(depth) };
    let mut segs = Vec::with_capacity(10);
    // One column, ahead of everything: the accent is the row's identity under a
    // colour dimension, so it sheds last.
    if let Some(hue) = accent_hue(app, s) {
        segs.push(Seg::new(KEEP, theme::hue_style(hue), "▏".to_owned()));
    }
    segs.extend([
        Seg::new(KEEP, theme::dim(), lead),
        Seg::new(KEEP, theme::hotkey(), checkbox(app, &s.id)),
        Seg::new(KEEP, theme::liveness_style(liveness), format!("{} ", liveness.glyph())),
        Seg::new(5, theme::dim(), format!("[{adapter}] ")),
        Seg::new(KEEP, if is_subagent { theme::dim() } else { theme::bold() }, project.to_owned()),
    ]);
    if !branch.is_empty() {
        segs.push(Seg::new(6, theme::branch(), format!(" ({branch})")));
    }
    if app.config.prefs.machine_column {
        segs.push(machine_seg(s));
    }
    // Only a machine that is not online earns a glyph; a dot on every row is
    // noise, and the row already says whether the session itself is live.
    if let Some(tier) = session_live::machine_dot(app, &s.machine_id)
        && tier != cctui_proto::models::MachineLiveness::Online
    {
        let tint = if tier == cctui_proto::models::MachineLiveness::Stale {
            theme::stale()
        } else {
            theme::error()
        };
        segs.push(Seg::new(7, tint, format!(" {}", session_live::machine_glyph(tier))));
    }

    if !compact {
        if !model.is_empty() {
            segs.push(Seg::new(2, theme::model(), format!("  {model}")));
        }
        segs.push(Seg::new(
            3,
            theme::dim(),
            format!("  {}", format_uptime(uptime_secs_at(s, now))),
        ));
        segs.push(Seg::new(1, theme::cost(), format!("  ${:.2}", s.token_usage.cost_usd)));
        if let Some(cadence) = session_status::cadence_text(&act) {
            segs.push(Seg::new(4, theme::dim(), format!("  {cadence}")));
        }
        for href in &s.pr_links {
            segs.push(Seg::new(0, theme::branch(), format!("  ⇄ {}", pr_ref(href))));
        }
    }

    let width = usize::from(width);
    let badge_text = badges.text();
    let glyph_cols = if badges.glyphs_empty() { 0 } else { badge_text.chars().count() + 2 };
    // Chips are reserved with the glyphs: they are the right-hand tail too, and
    // shedding has to know the whole of it before it decides what to drop.
    let chip_cols: usize = badges.labels.iter().map(|chip| chip.text.chars().count() + 1).sum();
    let tail_cols = glyph_cols + chip_cols;
    shed(&mut segs, width.saturating_sub(tail_cols));

    if !compact {
        let spare = width.saturating_sub(tail_cols + total_width(&segs));
        if spare >= MIN_ACTIVITY_COLS
            && let Some(text) = session_status::activity_text(s, &act, stale, now)
        {
            let tint = if stale || act.asleep { theme::stale() } else { theme::dim() };
            let text = session_status::truncate(&text, spare - 2);
            segs.push(Seg::new(KEEP, tint, format!("  {text}")));
        }
    }

    let mut spans = spans_of(segs);
    for chip in &badges.labels {
        spans.push(Span::raw(" "));
        let tint = chip.hue.map_or_else(theme::dim, theme::hue_style);
        spans.push(Span::styled(chip.text.clone(), tint));
    }
    if !badges.glyphs_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(badge_text, badge_style(&badges)));
    }
    clamp(spans, width)
}

/// Last defence: an identity segment alone (a very long project name) can still
/// outgrow the row, and a wrapped row would push every later row off by one.
fn clamp(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut left = width;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if left == 0 {
            break;
        }
        let cols = span.content.chars().count();
        if cols <= left {
            left -= cols;
            out.push(span);
            continue;
        }
        let cut = session_status::truncate(&span.content, left);
        left = 0;
        out.push(Span::styled(cut, span.style));
    }
    out
}

/// Below this there is no room for a phrase worth reading.
const MIN_ACTIVITY_COLS: usize = 8;

/// The loudest thing the cluster says wins its colour.
fn badge_style(badges: &session_status::RowBadges) -> ratatui::style::Style {
    if badges.wants_you() {
        return theme::attention();
    }
    if badges.unread > 0 {
        return theme::unread();
    }
    if badges.end.is_some() { theme::error() } else { theme::dim() }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Compact PR reference for a session row: `owner/repo#123` from a GitHub PR
/// URL, falling back to the raw href when the shape is unfamiliar.
fn pr_ref(href: &str) -> String {
    let tail = href.trim_end_matches('/');
    let parts: Vec<&str> = tail.split('/').collect();
    if let Some(pos) = parts.iter().position(|p| *p == "pull" || *p == "pulls")
        && let (Some(owner), Some(repo), Some(num)) = (
            pos.checked_sub(2).and_then(|i| parts.get(i)),
            pos.checked_sub(1).and_then(|i| parts.get(i)),
            parts.get(pos + 1),
        )
    {
        return format!("{owner}/{repo}#{num}");
    }
    tail.to_string()
}

fn format_uptime(secs: i64) -> String {
    if secs < 0 {
        return "?".to_string();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

// --- Event formatting (used by conversation view and main.rs) ---

pub fn format_tool_input(tool: &str, input: &serde_json::Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Agent" => "description",
        _ => "",
    };

    if !key.is_empty() {
        return input.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    }

    let s = serde_json::to_string(input).unwrap_or_default();
    if s.len() > 100 { format!("{}...", &s[..100]) } else { s }
}

#[cfg(test)]
mod tests {
    use super::{KEEP, Seg, session_line_spans, shed, total_width};
    use crate::app::App;
    use crate::testsupport::{session, subagent};
    use crate::theme;

    fn cols(spans: &[ratatui::text::Span<'static>]) -> usize {
        spans.iter().map(|s| s.content.chars().count()).sum()
    }

    fn text(spans: &[ratatui::text::Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn seg(priority: u8, body: &str) -> Seg {
        Seg::new(priority, theme::dim(), body.to_owned())
    }

    fn app_with(s: cctui_proto::api::SessionListItem) -> App {
        let mut app = App::new();
        app.sessions = vec![s];
        app.update_aggregates();
        app
    }

    #[test]
    fn shedding_drops_the_cheapest_segment_first_and_keeps_the_identity() {
        let mut segs = vec![seg(KEEP, "name"), seg(0, " cost"), seg(4, " age")];
        shed(&mut segs, 9);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].priority, 4);
        shed(&mut segs, 4);
        assert_eq!(segs.len(), 1);
        assert_eq!(total_width(&segs), 4);
    }

    #[test]
    fn shedding_stops_rather_than_dropping_an_identity_segment() {
        let mut segs = vec![seg(KEEP, "a very long project name")];
        shed(&mut segs, 4);
        assert_eq!(segs.len(), 1, "the clamp, not the shed, cuts what is left");
    }

    #[test]
    fn the_cursor_gutter_comes_out_of_the_row_budget() {
        assert_eq!(super::content_width(80), 78);
        assert_eq!(super::content_width(1), 0);
    }

    #[test]
    fn a_snippet_picks_out_the_terms_and_leaves_the_rest_dim() {
        let terms = vec!["auth".to_owned()];
        let spans = super::snippet_spans("refresh the auth token", &terms, 80);
        let hits: Vec<&str> = spans
            .iter()
            .filter(|s| s.style == theme::search_hit())
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(hits, ["auth"]);
        assert!(text(&spans).contains("refresh the auth token"));
        assert!(text(&spans).starts_with("     …"), "the snippet is indented under its row");
    }

    #[test]
    fn a_snippet_with_no_term_is_one_dim_run_and_is_clipped_to_the_row() {
        let plain = super::snippet_spans("nothing to see", &[], 80);
        assert!(plain.iter().all(|s| s.style != theme::search_hit()));

        let long = "x".repeat(200);
        let clipped = super::snippet_spans(&long, &[], 40);
        assert!(cols(&clipped) <= 40, "a long snippet must not overflow the row");
    }

    #[test]
    fn a_snippet_newline_does_not_break_the_row() {
        let spans = super::snippet_spans("first line\nsecond line", &[], 80);
        assert!(!text(&spans).contains('\n'));
    }

    #[test]
    fn an_accented_row_still_fits_and_leads_with_its_tint() {
        let mut s = session("s-long", "a-project-with-a-really-long-name", "active", "working");
        s.activity_detail = Some("Running an extremely long tool description".to_owned());
        s.tool_use_count = 140;
        let mut app = app_with(s);
        app.clock_ms = 600_000;
        app.list_shape.color_by = crate::app::list_view::ColorBy::Machine;

        for width in [40_u16, 60, 80, 100] {
            let spans = session_line_spans(&app, &app.sessions[0], 0, width);
            assert!(
                cols(&spans) <= usize::from(width),
                "{width} columns overflowed with an accent: {:?}",
                text(&spans)
            );
        }
        let spans = session_line_spans(&app, &app.sessions[0], 0, 100);
        assert_eq!(spans[0].content.as_ref(), "▏", "the accent leads the row");
        assert_ne!(spans[0].style, theme::dim(), "and carries the dimension's hue");
    }

    #[test]
    fn no_accent_without_a_colour_dimension() {
        let app = app_with(session("s-a", "alpha", "active", "working"));
        let spans = session_line_spans(&app, &app.sessions[0], 0, 100);
        assert_ne!(spans[0].content.as_ref(), "▏");
    }

    #[test]
    fn a_row_never_exceeds_eighty_columns() {
        let mut s = session("s-long", "a-project-with-a-really-long-name", "active", "working");
        s.metadata = serde_json::json!({
            "project_name": "a-project-with-a-really-long-name",
            "git_branch": "feature/an-extremely-long-branch-name-that-keeps-going",
            "model": "claude-opus-5-1m",
        });
        s.unread_count = 12;
        s.auto_approve = true;
        s.tool_use_count = 140;
        s.last_tool_at = Some(chrono::DateTime::from_timestamp_millis(1_000).expect("stamp"));
        s.activity_detail = Some("Running an extremely long tool description".to_owned());
        s.pr_links = vec!["https://github.com/DorskFR/cctui/pull/1234".to_owned()];
        s.end_reason = Some(cctui_proto::models::SessionEndReason::MachineOffline);
        let mut app = app_with(s);
        app.clock_ms = 600_000;

        for width in [40_u16, 60, 80, 100] {
            let spans = session_line_spans(&app, &app.sessions[0], 0, width);
            assert!(
                cols(&spans) <= usize::from(width),
                "{width} columns overflowed: {:?}",
                text(&spans)
            );
        }
    }

    #[test]
    fn a_row_in_select_mode_still_fits_eighty_columns() {
        let mut s = session("s-long", "a-project-with-a-really-long-name", "active", "working");
        s.metadata = serde_json::json!({
            "project_name": "a-project-with-a-really-long-name",
            "git_branch": "feature/an-extremely-long-branch-name-that-keeps-going",
            "model": "claude-opus-5-1m",
        });
        s.unread_count = 12;
        let mut app = app_with(s);
        app.clock_ms = 600_000;
        crate::app::reduce(
            &mut app,
            crate::app::Action::RowAction(crate::app::row_actions::RowAction::ToggleSelect),
        );

        for width in [40_u16, 80] {
            let spans = session_line_spans(&app, &app.sessions[0], 0, width);
            assert!(cols(&spans) <= usize::from(width), "{width} overflowed: {:?}", text(&spans));
            assert!(text(&spans).contains("[x] "), "the checkbox is part of the identity");
        }
    }

    #[test]
    fn the_badges_survive_a_narrow_row_even_when_everything_else_goes() {
        let mut s = session("s-narrow", "proj", "active", "blocked");
        s.unread_count = 4;
        let mut app = app_with(s);
        app.permissions.push(crate::app::PendingPermission {
            session_id: "s-narrow".to_owned(),
            request_id: "r".to_owned(),
            tool_name: "Bash".to_owned(),
            description: String::new(),
            input_preview: String::new(),
        });
        let spans = session_line_spans(&app, &app.sessions[0], 0, 30);
        let rendered = text(&spans);
        assert!(rendered.contains("! ●4"), "{rendered}");
    }

    #[test]
    fn a_stale_working_row_says_why_it_is_dim() {
        let mut s = session("s-stale", "proj", "active", "working");
        let now = 120 * 60 * 1000;
        s.last_heartbeat =
            Some(chrono::DateTime::from_timestamp_millis(now - 42 * 60 * 1000).expect("stamp"));
        let mut app = app_with(s);
        app.clock_ms = now;
        let rendered = text(&session_line_spans(&app, &app.sessions[0], 0, 100));
        assert!(rendered.contains('◐'), "{rendered}");
        assert!(rendered.contains("stale 42m"), "{rendered}");
    }

    #[test]
    fn a_nested_row_is_indented_one_step_per_level() {
        let app = app_with(subagent("s-child", "s-parent", "sub"));
        let one = text(&session_line_spans(&app, &app.sessions[0], 1, 100));
        let two = text(&session_line_spans(&app, &app.sessions[0], 2, 100));
        assert!(one.starts_with("     ↳ "), "{one}");
        assert!(two.starts_with("       ↳ "), "{two}");
        assert!(!text(&session_line_spans(&app, &app.sessions[0], 0, 100)).contains('↳'));
    }

    #[test]
    fn only_a_machine_that_is_not_online_marks_its_rows() {
        use cctui_proto::models::MachineLiveness;
        let mut app = app_with(session("s-off", "proj", "active", "working"));
        assert!(!text(&session_line_spans(&app, &app.sessions[0], 0, 100)).contains('✗'));
        app.machine_liveness.insert("orion".to_owned(), MachineLiveness::Online);
        assert!(!text(&session_line_spans(&app, &app.sessions[0], 0, 100)).contains('▪'));
        app.machine_liveness.insert("orion".to_owned(), MachineLiveness::Offline);
        let rendered = text(&session_line_spans(&app, &app.sessions[0], 0, 100));
        assert!(rendered.contains('✗'), "{rendered}");
    }

    #[test]
    fn compact_rows_drop_the_activity_column_but_not_the_badges() {
        let mut s = session("s-c", "proj", "active", "working");
        s.activity_detail = Some("Reading files".to_owned());
        s.unread_count = 2;
        let mut app = app_with(s);
        app.config.prefs.compact_rows = true;
        let rendered = text(&session_line_spans(&app, &app.sessions[0], 0, 100));
        assert!(!rendered.contains("Reading files"), "{rendered}");
        assert!(rendered.contains("●2"), "{rendered}");
    }
}
