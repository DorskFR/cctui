//! The top-level view switcher: which slice of the app `1-9` selects, the
//! summary every slice carries, and the Overview tiles.
//!
//! A slice is a *root*, not an overlay: switching resets the router rather
//! than pushing, so each slice keeps its own cursor and the overlay stack
//! never spans two of them.

use cctui_clientcore::spend::SessionSpend;
use cctui_proto::api::SessionStats;
use cctui_proto::models::MachineLiveness;

use super::action::Effect;
use super::state::{App, View};

/// A slice the TUI can show, in tab order. The number is the `1-9` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slice {
    Sessions,
    Overview,
    Machines,
}

impl Slice {
    /// The root view the slice resets the router to.
    pub const fn root(self) -> View {
        match self {
            Self::Sessions => View::SessionList,
            Self::Overview => View::Overview,
            Self::Machines => View::Machines,
        }
    }
}

/// One tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tab {
    pub label: &'static str,
    pub slice: Slice,
}

const fn tab(label: &'static str, slice: Slice) -> Tab {
    Tab { label, slice }
}

/// The tab bar. Administration lives in the web UI; the TUI keeps the
/// operator's slices.
pub const TABS: &[Tab] = &[
    tab("Sessions", Slice::Sessions),
    tab("Overview", Slice::Overview),
    tab("Machines", Slice::Machines),
];

/// Where a slice's cursor was when it was last left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub selected_index: usize,
    pub scroll_offset: usize,
}

/// The figures the summary line carries, mirroring the web UI's overview tiles.
///
/// `needs_input` is read by the Overview tile only: on the status line the
/// count belongs to the attention chip, which also carries the jump key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub live: i64,
    pub needs_input: i64,
    pub unread: u32,
    pub machines_online: usize,
    pub machines_total: usize,
    pub today_cost_usd: f64,
    /// No stats reply yet: the counts are the locally derived fallback.
    pub pending: bool,
}

/// Online over total. The fetched machine list is authoritative once it has
/// arrived; before that the live tiers the socket reported are all there is.
#[must_use]
pub fn machines(app: &App) -> (usize, usize) {
    if app.machines.loaded {
        return app.machines.counts();
    }
    let total = app.machine_liveness.len();
    let online =
        app.machine_liveness.values().filter(|tier| **tier == MachineLiveness::Online).count();
    (online, total)
}

/// Cost booked against sessions registered since local midnight.
///
/// This is what the session list can answer without another fetch: the
/// lifetime cost of today's sessions, not spend incurred today on older ones.
/// The per-day dollars live in `/sessions/stats/usage` buckets.
#[must_use]
pub fn today_cost_usd(app: &App) -> f64 {
    let Some(cutoff) = local_midnight_ms(app.clock_ms) else { return 0.0 };
    let sessions: Vec<SessionSpend> = app
        .sessions
        .iter()
        .map(|s| SessionSpend {
            model: s.model.clone(),
            registered_at_ms: s.registered_at.map(|at| at.timestamp_millis()),
            cost_usd: s.token_usage.cost_usd,
            tokens: s.token_usage.tokens_in + s.token_usage.tokens_out,
        })
        .collect();
    cctui_clientcore::spend::spend_since(&sessions, cutoff)
}

/// Local midnight preceding `at_ms`, in unix ms.
#[must_use]
pub fn local_midnight_ms(at_ms: i64) -> Option<i64> {
    use chrono::TimeZone;
    let at = chrono::DateTime::from_timestamp_millis(at_ms)?.with_timezone(&chrono::Local);
    let date = at.date_naive().and_hms_opt(0, 0, 0)?;
    chrono::Local.from_local_datetime(&date).single().map(|dt| dt.timestamp_millis())
}

#[must_use]
pub fn summary(app: &App) -> Summary {
    let (machines_online, machines_total) = machines(app);
    let waiting = i64::try_from(super::attention::waiting_count(app)).unwrap_or(i64::MAX);
    let shared = Summary {
        live: 0,
        needs_input: waiting,
        unread: super::unread::total(app),
        machines_online,
        machines_total,
        today_cost_usd: today_cost_usd(app),
        pending: true,
    };
    app.stats.as_ref().map_or_else(
        || Summary { live: i64::try_from(app.active_count).unwrap_or(i64::MAX), ..shared },
        |stats| Summary { live: stats.live, pending: false, ..shared },
    )
}

