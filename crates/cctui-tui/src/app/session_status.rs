//! Derived row status signals, mirroring the webui `sessions.logic.ts`.
//!
//! `now_ms` is a parameter everywhere: these re-evaluate on a clock tick, with
//! no refetch, so nothing here may read the clock itself.

use cctui_proto::api::SessionListItem;
use cctui_proto::classifier::Bucket;
use cctui_proto::models::{Liveness, SessionEndReason, SessionStatus};

use super::session_list::{Group, group_of};

/// A Working session whose newest activity is older than this is not working.
pub const STALE_WORKING_AFTER_MS: i64 = 30 * 60 * 1000;

/// A Working session with no tool call for longer than this reads as wedged.
pub const TOOL_ASLEEP_AFTER_MS: i64 = 2 * 60 * 1000;

fn in_working_group(s: &SessionListItem) -> bool {
    group_of(s) == Group::Bucket(Bucket::Working)
}

fn age_ms(at: chrono::DateTime<chrono::Utc>, now_ms: i64) -> i64 {
    (now_ms - at.timestamp_millis()).max(0)
}

/// A Working-bucket session whose last heartbeat aged past the threshold.
#[must_use]
pub fn is_stale_working(s: &SessionListItem, now_ms: i64) -> bool {
    in_working_group(s)
        && s.last_heartbeat.is_some_and(|h| age_ms(h, now_ms) > STALE_WORKING_AFTER_MS)
}

/// What the leading dot says, in the webui's precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowLiveness {
    Hibernated,
    Stale,
    Active,
    Dead,
}

impl RowLiveness {
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Hibernated => "☾",
            Self::Stale => "◐",
            Self::Active => "●",
            Self::Dead => "○",
        }
    }
}

/// Hibernation wins, then staleness, then the server's own tier.
#[must_use]
pub fn row_liveness(s: &SessionListItem, stale: bool) -> RowLiveness {
    if s.hibernated {
        return RowLiveness::Hibernated;
    }
    if stale || s.liveness == Liveness::Stale {
        return RowLiveness::Stale;
    }
    if s.liveness == Liveness::Active { RowLiveness::Active } else { RowLiveness::Dead }
}

/// Live tool cadence and task-list progress for one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivity {
    /// Whether there is anything to show at all.
    pub show: bool,
    pub count: u32,
    /// Since the newest tool call, own or rolled up from a subagent.
    pub age_ms: Option<i64>,
    pub detail: Option<String>,
    /// Working, but no tool call for longer than [`TOOL_ASLEEP_AFTER_MS`].
    pub asleep: bool,
    pub todo_done: usize,
    pub todo_total: usize,
    /// The one `in_progress` entry's active form, on Working rows only.
    pub todo_active: Option<String>,
}

#[must_use]
pub fn tool_activity(s: &SessionListItem, now_ms: i64) -> ToolActivity {
    let working = in_working_group(s) && s.status != SessionStatus::Archived;
    let age_ms = s.last_tool_at.map(|t| age_ms(t, now_ms));
    let detail = s.activity_detail.clone().filter(|d| !d.trim().is_empty());
    let asleep = working && age_ms.is_some_and(|a| a > TOOL_ASLEEP_AFTER_MS);
    let todo_total = s.todos.len();
    let todo_done = s.todos.iter().filter(|t| t.status == "completed").count();
    let todo_active = if working {
        s.todos
            .iter()
            .find(|t| t.status == "in_progress")
            .and_then(|t| nonempty(t.active_form.as_deref()).or_else(|| nonempty(Some(&t.content))))
    } else {
        None
    };
    let show = (working && (age_ms.is_some() || detail.is_some())) || todo_total > 0;
    ToolActivity {
        show,
        count: s.tool_use_count,
        age_ms,
        detail,
        asleep,
        todo_done,
        todo_total,
        todo_active,
    }
}

