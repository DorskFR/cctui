//! The usage panel: each pool's aggregated windows and each credential's own,
//! with the reset countdown and burn rate computed from the client's clock.
//!
//! Both panes render the same [`WindowReadout`], so a percent, a dollar and an
//! unreported window read identically wherever they appear. The arithmetic and
//! the thresholds come from `cctui_clientcore::usage`, shared with the web UI's
//! cap bars.

use cctui_client::{AccountUsageEntry, PoolUsageView, UsageWindowView};
use cctui_clientcore::usage::{
    HeadroomTone, PaceState, bar_pct, headroom_tone, pace_state, reset_in_short, usd_readout,
};

use super::action::Effect;
use super::state::App;

/// How often the panel refetches while it is open.
pub const POLL_MS: i64 = 30_000;

/// Dollar windows report a spend, not a share of a quota.
fn is_usd(key: &str) -> bool {
    matches!(key, "session_usd" | "usd_5h" | "usd_7d")
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Pools,
    Accounts,
}

impl Pane {
    const fn other(self) -> Self {
        match self {
            Self::Pools => Self::Accounts,
            Self::Accounts => Self::Pools,
        }
    }
}

/// One window as the panel shows it, whatever it measures.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReadout {
    pub label: String,
    /// `None` when nothing was reported, which the view says rather than
    /// drawing an empty gauge.
    pub pct: Option<i64>,
    /// `$12.40 / $60.00` for a dollar window.
    pub usd: Option<String>,
    pub resets: Option<String>,
    pub tone: HeadroomTone,
    pub pace: Option<PaceState>,
    pub ratio: Option<f64>,
}

impl WindowReadout {
    /// `78%`, `$12.40 / $60.00`, or `not reported`.
    #[must_use]
    pub fn value_text(&self) -> String {
        if let Some(usd) = &self.usd {
            return usd.clone();
        }
        self.pct.map_or_else(|| "not reported".to_owned(), |pct| format!("{pct}%"))
    }

    /// `1.4x` when the window has a rate worth naming.
    #[must_use]
    pub fn burn_text(&self) -> Option<String> {
        let ratio = self.ratio?;
        self.pace.map(|_| format!("{ratio:.1}x"))
    }

    #[must_use]
    pub const fn burning(&self) -> bool {
        matches!(self.pace, Some(PaceState::Flame))
    }
}

/// A window of one credential.
fn account_readout(window: &UsageWindowView, now_ms: i64) -> WindowReadout {
    let usd = is_usd(&window.key).then(|| usd_readout(window.amount_usd, None)).flatten();
    let pct = if usd.is_some() { None } else { bar_pct(window.utilization) };
    let ratio = window.pace.as_ref().map(|p| p.ratio);
    WindowReadout {
        label: window.label.clone(),
        pct,
        usd,
        resets: reset_in_short(window.resets_at.map(|at| at.timestamp_millis()), now_ms),
        tone: headroom_tone(pct.map(|p| p as f64)),
        pace: pace_state(ratio),
        ratio,
    }
}

/// One line of the pools pane: a window of one family of one pool. `head` is
/// `Some` only on the family's first window, so the name is not repeated.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolLine {
    pub head: Option<String>,
    pub pool_id: uuid::Uuid,
    pub family: String,
    pub readout: WindowReadout,
    /// Members whose usage could not be read: the aggregate speaks for fewer
    /// accounts than the pool has, and saying so is the difference between
    /// "there is headroom" and "we cannot see".
    pub unknown_members: usize,
}

/// One line of the accounts pane: a credential and every window it reported.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountLine {
    pub account: uuid::Uuid,
    pub name: String,
    pub provider: String,
    pub readouts: Vec<WindowReadout>,
    /// The upstream incident reading, present only when the provider is
    /// degraded.
    pub status: Option<String>,
}

impl AccountLine {
    /// `alice (anthropic)`, the identity the rows are sorted and matched by.
    #[must_use]
    pub fn title(&self) -> String {
        format!("{} ({})", self.name, self.provider)
    }

    /// The fullest window this credential reported.
    #[must_use]
    pub fn worst(&self) -> Option<&WindowReadout> {
        self.readouts.iter().filter(|r| r.pct.is_some()).max_by_key(|r| r.pct.unwrap_or(0))
    }
}

