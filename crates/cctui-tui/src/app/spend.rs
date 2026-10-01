//! The spend slice: what the token windows hold, what the models cost, and the
//! dollars prompt-cache busts burned.
//!
//! Dollars per window are attributed by `cctui_clientcore::spend` from the
//! session list the TUI already holds — no endpoint reports spend per window —
//! so a figure here means "what the sessions registered in this window have
//! cost", the same basis as the Overview's cost tile.

use cctui_clientcore::spend::{
    self, CacheLossTotals, DailyPoint, LangfuseSpend, ModelSpendRow, SessionSpend, Windows,
};
use cctui_proto::api::cache_loss::DailyCacheLoss;
use cctui_proto::api::{TokenUsageWindows, UsageAnalytics, WindowTokenUsage};

use super::action::Effect;
use super::state::App;

/// Days of daily buckets the sparkline draws, and the range the model table
/// calls "30d".
pub const RANGE_DAYS: u32 = 30;
/// Days of cache-loss attribution, matching the webui card's own window.
pub const CACHE_LOSS_DAYS: u32 = 7;

#[derive(Debug, Default)]
pub struct Spend {
    pub windows: Option<TokenUsageWindows>,
    pub analytics: Option<UsageAnalytics>,
    pub cache_loss: Vec<DailyCacheLoss>,
    /// The Langfuse rollup of the session it was fetched for. One slot: only
    /// the open conversation needs a cost line.
    pub langfuse: Option<(String, LangfuseSpend)>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
}

impl Spend {
    /// The rollup to price the open conversation with, if it is the one that
    /// was fetched and it has traces.
    #[must_use]
    pub fn langfuse_for(&self, session_id: &str) -> Option<LangfuseSpend> {
        let (id, usage) = self.langfuse.as_ref()?;
        (id == session_id && spend::has_langfuse_cost(Some(*usage))).then_some(*usage)
    }
}

/// One row of the token table: a window's three token classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenRow {
    pub label: &'static str,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
}

impl TokenRow {
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input + self.output + self.cache_read
    }
}

const fn token_row(label: &'static str, w: &WindowTokenUsage) -> TokenRow {
    TokenRow { label, input: w.input, output: w.output, cache_read: w.cache_read }
}

/// The windows in the order the table lists them. Empty until the stats land,
/// so the view says "loading" rather than drawing four zeroes.
#[must_use]
pub fn token_rows(spend: &Spend) -> Vec<TokenRow> {
    let Some(w) = &spend.windows else { return Vec::new() };
    vec![
        token_row("1h", &w.hour),
        token_row("today", &w.today),
        token_row("24h", &w.day),
        token_row("7d", &w.week),
        token_row("30d", &w.month),
    ]
}

/// Dollars per model family per window, from the session list.
#[must_use]
pub fn model_rows(app: &App) -> Vec<ModelSpendRow> {
    let Some(windows) = windows_for(app.clock_ms) else { return Vec::new() };
    spend::model_spend(&session_spend(app), windows)
}

#[must_use]
pub fn totals(rows: &[ModelSpendRow]) -> ModelSpendRow {
    spend::spend_totals(rows)
}

/// Cost of the sessions registered since local midnight — what the Overview
/// tile shows.
#[must_use]
pub fn today_cost_usd(app: &App) -> f64 {
    windows_for(app.clock_ms).map_or(0.0, |w| spend::spend_since(&session_spend(app), w.today_ms))
}

fn session_spend(app: &App) -> Vec<SessionSpend> {
    app.sessions
        .iter()
        .map(|s| SessionSpend {
            model: s.model.clone(),
            registered_at_ms: s.registered_at.map(|at| at.timestamp_millis()),
            cost_usd: s.token_usage.cost_usd,
            tokens: s.token_usage.tokens_in + s.token_usage.tokens_out,
        })
        .collect()
}

/// Daily totals for the sparkline, zero-filled to `RANGE_DAYS`.
#[must_use]
pub fn daily_series(app: &App) -> Vec<u64> {
    let Some(analytics) = &app.spend.analytics else { return Vec::new() };
    if analytics.granularity != "day" {
        return Vec::new();
    }
    let points: Vec<DailyPoint> = analytics
        .buckets
        .iter()
        .filter_map(|b| {
            let at = chrono::DateTime::parse_from_rfc3339(&b.bucket).ok()?;
            Some(DailyPoint {
                day_ms: local_midnight_ms(at.timestamp_millis())?,
                tokens: b.input + b.output + b.cache_read,
            })
        })
        .collect();
    let Some(end) = local_midnight_ms(app.clock_ms) else { return Vec::new() };
    spend::fill_daily(&points, RANGE_DAYS as usize, end)
}