fn nonempty(text: Option<&str>) -> Option<String> {
    text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

/// Compact `12s` / `3m` / `1h` age label.
#[must_use]
pub fn format_ago(ms: i64) -> String {
    let secs = (ms.max(0) + 500) / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 { format!("{mins}m") } else { format!("{}h", mins / 60) }
}

/// `✕<label>` for an ended session, or `None`: a plain completion is the row's
/// resting state, not a badge. The wording comes from the shared end-reason
/// table, so the row, the banner and the toast all say the same thing.
#[must_use]
pub fn end_badge(s: &SessionListItem) -> Option<String> {
    let reason = s.end_reason?;
    if reason == SessionEndReason::Completed {
        return None;
    }
    Some(format!("✕{}", crate::app::attention::end_reason_label(reason)))
}

/// Everything the right-hand side of a row can say, in triage order. The one
/// place a row-level indicator goes: a new signal is a field here, never a span
/// appended after the width budget is settled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RowBadges {
    /// Label chips, cut to [`MAX_LABEL_CHIPS`] with a `+N` tail. Each carries its
    /// own hue, so the row renders one segment per chip rather than one string.
    pub labels: Vec<LabelChip>,
    /// `?` an unanswered question, `P` a plan waiting for approval.
    pub prompt: Option<&'static str>,
    /// A permission card is up and nobody has answered it.
    pub pending: bool,
    pub unread: u32,
    pub auto_approve: bool,
    /// The next turn pays to re-read the context the harness dropped.
    pub cache_cold: bool,
    /// The account is parked behind a provider soft limit.
    pub soft_limited: bool,
    /// A named account whose traffic the gateway has never seen: the session
    /// is probably billing somewhere else.
    pub account_traffic: bool,
    /// A job cctui did not start, so removing it ends someone else's work.
    pub foreign: bool,
    pub end: Option<String>,
}

impl RowBadges {
    #[must_use]
    pub fn of(
        s: &SessionListItem,
        pending: bool,
        prompt: Option<&'static str>,
        soft_limited: bool,
    ) -> Self {
        Self {
            labels: label_chips(&s.labels),
            prompt,
            pending,
            unread: s.unread_count,
            auto_approve: s.auto_approve,
            cache_cold: s.cache_cold,
            soft_limited,
            account_traffic: s.account_name.is_some() && !s.account_traffic_observed,
            foreign: s.origin.is_foreign(),
            end: end_badge(s),
        }
    }

    #[must_use]
    #[cfg(test)]
    pub const fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.glyphs_empty()
    }

    /// Whether the glyph cluster — everything but the label chips — says nothing.
    #[must_use]
    pub const fn glyphs_empty(&self) -> bool {
        self.prompt.is_none()
            && !self.pending
            && self.unread == 0
            && !self.auto_approve
            && !self.cache_cold
            && !self.soft_limited
            && !self.account_traffic
            && !self.foreign
            && self.end.is_none()
    }

    /// Whether the cluster is asking for the user, as opposed to just reporting.
    #[must_use]
    pub const fn wants_you(&self) -> bool {
        self.pending || self.prompt.is_some()
    }

    /// One space-joined string; the row renders it as a single tail.
    #[must_use]
    pub fn text(&self) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(6);
        if self.pending {
            parts.push("!".to_owned());
        }
        if let Some(prompt) = self.prompt {
            parts.push(prompt.to_owned());
        }
        if self.unread > 0 {
            parts.push(format!("●{}", self.unread));
        }
        if self.auto_approve {
            parts.push("⚡".to_owned());
        }
        if self.soft_limited {
            parts.push("⏸".to_owned());
        }
        if self.account_traffic {
            parts.push("⚠".to_owned());
        }
        if self.cache_cold {
            parts.push("❄".to_owned());
        }
        if self.foreign {
            parts.push("⌂".to_owned());
        }
        if let Some(end) = &self.end {
            parts.push(end.clone());
        }
        parts.join(" ")
    }
}

/// At most this many label chips before the rest collapse into `+N`: a row with
/// eight labels would otherwise have no room left for what it is doing.
pub const MAX_LABEL_CHIPS: usize = 2;

/// One label as the row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelChip {
    pub text: String,
    /// `None` for the `+N` overflow chip, which belongs to no single label.
    pub hue: Option<u32>,
}

/// `[wave-5] [infra] +2` — the first [`MAX_LABEL_CHIPS`] by name, then a count.
#[must_use]
pub fn label_chips(labels: &[cctui_proto::api::Label]) -> Vec<LabelChip> {
    let mut shown: Vec<&cctui_proto::api::Label> = labels.iter().collect();
    shown.sort_by(|a, b| a.name.cmp(&b.name));
    let overflow = shown.len().saturating_sub(MAX_LABEL_CHIPS);
    let mut out: Vec<LabelChip> = shown
        .into_iter()
        .take(MAX_LABEL_CHIPS)
        .map(|l| LabelChip {
            text: format!("[{}]", l.name),
            hue: Some(cctui_clientcore::labels::label_hue(&l.name, &l.color)),
        })
        .collect();
    if overflow > 0 {
        out.push(LabelChip { text: format!("+{overflow}"), hue: None });
    }
    out
}