/// The IANA zone the server's local-calendar counts need.
///
/// `TZ` first, then the `/etc/localtime` symlink, which is the only portable
/// place a zone *name* survives — `chrono::Local` keeps the offset, not the name.
#[must_use]
pub fn local_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.is_empty()
    {
        return tz.trim_start_matches(':').to_owned();
    }
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|path| zone_from_path(&path.to_string_lossy()))
        .unwrap_or_else(|| "UTC".to_owned())
}

/// `/usr/share/zoneinfo/Europe/Paris` -> `Europe/Paris`.
fn zone_from_path(path: &str) -> Option<String> {
    let (_, zone) = path.split_once("zoneinfo/")?;
    (!zone.is_empty()).then(|| zone.to_owned())
}

pub enum SliceAction {
    /// A `1-9` press: the tab's number, 1-based as shown.
    Switch(usize),
    OverviewScroll(i32),
    Refresh,
    StatsLoaded(Box<SessionStats>),
    StatsFailed,
}

pub fn reduce_slice(app: &mut App, action: SliceAction) -> Vec<Effect> {
    match action {
        SliceAction::Switch(number) => switch(app, number),
        SliceAction::Refresh => vec![Effect::FetchSessionStats],
        SliceAction::OverviewScroll(lines) => {
            app.overview_scroll = if lines < 0 {
                app.overview_scroll.saturating_sub(lines.unsigned_abs() as usize)
            } else {
                app.overview_scroll.saturating_add(lines as usize)
            };
            Vec::new()
        }
        SliceAction::StatsLoaded(stats) => {
            app.stats = Some(*stats);
            Vec::new()
        }
        // The summary falls back to the locally derived counts, so a failed
        // fetch costs precision, not the line.
        SliceAction::StatsFailed => Vec::new(),
    }
}

fn switch(app: &mut App, number: usize) -> Vec<Effect> {
    let Some(tab) = number.checked_sub(1).and_then(|i| TABS.get(i)) else { return Vec::new() };
    go_to(app, tab.slice)
}