#[must_use]
pub fn cache_loss(app: &App) -> CacheLossTotals {
    let days: Vec<spend::DailyCacheLoss> = app
        .spend
        .cache_loss
        .iter()
        .map(|d| spend::DailyCacheLoss { usd: d.total, lost_tokens: d.lost_tokens, busts: d.busts })
        .collect();
    spend::cache_loss_totals(&days)
}

/// Local midnight, 7 days back and 30 days back, in unix ms.
fn windows_for(now_ms: i64) -> Option<Windows> {
    let today_ms = local_midnight_ms(now_ms)?;
    const DAY: i64 = 86_400_000;
    Some(Windows {
        today_ms,
        week_ms: today_ms - 6 * DAY,
        month_ms: today_ms - i64::from(RANGE_DAYS - 1) * DAY,
    })
}

/// Local midnight preceding `at_ms`, in unix ms.
#[must_use]
pub fn local_midnight_ms(at_ms: i64) -> Option<i64> {
    use chrono::TimeZone;
    let at = chrono::DateTime::from_timestamp_millis(at_ms)?.with_timezone(&chrono::Local);
    let date = at.date_naive().and_hms_opt(0, 0, 0)?;
    chrono::Local.from_local_datetime(&date).single().map(|dt| dt.timestamp_millis())
}

/// Minutes to subtract from UTC for local time, as the stats endpoints want it.
#[must_use]
pub fn tz_offset_minutes(now_ms: i64) -> i32 {
    use chrono::Offset;
    chrono::DateTime::from_timestamp_millis(now_ms)
        .map_or(0, |at| -(at.with_timezone(&chrono::Local).offset().fix().local_minus_utc() / 60))
}

pub enum SpendAction {
    /// `$`, or the tab.
    Open,
    Close,
    Refresh,
    Loaded(Box<SpendData>),
    Failed(String),
    /// The open conversation's Langfuse rollup, or its absence.
    Langfuse {
        session_id: String,
        usage: Option<LangfuseSpend>,
    },
}

/// One refresh's three replies, so a partial fetch cannot leave half a view.
#[derive(Debug)]
pub struct SpendData {
    pub windows: TokenUsageWindows,
    pub analytics: UsageAnalytics,
    pub cache_loss: Vec<DailyCacheLoss>,
}

pub fn reduce_spend(app: &mut App, action: SpendAction) -> Vec<Effect> {
    match action {
        SpendAction::Open => {
            let mut effects = super::slice::go_to(app, super::slice::Slice::Spend);
            if !app.spend.loaded {
                effects.extend(refresh(app));
            }
            effects
        }
        SpendAction::Close => super::slice::go_to(app, super::slice::Slice::Sessions),
        SpendAction::Refresh => refresh(app),
        SpendAction::Loaded(data) => {
            let SpendData { windows, analytics, cache_loss } = *data;
            app.spend.windows = Some(windows);
            app.spend.analytics = Some(analytics);
            app.spend.cache_loss = cache_loss;
            app.spend.loading = false;
            app.spend.loaded = true;
            app.spend.error = None;
            Vec::new()
        }
        SpendAction::Failed(message) => {
            app.spend.loading = false;
            app.spend.loaded = true;
            app.spend.error = Some(message);
            Vec::new()
        }
        SpendAction::Langfuse { session_id, usage } => {
            app.spend.langfuse = usage.map(|u| (session_id, u));
            Vec::new()
        }
    }
}

fn refresh(app: &mut App) -> Vec<Effect> {
    app.spend.loading = true;
    vec![Effect::FetchSpend { tz_offset: tz_offset_minutes(app.clock_ms) }]
}