/// The dim trailing column: why this row looks the way it does, in one phrase.
#[must_use]
pub fn activity_text(
    s: &SessionListItem,
    act: &ToolActivity,
    stale: bool,
    now_ms: i64,
) -> Option<String> {
    if s.hibernated {
        return Some("hibernated".to_owned());
    }
    if stale {
        let age =
            s.last_heartbeat.map_or_else(|| "?".to_owned(), |h| format_ago(age_ms(h, now_ms)));
        return Some(format!("stale {age}"));
    }
    if act.asleep {
        return Some("asleep".to_owned());
    }
    if act.todo_total > 0 {
        let head = format!("{}/{}", act.todo_done, act.todo_total);
        return Some(
            act.todo_active.as_ref().map_or_else(|| head.clone(), |t| format!("{head} {t}")),
        );
    }
    act.detail.clone()
}

/// `⚙14 8s`: this turn's tool count and the age of the newest call.
#[must_use]
pub fn cadence_text(act: &ToolActivity) -> Option<String> {
    act.age_ms.map(|ms| format!("⚙{} {}", act.count, format_ago(ms)))
}

/// Cut `text` to `max` display columns, marking the cut with `…`.
#[must_use]
pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// The glyph legend the cheat sheet prints, so `?` explains every row marker.
pub const GLYPH_LEGEND: &[(&str, &str)] = &[
    ("●", "working"),
    ("◐", "stale or asleep"),
    ("○", "idle"),
    ("☾", "hibernated"),
    ("↳", "subagent"),
    ("▸ / ▾", "folded / open group"),
    ("[name]", "a label, tinted by its hue"),
    ("+N", "more labels than fit"),
    ("!", "permission waiting on you"),
    ("?", "question waiting on you"),
    ("P", "plan waiting for approval"),
    ("●N", "unread messages"),
    ("⚡", "auto-approve on"),
    ("⏸", "account soft-limited"),
    ("⚠", "account traffic never seen"),
    ("❄", "cache cold, next turn re-reads"),
    ("⌂", "a job cctui did not start"),
    ("✕reason", "how it ended"),
    ("⚙N age", "tool calls, age of the last"),
];

#[cfg(test)]
mod tests {
    use super::{
        RowBadges, RowLiveness, STALE_WORKING_AFTER_MS, TOOL_ASLEEP_AFTER_MS, activity_text,
        cadence_text, end_badge, format_ago, is_stale_working, row_liveness, tool_activity,
        truncate,
    };
    use crate::testsupport::{pinned_session, session, todo};

    const MIN: i64 = 60 * 1000;

    fn at(ms: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(ms).expect("a valid stamp")
    }

    #[test]
    fn a_working_session_goes_stale_only_past_the_threshold() {
        let now = 10 * STALE_WORKING_AFTER_MS;
        let mut s = session("s", "p", "active", "working");
        s.last_heartbeat = Some(at(now - STALE_WORKING_AFTER_MS));
        assert!(!is_stale_working(&s, now), "exactly at the threshold is not yet stale");
        s.last_heartbeat = Some(at(now - STALE_WORKING_AFTER_MS - 1));
        assert!(is_stale_working(&s, now));
    }

    #[test]
    fn only_the_working_group_can_be_stale() {
        let now = 10 * STALE_WORKING_AFTER_MS;
        let old = Some(at(now - 10 * STALE_WORKING_AFTER_MS + 1));
        for bucket in ["blocked", "review", "done"] {
            let mut s = session("s", "p", "active", bucket);
            s.last_heartbeat = old;
            assert!(!is_stale_working(&s, now), "{bucket} carries its own signal");
        }
        let mut pinned = pinned_session("s", "p");
        pinned.last_heartbeat = old;
        assert!(!is_stale_working(&pinned, now), "a pinned row is in the Pinned group");
    }

