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

    // Tab bar: which slice `1-9` is on, in place of a static title.
    frame.render_widget(Paragraph::new(crate::widgets::tabs::tab_line(app)), tabs_area);

    // Session list
    draw_session_list(frame, app, list_area);

    // Hotkeys
    crate::widgets::hotkeys::draw_session_hotkeys(frame, hotkeys_area, &app.config.keys);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut spans = vec![
        Span::styled(" cctui ", theme::status_bar_bg()),
        Span::raw(" "),
        Span::styled(format!("v{}", app.version), theme::dim()),
        Span::raw("  "),
    ];
    spans.extend(crate::widgets::tabs::summary_spans(app, usize::from(area.width)));
    if let Some(filter) = app.startup_filter.as_deref() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("filter {filter}"), theme::branch()));
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

fn draw_session_list(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let rows = app.list_rows();
    let visible = session_list::sessions_of(&rows);
    let selected_flat = if app.selected_index < visible.len() { app.selected_index } else { 0 };
    let selected_row = session_list::selected_row(&rows, selected_flat);
    let offset = session_list::viewport_offset(rows.len(), selected_row, area.height as usize);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| match row {
            session_list::Row::Header { group, total, open } => group_header(*group, *total, *open),
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
    let mut segs = vec![
        Seg::new(KEEP, theme::dim(), lead),
        Seg::new(KEEP, theme::liveness_style(liveness), format!("{} ", liveness.glyph())),
        Seg::new(5, theme::dim(), format!("[{adapter}] ")),
        Seg::new(KEEP, if is_subagent { theme::dim() } else { theme::bold() }, project.to_owned()),
    ];
    if !branch.is_empty() {
        segs.push(Seg::new(6, theme::branch(), format!(" ({branch})")));
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
    let badge_cols = if badges.is_empty() { 0 } else { badge_text.chars().count() + 2 };
    shed(&mut segs, width.saturating_sub(badge_cols));

    if !compact {
        let spare = width.saturating_sub(badge_cols + total_width(&segs));
        if spare >= MIN_ACTIVITY_COLS
            && let Some(text) = session_status::activity_text(s, &act, stale, now)
        {
            let tint = if stale || act.asleep { theme::stale() } else { theme::dim() };
            let text = session_status::truncate(&text, spare - 2);
            segs.push(Seg::new(KEEP, tint, format!("  {text}")));
        }
    }

    let mut spans = spans_of(segs);
    if !badges.is_empty() {
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