#[derive(Debug, Default)]
pub struct Usage {
    pub pools: Vec<PoolUsageView>,
    pub accounts: Vec<AccountUsageEntry>,
    pub pane: Pane,
    pub pool_selected: usize,
    pub account_selected: usize,
    pub open: bool,
    pub loading: bool,
    /// Whether both halves have ever answered, so an empty panel can be told
    /// apart from an unasked one.
    pub loaded: bool,
    pub error: Option<String>,
    /// Clock reading of the last fetch, the base of the 30 s poll.
    pub polled_ms: i64,
    /// The credential `Enter` picked, for the accounts slice to consume.
    pub open_account: Option<uuid::Uuid>,
}

impl Usage {
    #[must_use]
    pub fn pool_lines(&self, now_ms: i64) -> Vec<PoolLine> {
        let mut out = Vec::new();
        for pool in &self.pools {
            for family in &pool.families {
                let unknown = family.members.iter().filter(|m| !m.usage_known).count();
                for (i, window) in family.windows.iter().enumerate() {
                    let ratio = window.ratio;
                    let pct = bar_pct(window.level_pct);
                    out.push(PoolLine {
                        head: (i == 0).then(|| format!("{}/{}", family.family, pool.name)),
                        pool_id: pool.pool_id,
                        family: family.family.clone(),
                        readout: WindowReadout {
                            label: window.label.clone(),
                            pct,
                            usd: None,
                            resets: reset_in_short(
                                window.next_reset_at.map(|at| at.timestamp_millis()),
                                now_ms,
                            ),
                            tone: headroom_tone(pct.map(|p| p as f64)),
                            pace: pace_state(ratio),
                            ratio,
                        },
                        unknown_members: unknown,
                    });
                }
            }
        }
        out
    }

    #[must_use]
    pub fn account_lines(&self, now_ms: i64) -> Vec<AccountLine> {
        self.accounts
            .iter()
            .map(|entry| AccountLine {
                account: entry.account,
                name: entry.account_name.clone(),
                provider: entry.provider.clone(),
                readouts: entry
                    .windows
                    .iter()
                    .map(|window| account_readout(window, now_ms))
                    .collect(),
                status: entry
                    .provider_status
                    .as_ref()
                    .map(|s| s.description.clone().unwrap_or_else(|| s.indicator.clone())),
            })
            .collect()
    }

    fn len(&self, pane: Pane, now_ms: i64) -> usize {
        match pane {
            Pane::Pools => self.pool_lines(now_ms).len(),
            Pane::Accounts => self.accounts.len(),
        }
    }

    const fn cursor(&mut self, pane: Pane) -> &mut usize {
        match pane {
            Pane::Pools => &mut self.pool_selected,
            Pane::Accounts => &mut self.account_selected,
        }
    }
}

/// Worst window of each provider family, for the status line: `anthropic 91%`,
/// families in order, worst first. Empty while nothing reports a percentage.
#[must_use]
pub fn family_summary(app: &App) -> Vec<(String, i64)> {
    let mut worst: Vec<(String, i64)> = Vec::new();
    for line in app.usage.account_lines(app.clock_ms) {
        let Some(pct) = line.worst().and_then(|r| r.pct) else { continue };
        match worst.iter_mut().find(|(family, _)| *family == line.provider) {
            Some((_, best)) => *best = (*best).max(pct),
            None => worst.push((line.provider.clone(), pct)),
        }
    }
    worst.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    worst
}

/// The status-line chip: the worst family, and whether anything in it is
/// burning. The caller colours it rather than prefixing a glyph — the line
/// already carries the live count, the machines, the day's cost and the
/// attention chip, and two more cells is what makes it wrap. `family_summary`
/// is there for a surface with room for every family.
#[must_use]
pub fn summary_chip(app: &App) -> Option<(String, bool)> {
    let (family, pct) = family_summary(app).into_iter().next()?;
    let burning =
        app.usage.account_lines(app.clock_ms).iter().any(|line| {
            line.provider == family && line.readouts.iter().any(WindowReadout::burning)
        });
    Some((format!("{family} {pct}%"), burning))
}

/// Short reading for one account identity, for a pane that shows a credential
/// without drawing its gauges. Consumed by the accounts slice, which owns no
/// usage state of its own.
#[must_use]
pub fn account_summary(app: &App, account: uuid::Uuid) -> Option<String> {
    let lines = app.usage.account_lines(app.clock_ms);
    let line = lines.iter().find(|line| line.account == account)?;
    let worst = line.worst()?;
    let value = worst.value_text();
    Some(worst.resets.as_ref().map_or_else(
        || format!("{} {value}", worst.label),
        |resets| format!("{} {value} · resets {resets}", worst.label),
    ))
}