/// Switch to `target`, keeping each slice's cursor where it was. The one path a
/// slice is entered by, whether a tab number or a slice's own key did it.
pub fn go_to(app: &mut App, target: Slice) -> Vec<Effect> {
    if target == app.slice {
        return Vec::new();
    }
    app.slice_cursors.insert(
        app.slice,
        Cursor { selected_index: app.selected_index, scroll_offset: app.scroll_offset },
    );
    app.slice = target;
    let cursor = app.slice_cursors.get(&target).copied().unwrap_or_default();
    app.selected_index = cursor.selected_index;
    app.scroll_offset = cursor.scroll_offset;
    app.router.reset(target.root());
    match target {
        Slice::Overview => vec![Effect::FetchSessionStats],
        Slice::Machines => super::machines::on_enter(app),
        Slice::Sessions => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::SessionStats;
    use cctui_proto::models::{Attention, MachineLiveness};

    use super::{
        Slice, SliceAction, TABS, local_midnight_ms, machines, summary, today_cost_usd,
        zone_from_path,
    };
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::{CLOCK_MS, ms_ago, session};

    fn app() -> App {
        let mut app = App::new();
        app.clock_ms = CLOCK_MS;
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "blocked"),
        ];
        app.update_aggregates();
        app
    }

    fn dispatch(app: &mut App, action: SliceAction) -> Vec<Effect> {
        reduce(app, Action::Slice(action))
    }

    fn stats() -> SessionStats {
        SessionStats {
            total: 12,
            live: 3,
            needs_input: 2,
            archived: 4,
            today: 5,
            yesterday: 1,
            week: 9,
            month: 11,
        }
    }

    #[test]
    fn the_tab_numbers_are_stable() {
        let slices: Vec<Slice> = TABS.iter().map(|t| t.slice).collect();
        assert_eq!(slices, [Slice::Sessions, Slice::Overview, Slice::Machines]);
    }

    #[test]
    fn switching_resets_the_router_to_the_slices_root() {
        let mut app = app();
        assert_eq!(app.slice, Slice::Sessions);
        let effects = dispatch(&mut app, SliceAction::Switch(2));
        assert!(matches!(effects.as_slice(), [Effect::FetchSessionStats]));
        assert_eq!(app.slice, Slice::Overview);
        assert_eq!(app.view(), View::Overview);
        assert_eq!(app.view(), app.slice.root());

        dispatch(&mut app, SliceAction::Switch(3));
        assert_eq!(app.view(), View::Machines);
        dispatch(&mut app, SliceAction::Switch(1));
        assert_eq!(app.view(), View::SessionList);
    }

    /// A tab number is the same door as the slice's own key: both must load it,
    /// or the view sits on "loading…" for ever.
    #[test]
    fn entering_a_slice_by_number_loads_it_like_its_own_key() {
        for (number, slice) in [(2, Slice::Overview), (3, Slice::Machines)] {
            let mut by_number = app();
            let numbered = dispatch(&mut by_number, SliceAction::Switch(number));
            assert_eq!(by_number.slice, slice, "tab {number} must reach {slice:?}");
            assert!(
                !numbered.is_empty(),
                "tab {number} entered {slice:?} without asking for its data"
            );

            let mut by_key = app();
            let keyed = super::go_to(&mut by_key, slice);
            assert_eq!(
                numbered.len(),
                keyed.len(),
                "tab {number} and {slice:?}'s own key must ask for the same work"
            );
        }
    }

    /// A second visit must not refetch what is already loaded.
    #[test]
    fn re_entering_a_loaded_slice_asks_for_nothing() {
        let mut app = app();
        assert!(!super::go_to(&mut app, Slice::Machines).is_empty());
        app.machines.loaded = true;
        super::go_to(&mut app, Slice::Sessions);
        assert!(super::go_to(&mut app, Slice::Machines).is_empty());
    }

    #[test]
    fn switching_away_and_back_keeps_each_slices_cursor() {
        let mut app = app();
        app.selected_index = 1;
        app.scroll_offset = 7;

        dispatch(&mut app, SliceAction::Switch(2));
        assert_eq!((app.selected_index, app.scroll_offset), (0, 0), "a fresh slice starts at top");
        app.selected_index = 4;
        app.scroll_offset = 2;

        dispatch(&mut app, SliceAction::Switch(1));
        assert_eq!((app.selected_index, app.scroll_offset), (1, 7), "the list cursor came back");
        dispatch(&mut app, SliceAction::Switch(2));
        assert_eq!((app.selected_index, app.scroll_offset), (4, 2), "so did the overview's");
    }

    #[test]
    fn an_overlay_opened_over_a_slice_does_not_survive_the_switch() {
        let mut app = app();
        app.router.push(View::Help);
        dispatch(&mut app, SliceAction::Switch(2));
        assert_eq!(app.view(), View::Overview, "the stack is reset, not pushed onto");
    }

    #[test]
    fn switching_to_the_current_slice_is_a_noop() {
        let mut app = app();
        assert!(dispatch(&mut app, SliceAction::Switch(1)).is_empty());
        assert_eq!(app.slice, Slice::Sessions);
    }

    #[test]
    fn a_number_past_the_tab_bar_does_nothing() {
        let mut app = app();
        assert!(dispatch(&mut app, SliceAction::Switch(99)).is_empty());
        assert!(dispatch(&mut app, SliceAction::Switch(0)).is_empty());
        assert_eq!(app.slice, Slice::Sessions);
    }

    #[test]
    fn the_summary_prefers_the_servers_counts_over_the_local_guess() {
        let mut app = app();
        app.sessions[1].attention = Some(Attention::NeedsInput);
        let local = summary(&app);
        assert!(local.pending, "no reply yet");
        assert_eq!(local.live, 2, "the local fallback is the active count");

        dispatch(&mut app, SliceAction::StatsLoaded(Box::new(stats())));
        let served = summary(&app);
        assert!(!served.pending);
        assert_eq!(served.live, 3);
        assert_eq!(served.needs_input, local.needs_input, "the count stays the client's own");
    }

    #[test]
    fn a_failed_stats_fetch_leaves_the_fallback_standing() {
        let mut app = app();
        dispatch(&mut app, SliceAction::StatsLoaded(Box::new(stats())));
        assert!(!summary(&app).pending);
        assert!(dispatch(&mut app, SliceAction::StatsFailed).is_empty());
        assert_eq!(summary(&app).live, 3, "the last good reply is kept");
    }

    #[test]
    fn needs_input_is_whatever_the_attention_module_counts() {
        let mut app = app();
        let delegated =
            |app: &App| i64::try_from(crate::app::attention::waiting_count(app)).expect("fits");
        assert_eq!(summary(&app).needs_input, delegated(&app));
        app.sessions[0].attention = Some(Attention::NeedsInput);
        app.sessions[1].attention = Some(Attention::NeedsInput);
        assert_eq!(summary(&app).needs_input, delegated(&app));
        assert!(summary(&app).needs_input > 0, "the fixture does have waiting sessions");
    }

    #[test]
    fn the_summary_carries_the_unread_total() {
        let mut app = app();
        assert_eq!(summary(&app).unread, 0);
        app.sessions[0].unread_count = 3;
        app.sessions[1].unread_count = 4;
        assert_eq!(summary(&app).unread, crate::app::unread::total(&app));
    }

    #[test]
    fn machines_counts_online_over_known() {
        let mut app = app();
        assert_eq!(machines(&app), (0, 0));
        app.machine_liveness.insert("m1".to_owned(), MachineLiveness::Online);
        app.machine_liveness.insert("m2".to_owned(), MachineLiveness::Stale);
        app.machine_liveness.insert("m3".to_owned(), MachineLiveness::Offline);
        assert_eq!(machines(&app), (1, 3));
    }

    #[test]
    fn todays_cost_takes_only_the_sessions_registered_since_midnight() {
        let mut app = app();
        let midnight = local_midnight_ms(CLOCK_MS).expect("a local midnight");
        app.sessions[0].registered_at = chrono::DateTime::from_timestamp_millis(midnight + 1_000);
        app.sessions[0].token_usage.cost_usd = 2.50;
        app.sessions[1].registered_at = chrono::DateTime::from_timestamp_millis(midnight - 1_000);
        app.sessions[1].token_usage.cost_usd = 9.00;
        assert!((today_cost_usd(&app) - 2.50).abs() < f64::EPSILON);
    }

    #[test]
    fn no_sessions_today_reads_as_zero_not_minus_zero() {
        let mut app = app();
        app.sessions.clear();
        assert_eq!(format!("{:.2}", today_cost_usd(&app)), "0.00");
    }

    #[test]
    fn a_session_with_no_registration_stamp_is_not_counted_as_todays() {
        let mut app = app();
        app.sessions[0].registered_at = None;
        app.sessions[0].token_usage.cost_usd = 4.00;
        app.sessions[1].registered_at = Some(ms_ago(0));
        app.sessions[1].token_usage.cost_usd = 1.00;
        assert!((today_cost_usd(&app) - 1.00).abs() < f64::EPSILON);
    }

    #[test]
    fn scrolling_the_overview_never_goes_above_the_top() {
        let mut app = app();
        dispatch(&mut app, SliceAction::OverviewScroll(-4));
        assert_eq!(app.overview_scroll, 0);
        dispatch(&mut app, SliceAction::OverviewScroll(3));
        assert_eq!(app.overview_scroll, 3);
    }

    #[test]
    fn a_zone_name_survives_the_localtime_symlink() {
        assert_eq!(
            zone_from_path("/usr/share/zoneinfo/Europe/Paris").as_deref(),
            Some("Europe/Paris")
        );
        assert_eq!(
            zone_from_path("/var/db/timezone/zoneinfo/Asia/Tokyo").as_deref(),
            Some("Asia/Tokyo")
        );
        assert_eq!(zone_from_path("/etc/localtime"), None);
        assert_eq!(zone_from_path("/usr/share/zoneinfo/"), None);
    }
}