    #[test]
    fn a_session_without_a_heartbeat_is_never_stale() {
        let s = session("s", "p", "active", "working");
        assert!(!is_stale_working(&s, 10 * STALE_WORKING_AFTER_MS));
    }

    #[test]
    fn liveness_precedence_is_hibernated_then_stale_then_the_tier() {
        let mut s = session("s", "p", "active", "working");
        assert_eq!(row_liveness(&s, false), RowLiveness::Active);
        assert_eq!(row_liveness(&s, true), RowLiveness::Stale);
        s.liveness = cctui_proto::models::Liveness::Stale;
        assert_eq!(row_liveness(&s, false), RowLiveness::Stale);
        s.liveness = cctui_proto::models::Liveness::Dead;
        assert_eq!(row_liveness(&s, false), RowLiveness::Dead);
        s.hibernated = true;
        assert_eq!(row_liveness(&s, true), RowLiveness::Hibernated);
        assert_eq!(RowLiveness::Active.glyph(), "●");
    }

    #[test]
    fn a_grinding_session_is_not_asleep_and_a_quiet_one_is() {
        let now = 100 * MIN;
        let mut s = session("s", "p", "active", "working");
        s.tool_use_count = 14;
        s.last_tool_at = Some(at(now - 8_000));
        let act = tool_activity(&s, now);
        assert!(act.show && !act.asleep);
        assert_eq!(act.age_ms, Some(8_000));
        assert_eq!(cadence_text(&act).as_deref(), Some("⚙14 8s"));

        s.last_tool_at = Some(at(now - TOOL_ASLEEP_AFTER_MS - 1));
        assert!(tool_activity(&s, now).asleep);
    }

    #[test]
    fn a_non_working_session_is_never_asleep() {
        let now = 100 * MIN;
        let mut s = session("s", "p", "inactive", "done");
        s.last_tool_at = Some(at(now - 10 * MIN));
        let act = tool_activity(&s, now);
        assert!(!act.asleep);
        assert!(!act.show, "a done row has no live cadence to show");
    }

    #[test]
    fn todo_progress_counts_completions_and_reads_the_active_form() {
        let mut s = session("s", "p", "active", "working");
        s.todos = vec![
            todo("completed", "write it", None),
            todo("completed", "check it", None),
            todo("in_progress", "Run the tests", Some("Running tests")),
            todo("pending", "ship it", None),
        ];
        let act = tool_activity(&s, 0);
        assert_eq!((act.todo_done, act.todo_total), (2, 4));
        assert_eq!(act.todo_active.as_deref(), Some("Running tests"));
        assert_eq!(activity_text(&s, &act, false, 0).as_deref(), Some("2/4 Running tests"));
    }

    #[test]
    fn an_active_entry_without_an_active_form_falls_back_to_its_content() {
        let mut s = session("s", "p", "active", "working");
        s.todos = vec![todo("in_progress", "Bake the image", None)];
        assert_eq!(tool_activity(&s, 0).todo_active.as_deref(), Some("Bake the image"));
    }

    #[test]
    fn a_task_list_on_an_idle_row_shows_progress_but_no_active_line() {
        let mut s = session("s", "p", "inactive", "done");
        s.todos = vec![todo("in_progress", "Run the tests", Some("Running tests"))];
        let act = tool_activity(&s, 0);
        assert!(act.show, "a task list is worth showing even when the turn is over");
        assert_eq!(act.todo_active, None);
        assert_eq!(activity_text(&s, &act, false, 0).as_deref(), Some("0/1"));
    }

    #[test]
    fn the_activity_column_prefers_the_loudest_reason() {
        let now = 100 * MIN;
        let mut s = session("s", "p", "active", "working");
        s.activity_detail = Some("Reading files".to_owned());
        let act = tool_activity(&s, now);
        assert_eq!(activity_text(&s, &act, false, now).as_deref(), Some("Reading files"));

        s.last_heartbeat = Some(at(now - 42 * MIN));
        let stale = is_stale_working(&s, now);
        assert!(stale);
        let act = tool_activity(&s, now);
        assert_eq!(activity_text(&s, &act, stale, now).as_deref(), Some("stale 42m"));

        s.hibernated = true;
        assert_eq!(activity_text(&s, &act, stale, now).as_deref(), Some("hibernated"));
    }