pub enum UsageAction {
    Open,
    Close,
    Refresh,
    PoolsLoaded(Vec<PoolUsageView>),
    AccountsLoaded(Vec<AccountUsageEntry>),
    Failed(String),
    SelectNext,
    SelectPrev,
    SwitchPane,
    /// `Enter`: hand the credential to the accounts slice.
    OpenAccount,
}

pub fn reduce_usage(app: &mut App, action: UsageAction) -> Vec<Effect> {
    match action {
        UsageAction::Open => {
            if !app.usage.open {
                app.usage.open = true;
                app.router.push(super::state::View::Usage);
            }
            refresh(app)
        }
        UsageAction::Close => {
            if std::mem::take(&mut app.usage.open) {
                app.router.pop();
            }
            Vec::new()
        }
        UsageAction::Refresh => refresh(app),
        UsageAction::PoolsLoaded(pools) => {
            app.usage.pools = pools;
            settle(app);
            Vec::new()
        }
        UsageAction::AccountsLoaded(accounts) => {
            app.usage.accounts = accounts;
            settle(app);
            Vec::new()
        }
        UsageAction::Failed(message) => {
            app.usage.loading = false;
            app.usage.loaded = true;
            app.usage.error = Some(message);
            Vec::new()
        }
        UsageAction::SelectNext => {
            let (pane, now) = (app.usage.pane, app.clock_ms);
            let len = app.usage.len(pane, now);
            if len > 0 {
                let cursor = app.usage.cursor(pane);
                *cursor = (*cursor + 1).min(len - 1);
            }
            Vec::new()
        }
        UsageAction::SelectPrev => {
            let pane = app.usage.pane;
            let cursor = app.usage.cursor(pane);
            *cursor = cursor.saturating_sub(1);
            Vec::new()
        }
        UsageAction::SwitchPane => {
            app.usage.pane = app.usage.pane.other();
            Vec::new()
        }
        UsageAction::OpenAccount => open_account(app),
    }
}

/// A poll while the panel is open; the reducer owns the period so a tick that
/// arrives with the panel closed costs nothing.
pub fn on_tick(app: &mut App) -> Vec<Effect> {
    if !app.usage.open || app.usage.loading {
        return Vec::new();
    }
    if app.clock_ms - app.usage.polled_ms < POLL_MS {
        return Vec::new();
    }
    refresh(app)
}

fn refresh(app: &mut App) -> Vec<Effect> {
    app.usage.loading = true;
    app.usage.polled_ms = app.clock_ms;
    vec![Effect::FetchUsage]
}

/// Both halves arrive as separate actions; the panel stops loading on the first
/// and keeps whatever the other last said, so one empty pane never blanks the
/// other.
fn settle(app: &mut App) {
    app.usage.loading = false;
    app.usage.loaded = true;
    app.usage.error = None;
    clamp(app);
}

fn clamp(app: &mut App) {
    let now = app.clock_ms;
    for pane in [Pane::Pools, Pane::Accounts] {
        let len = app.usage.len(pane, now);
        let cursor = app.usage.cursor(pane);
        *cursor = if len == 0 { 0 } else { (*cursor).min(len - 1) };
    }
}

fn open_account(app: &mut App) -> Vec<Effect> {
    if app.usage.pane != Pane::Accounts {
        return Vec::new();
    }
    let lines = app.usage.account_lines(app.clock_ms);
    let Some(line) = lines.get(app.usage.account_selected) else { return Vec::new() };
    app.usage.open_account = Some(line.account);
    let title = line.title();
    app.toast(super::toast::Level::Info, format!("{title} — open it from the Accounts tab"));
    Vec::new()
}

#[cfg(test)]
mod tests {
    use cctui_client::{
        AccountUsageEntry, PoolFamilyUsage, PoolUsageMember, PoolUsageView, PoolUsageWindow,
        UsagePace, UsageWindowView,
    };
    use cctui_clientcore::usage::{HeadroomTone, PaceState};
    use uuid::Uuid;