/// Price the conversation being opened, if the deployment has a Langfuse sink.
/// A failure clears the line rather than keeping the previous session's figure.
#[must_use]
pub fn on_conversation_open(session_id: &str) -> Effect {
    Effect::FetchSessionLangfuse { session_id: session_id.to_owned() }
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::spend::LangfuseSpend;
    use cctui_proto::api::{TokenUsageWindows, UsageAnalytics, UsageBucket, WindowTokenUsage};

    use super::{SpendAction, SpendData, daily_series, token_rows, tz_offset_minutes};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    const fn window(input: u64, output: u64, cache_read: u64) -> WindowTokenUsage {
        WindowTokenUsage { input, output, cache_read }
    }

    fn data(buckets: Vec<UsageBucket>) -> Box<SpendData> {
        Box::new(SpendData {
            windows: TokenUsageWindows {
                hour: window(1, 2, 3),
                today: window(10, 20, 30),
                day: window(100, 200, 300),
                week: window(1000, 2000, 3000),
                month: window(10_000, 20_000, 30_000),
            },
            analytics: UsageAnalytics {
                granularity: "day".to_owned(),
                buckets,
                models: Vec::new(),
                heatmap: Vec::new(),
            },
            cache_loss: Vec::new(),
        })
    }

    fn bucket(day: &str, output: u64) -> UsageBucket {
        UsageBucket { bucket: day.to_owned(), input: 0, output, cache_read: 0, cache_creation: 0 }
    }

    #[test]
    fn a_fetch_fills_every_window_row() {
        let mut app = App::default();
        assert!(token_rows(&app.spend).is_empty());
        reduce(&mut app, Action::Spend(SpendAction::Loaded(data(Vec::new()))));
        let rows = token_rows(&app.spend);
        let labels: Vec<&str> = rows.iter().map(|r| r.label).collect();
        assert_eq!(labels, ["1h", "today", "24h", "7d", "30d"]);
        assert_eq!(rows[1].total(), 60);
        assert_eq!(rows[4].total(), 60_000);
        assert!(app.spend.loaded);
        assert!(!app.spend.loading);
    }

    #[test]
    fn a_failed_fetch_names_the_reason_and_stops_loading() {
        let mut app = App::default();
        reduce(&mut app, Action::Spend(SpendAction::Failed("nope".to_owned())));
        assert_eq!(app.spend.error.as_deref(), Some("nope"));
        assert!(!app.spend.loading);
        assert!(token_rows(&app.spend).is_empty());
    }

    #[test]
    fn the_daily_series_is_dense_and_ends_today() {
        let mut app = App::default();
        let today = chrono::Local::now();
        app.clock_ms = today.timestamp_millis();
        let day = |back: i64| (today - chrono::Duration::days(back)).to_rfc3339();
        reduce(
            &mut app,
            Action::Spend(SpendAction::Loaded(data(vec![bucket(&day(0), 5), bucket(&day(2), 7)]))),
        );
        let series = daily_series(&app);
        assert_eq!(series.len(), super::RANGE_DAYS as usize);
        assert_eq!(series[series.len() - 1], 5);
        assert_eq!(series[series.len() - 3], 7);
        assert_eq!(series[series.len() - 2], 0);
    }

    #[test]
    fn an_hourly_range_draws_no_daily_sparkline() {
        let mut app = App::default();
        let mut payload = data(Vec::new());
        payload.analytics.granularity = "hour".to_owned();
        reduce(&mut app, Action::Spend(SpendAction::Loaded(payload)));
        assert!(daily_series(&app).is_empty());
    }

    #[test]
    fn a_langfuse_rollup_prices_only_its_own_session_and_only_with_traces() {
        let mut app = App::default();
        let traced = LangfuseSpend { cost_usd: 0.5, trace_count: 3 };
        reduce(
            &mut app,
            Action::Spend(SpendAction::Langfuse {
                session_id: "a".to_owned(),
                usage: Some(traced),
            }),
        );
        assert_eq!(app.spend.langfuse_for("a"), Some(traced));
        assert_eq!(app.spend.langfuse_for("b"), None);

        reduce(
            &mut app,
            Action::Spend(SpendAction::Langfuse {
                session_id: "a".to_owned(),
                usage: Some(LangfuseSpend { cost_usd: 0.0, trace_count: 0 }),
            }),
        );
        assert_eq!(app.spend.langfuse_for("a"), None);

        reduce(
            &mut app,
            Action::Spend(SpendAction::Langfuse { session_id: "a".to_owned(), usage: None }),
        );
        assert_eq!(app.spend.langfuse_for("a"), None);
    }

    #[test]
    fn opening_fetches_once_then_reuses_what_arrived() {
        let mut app = App::default();
        let effects = reduce(&mut app, Action::Spend(SpendAction::Open));
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchSpend { .. })));
        reduce(&mut app, Action::Spend(SpendAction::Loaded(data(Vec::new()))));
        let effects = reduce(&mut app, Action::Spend(SpendAction::Open));
        assert!(!effects.iter().any(|e| matches!(e, Effect::FetchSpend { .. })));
    }

    #[test]
    fn the_tz_offset_is_the_minutes_to_subtract_from_utc() {
        use chrono::Offset;
        let now = chrono::Local::now();
        let want = -(now.offset().fix().local_minus_utc() / 60);
        assert_eq!(tz_offset_minutes(now.timestamp_millis()), want);
    }
}