    #[test]
    fn asleep_outranks_a_stale_activity_detail_but_not_staleness() {
        let now = 100 * MIN;
        let mut s = session("s", "p", "active", "working");
        s.activity_detail = Some("Reading files".to_owned());
        s.last_tool_at = Some(at(now - 5 * MIN));
        let act = tool_activity(&s, now);
        assert_eq!(activity_text(&s, &act, false, now).as_deref(), Some("asleep"));
    }

    #[test]
    fn ages_round_to_the_coarsest_unit() {
        assert_eq!(format_ago(0), "0s");
        assert_eq!(format_ago(-5_000), "0s");
        assert_eq!(format_ago(8_400), "8s");
        assert_eq!(format_ago(59_400), "59s");
        assert_eq!(format_ago(3 * MIN), "3m");
        assert_eq!(format_ago(90 * MIN), "1h");
    }

    #[test]
    fn a_plain_completion_is_not_a_badge_but_every_other_end_is() {
        let mut s = session("s", "p", "inactive", "done");
        assert_eq!(end_badge(&s), None);
        s.end_reason = Some(cctui_proto::models::SessionEndReason::Completed);
        assert_eq!(end_badge(&s), None);
        s.end_reason = Some(cctui_proto::models::SessionEndReason::DaemonLost);
        assert_eq!(end_badge(&s).as_deref(), Some("✕daemon lost"));
    }

    #[test]
    fn badges_render_in_triage_order_and_vanish_when_there_is_nothing_to_say() {
        let mut s = session("s", "p", "active", "working");
        assert!(RowBadges::of(&s, false, None, false).is_empty());
        assert_eq!(RowBadges::of(&s, false, None, false).text(), "");
        assert!(!RowBadges::of(&s, false, None, false).wants_you());

        s.unread_count = 4;
        s.auto_approve = true;
        s.hibernated = true;
        s.end_reason = Some(cctui_proto::models::SessionEndReason::Crashed);
        let badges = RowBadges::of(&s, true, Some("?"), false);
        assert!(!badges.is_empty());
        assert!(badges.wants_you());
        assert_eq!(
            badges.text(),
            "! ? ●4 ⚡ ✕crashed",
            "hibernation is the leading glyph and the activity word, not a third badge"
        );

        let quiet = RowBadges::of(&s, false, None, false);
        assert!(!quiet.wants_you(), "unread and auto-approve are reports, not requests");
        assert_eq!(quiet.text(), "●4 ⚡ ✕crashed");
    }

    #[test]
    fn a_foreign_job_earns_its_own_glyph_and_asks_for_nothing() {
        let mut s = session("s", "p", "active", "working");
        assert!(RowBadges::of(&s, false, None, false).is_empty());
        s.origin = cctui_proto::api::SessionOrigin::Foreign;
        let badges = RowBadges::of(&s, false, None, false);
        assert_eq!(badges.text(), "⌂");
        assert!(!badges.wants_you(), "it reports whose job it is, it does not ask");
        assert!(
            super::GLYPH_LEGEND.iter().any(|(glyph, _)| *glyph == "⌂"),
            "every row glyph is in the cheat sheet's legend"
        );
    }

    #[test]
    fn the_secondary_signals_each_earn_their_own_glyph() {
        let mut s = session("s", "p", "active", "working");
        s.cache_cold = true;
        assert_eq!(RowBadges::of(&s, false, None, false).text(), "❄");

        s.cache_cold = false;
        assert_eq!(RowBadges::of(&s, false, None, true).text(), "⏸");

        s.account_name = Some("main".to_owned());
        s.account_traffic_observed = false;
        assert_eq!(RowBadges::of(&s, false, None, false).text(), "⚠");
        s.account_traffic_observed = true;
        assert!(
            RowBadges::of(&s, false, None, false).is_empty(),
            "an account whose traffic was observed says nothing"
        );

        s.account_name = None;
        s.cache_cold = true;
        let all = RowBadges::of(&s, false, None, true);
        assert_eq!(all.text(), "⏸ ❄");
        assert!(!all.wants_you(), "a secondary signal reports, it does not ask");
    }

    #[test]
    fn truncation_is_char_safe_and_marks_the_cut() {
        assert_eq!(truncate("running", 0), "");
        assert_eq!(truncate("running", 7), "running");
        assert_eq!(truncate("running tests", 7), "runnin…");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
    }
}