    use super::{POLL_MS, Pane, UsageAction, account_summary, family_summary, summary_chip};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    const NOW: i64 = 1_700_000_000_000;
    const ALICE: &str = "11111111-1111-4111-8111-111111111111";
    const BOB: &str = "22222222-2222-4222-8222-222222222222";
    const POOL: &str = "33333333-3333-4333-8333-333333333333";

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).expect("a uuid")
    }

    fn at(ms: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(ms).expect("a stamp")
    }

    fn pct_window(key: &str, label: &str, util: f64, resets_in_ms: i64) -> UsageWindowView {
        UsageWindowView {
            key: key.to_owned(),
            kind: key.to_owned(),
            label: label.to_owned(),
            utilization: Some(util),
            amount_usd: None,
            resets_at: Some(at(NOW + resets_in_ms)),
            model_id: None,
            model_display_name: None,
            pace: None,
        }
    }

    fn usd_window(amount: f64) -> UsageWindowView {
        UsageWindowView {
            key: "usd_5h".to_owned(),
            kind: "usd".to_owned(),
            label: "$".to_owned(),
            utilization: None,
            amount_usd: Some(amount),
            resets_at: None,
            model_id: None,
            model_display_name: None,
            pace: None,
        }
    }

    fn null_window() -> UsageWindowView {
        UsageWindowView {
            key: "weekly_all".to_owned(),
            kind: "weekly_all".to_owned(),
            label: "7d".to_owned(),
            utilization: None,
            amount_usd: None,
            resets_at: None,
            model_id: None,
            model_display_name: None,
            pace: None,
        }
    }

    fn entry(
        account: &str,
        name: &str,
        provider: &str,
        windows: Vec<UsageWindowView>,
    ) -> AccountUsageEntry {
        AccountUsageEntry {
            account_id: id(account),
            provider: provider.to_owned(),
            windows,
            age_secs: 0,
            account: id(account),
            account_name: name.to_owned(),
            account_emoji: None,
            header_pin: false,
            ..AccountUsageEntry::default()
        }
    }

    fn pool_window(label: &str, level: Option<f64>, ratio: Option<f64>) -> PoolUsageWindow {
        PoolUsageWindow {
            key: "session".to_owned(),
            kind: "session".to_owned(),
            label: label.to_owned(),
            model_display_name: None,
            level_pct: level,
            expected_pct: 50.0,
            ratio,
            next_reset_at: Some(at(NOW + 72 * 60_000)),
            projection: None,
            projection_unavailable: None,
        }
    }

    fn pool(windows: Vec<PoolUsageWindow>, unknown: bool) -> PoolUsageView {
        PoolUsageView {
            pool_id: id(POOL),
            name: "default".to_owned(),
            strategy: "headroom".to_owned(),
            failover: true,
            families: vec![PoolFamilyUsage {
                family: "anthropic".to_owned(),
                members: vec![PoolUsageMember {
                    account_id: id(ALICE),
                    name: "alice".to_owned(),
                    emoji: None,
                    weight: 1.0,
                    usage_known: !unknown,
                }],
                windows,
            }],
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.clock_ms = NOW;
        app
    }

    fn act(app: &mut App, action: UsageAction) -> Vec<Effect> {
        reduce(app, Action::Usage(action))
    }

    fn loaded(app: &mut App) {
        act(
            app,
            UsageAction::PoolsLoaded(vec![pool(
                vec![pool_window("5h", Some(78.0), Some(1.4)), pool_window("7d", Some(31.0), None)],
                false,
            )]),
        );
        act(
            app,
            UsageAction::AccountsLoaded(vec![
                entry(
                    ALICE,
                    "alice",
                    "anthropic",
                    vec![pct_window("session", "5h", 91.0, 22 * 60_000), null_window()],
                ),
                entry(BOB, "bob", "openai", vec![usd_window(12.4)]),
            ]),
        );
    }

    #[test]
    fn opening_pushes_the_view_and_fetches_both_halves() {
        let mut app = app();
        let effects = act(&mut app, UsageAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchUsage)));
        assert_eq!(app.view(), crate::app::View::Usage);
        assert!(app.usage.loading);

        loaded(&mut app);
        assert!(!app.usage.loading);
        assert!(app.usage.loaded);

        act(&mut app, UsageAction::Close);
        assert_ne!(app.view(), crate::app::View::Usage);
        assert!(!app.usage.open);
    }

    #[test]
    fn the_three_window_types_each_read_as_themselves() {
        let mut app = app();
        loaded(&mut app);
        let lines = app.usage.account_lines(NOW);

        let five_hour = &lines[0].readouts[0];
        assert_eq!(five_hour.value_text(), "91%");
        assert_eq!(five_hour.resets.as_deref(), Some("22m"));
        assert_eq!(five_hour.tone, HeadroomTone::Danger);

        let weekly = &lines[0].readouts[1];
        assert_eq!(weekly.value_text(), "not reported");
        assert_eq!(weekly.tone, HeadroomTone::Unknown);
        assert_eq!(weekly.resets, None);

        let dollars = &lines[1].readouts[0];
        assert_eq!(dollars.value_text(), "$12.40");
        assert_eq!(dollars.pct, None, "a spend is not a share of a quota");
    }

    #[test]
    fn a_pool_window_carries_its_burn_and_names_its_family_once() {
        let mut app = app();
        loaded(&mut app);
        let lines = app.usage.pool_lines(NOW);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].head.as_deref(), Some("anthropic/default"));
        assert_eq!(lines[1].head, None, "the family is named on its first window only");
        assert_eq!(lines[0].readout.value_text(), "78%");
        assert_eq!(lines[0].readout.resets.as_deref(), Some("1h"));
        assert_eq!(lines[0].readout.pace, Some(PaceState::Flame));
        assert_eq!(lines[0].readout.burn_text().as_deref(), Some("1.4x"));
        assert!(lines[0].readout.burning());
        assert_eq!(lines[1].readout.burn_text(), None, "a rateless window claims no burn");
    }

    #[test]
    fn a_member_whose_usage_is_unreadable_is_counted_not_hidden() {
        let mut app = app();
        act(
            &mut app,
            UsageAction::PoolsLoaded(vec![pool(vec![pool_window("5h", None, None)], true)]),
        );
        let lines = app.usage.pool_lines(NOW);
        assert_eq!(lines[0].unknown_members, 1);
        assert_eq!(lines[0].readout.value_text(), "not reported");
    }

    #[test]
    fn each_pane_keeps_its_own_cursor_inside_its_own_list() {
        let mut app = app();
        loaded(&mut app);
        for _ in 0..5 {
            act(&mut app, UsageAction::SelectNext);
        }
        assert_eq!(app.usage.pane, Pane::Pools);
        assert_eq!(app.usage.pool_selected, 1);

        act(&mut app, UsageAction::SwitchPane);
        assert_eq!(app.usage.pane, Pane::Accounts);
        assert_eq!(app.usage.account_selected, 0, "the other pane's cursor is its own");
        for _ in 0..5 {
            act(&mut app, UsageAction::SelectNext);
        }
        assert_eq!(app.usage.account_selected, 1);

        act(&mut app, UsageAction::SwitchPane);
        assert_eq!(app.usage.pool_selected, 1, "switching back finds it where it was");
        act(&mut app, UsageAction::SelectPrev);
        act(&mut app, UsageAction::SelectPrev);
        assert_eq!(app.usage.pool_selected, 0);
    }

    #[test]
    fn a_shorter_list_brings_the_cursor_back() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, UsageAction::SwitchPane);
        act(&mut app, UsageAction::SelectNext);
        assert_eq!(app.usage.account_selected, 1);
        act(
            &mut app,
            UsageAction::AccountsLoaded(vec![entry(ALICE, "alice", "anthropic", vec![])]),
        );
        assert_eq!(app.usage.account_selected, 0);
    }

    #[test]
    fn the_panel_polls_on_its_own_period_and_only_while_open() {
        let mut app = app();
        act(&mut app, UsageAction::Open);
        loaded(&mut app);
        assert!(super::on_tick(&mut app).is_empty(), "nothing is due yet");

        app.clock_ms += POLL_MS;
        assert!(super::on_tick(&mut app).iter().any(|e| matches!(e, Effect::FetchUsage)));
        loaded(&mut app);

        act(&mut app, UsageAction::Close);
        app.clock_ms += 10 * POLL_MS;
        assert!(super::on_tick(&mut app).is_empty(), "a closed panel costs no requests");
    }

    #[test]
    fn a_fetch_in_flight_is_not_polled_over() {
        let mut app = app();
        act(&mut app, UsageAction::Open);
        app.clock_ms += 10 * POLL_MS;
        assert!(app.usage.loading);
        assert!(super::on_tick(&mut app).is_empty());
    }

    #[test]
    fn a_failed_fetch_is_reported_rather_than_looking_quota_less() {
        let mut app = app();
        act(&mut app, UsageAction::Open);
        act(&mut app, UsageAction::Failed("this key may not read usage".to_owned()));
        assert!(!app.usage.loading);
        assert!(app.usage.loaded);
        assert_eq!(app.usage.error.as_deref(), Some("this key may not read usage"));
        assert!(app.usage.accounts.is_empty());
    }

    #[test]
    fn one_half_arriving_does_not_blank_the_other() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, UsageAction::PoolsLoaded(Vec::new()));
        assert!(app.usage.pools.is_empty());
        assert_eq!(app.usage.accounts.len(), 2, "the accounts half stands");
    }

    #[test]
    fn the_status_summary_is_the_worst_window_of_each_family() {
        let mut app = app();
        assert_eq!(summary_chip(&app), None, "nothing reported, nothing to say");

        loaded(&mut app);
        assert_eq!(
            family_summary(&app),
            vec![("anthropic".to_owned(), 91)],
            "a dollar-only family reports no share"
        );
        assert_eq!(summary_chip(&app), Some(("anthropic 91%".to_owned(), false)));

        let mut burning = pct_window("session", "5h", 91.0, 22 * 60_000);
        burning.pace = Some(UsagePace {
            elapsed_fraction: 0.5,
            expected_pct: 50.0,
            ratio: 1.8,
            projected_wall_at: None,
            slope_hours: None,
        });
        act(
            &mut app,
            UsageAction::AccountsLoaded(vec![entry(ALICE, "alice", "anthropic", vec![burning])]),
        );
        assert_eq!(
            summary_chip(&app),
            Some(("anthropic 91%".to_owned(), true)),
            "the burn is a flag, not extra text"
        );

        act(
            &mut app,
            UsageAction::AccountsLoaded(vec![
                entry(ALICE, "alice", "anthropic", vec![pct_window("session", "5h", 40.0, 60_000)]),
                entry(BOB, "bob", "openai", vec![pct_window("session", "5h", 80.0, 60_000)]),
            ]),
        );
        assert_eq!(
            summary_chip(&app),
            Some(("openai 80%".to_owned(), false)),
            "worst family leads"
        );
    }

    #[test]
    fn two_credentials_of_one_family_report_the_fuller_one() {
        let mut app = app();
        act(
            &mut app,
            UsageAction::AccountsLoaded(vec![
                entry(ALICE, "alice", "anthropic", vec![pct_window("session", "5h", 40.0, 60_000)]),
                entry(BOB, "bob", "anthropic", vec![pct_window("session", "5h", 77.0, 60_000)]),
            ]),
        );
        assert_eq!(family_summary(&app), vec![("anthropic".to_owned(), 77)]);
    }

    #[test]
    fn the_summary_follows_a_poll_rather_than_the_first_reply() {
        let mut app = app();
        act(&mut app, UsageAction::Open);
        loaded(&mut app);
        assert_eq!(summary_chip(&app), Some(("anthropic 91%".to_owned(), false)));

        app.clock_ms += POLL_MS;
        assert!(!super::on_tick(&mut app).is_empty());
        act(
            &mut app,
            UsageAction::AccountsLoaded(vec![entry(
                ALICE,
                "alice",
                "anthropic",
                vec![pct_window("session", "5h", 100.0, 60_000)],
            )]),
        );
        assert_eq!(summary_chip(&app), Some(("anthropic 100%".to_owned(), false)));
    }

    #[test]
    fn a_credentials_reading_is_available_to_a_pane_that_draws_no_gauges() {
        let mut app = app();
        loaded(&mut app);
        assert_eq!(account_summary(&app, id(ALICE)).as_deref(), Some("5h 91% · resets 22m"));
        assert_eq!(account_summary(&app, id(BOB)), None, "a spend is not a window reading");
        assert_eq!(account_summary(&app, id(POOL)), None);
    }

    #[test]
    fn enter_hands_the_selected_credential_to_the_accounts_slice() {
        let mut app = app();
        loaded(&mut app);
        act(&mut app, UsageAction::OpenAccount);
        assert_eq!(app.usage.open_account, None, "the pools pane has no credential to open");

        act(&mut app, UsageAction::SwitchPane);
        act(&mut app, UsageAction::OpenAccount);
        assert_eq!(app.usage.open_account, Some(id(ALICE)));
        assert!(app.toasts.latest().expect("a toast").text.contains("alice"));
    }
}
