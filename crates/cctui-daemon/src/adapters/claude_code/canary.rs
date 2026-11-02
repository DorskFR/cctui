//! Flags a worker whose cctui hooks or MCP relay a managed policy silently
//! dropped. Any later hook or relay connect clears the flag: whether
//! `SessionStart` fires on every launch path is not guaranteed.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub(super) const WINDOW: Duration = Duration::from_secs(30);

/// A watch whose worker never shows up is dropped after this long; the spawn
/// watchdog owns the never-started case.
const STALE: Duration = Duration::from_mins(5);

const EXITED_WITHOUT_DETAIL: &str = "worker exited before cctui hooks loaded; Claude Code may \
                                     have refused a launch flag (disableSideloadFlags)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    HooksBlocked,
    McpBlocked,
    ExitedEarly(Option<String>),
}

impl Verdict {
    pub(super) fn notice(&self) -> String {
        let secs = WINDOW.as_secs();
        match self {
            Self::HooksBlocked => format!(
                "cctui hooks not loaded within {secs}s: a managed policy (allowManagedHooksOnly) \
                 may be blocking them, check /status in the session. Questions, permission \
                 prompts and turn ends will not reach cctui."
            ),
            Self::McpBlocked => format!(
                "cctui MCP server not connected within {secs}s while hooks fire: a managed \
                 policy (allowManagedMcpServersOnly) may be blocking it. CctuiAgent and the \
                 other cctui tools are unavailable in this session."
            ),
            Self::ExitedEarly(Some(detail)) => {
                format!("worker exited before cctui hooks loaded: {detail}")
            }
            Self::ExitedEarly(None) => EXITED_WITHOUT_DETAIL.to_owned(),
        }
    }

    const fn cleared_notice(&self) -> Option<&'static str> {
        match self {
            Self::HooksBlocked => Some("cctui hooks are firing; the earlier warning is cleared."),
            Self::McpBlocked => Some("cctui MCP server connected; the earlier warning is cleared."),
            Self::ExitedEarly(_) => None,
        }
    }
}

/// One roster entry as the canary needs it.
#[derive(Debug, Clone)]
pub(super) struct Observed {
    pub short: String,
    pub exited: bool,
    pub detail: Option<String>,
    /// Epoch ms at which the supervisor (re)started the worker.
    pub started_at_ms: Option<i64>,
}

#[derive(Debug)]
struct Watch {
    short: String,
    claude_session: String,
    mcp_session: Option<String>,
    armed_at: Instant,
    armed_at_ms: i64,
    live_since: Option<Instant>,
    pinged: bool,
    flagged: Option<Verdict>,
}

#[derive(Debug, Default)]
pub(super) struct Canary {
    watches: HashMap<String, Watch>,
}

impl Canary {
    /// Watch the launch of `local_id`. `claude_session` is the id Claude Code
    /// reports in hook payloads; `mcp_session` is set when a relay is expected.
    pub(super) fn arm(
        &mut self,
        local_id: &str,
        short: &str,
        claude_session: &str,
        mcp_session: Option<&str>,
        now: Instant,
        now_ms: i64,
    ) {
        self.watches.insert(
            local_id.to_owned(),
            Watch {
                short: short.to_owned(),
                claude_session: claude_session.to_owned(),
                mcp_session: mcp_session.map(str::to_owned),
                armed_at: now,
                armed_at_ms: now_ms,
                live_since: None,
                pinged: false,
                flagged: None,
            },
        );
    }

    /// A hook delivery for `id`, which is either a `local_id` or the Claude
    /// Code session id.
    pub(super) fn ping(&mut self, id: &str) {
        for (local_id, watch) in &mut self.watches {
            if local_id == id || watch.claude_session == id {
                watch.pinged = true;
            }
        }
    }

    pub(super) fn flagged(&self, local_id: &str) -> Option<&Verdict> {
        self.watches.get(local_id).and_then(|w| w.flagged.as_ref())
    }

    /// Advance every watch against one roster poll; returns the notices to
    /// post, keyed by `local_id`.
    pub(super) fn observe(
        &mut self,
        roster: &[Observed],
        now: Instant,
        mcp_ready: &impl Fn(&str) -> bool,
    ) -> Vec<(String, String)> {
        let mut notices = Vec::new();
        self.watches.retain(|local_id, watch| {
            let job = roster.iter().find(|j| j.short == watch.short);
            match step(watch, job, now, mcp_ready) {
                Step::Wait => true,
                Step::Done => false,
                Step::Flag(verdict) => {
                    notices.push((local_id.clone(), verdict.notice()));
                    let keep = !matches!(verdict, Verdict::ExitedEarly(_));
                    watch.flagged = Some(verdict);
                    keep
                }
                Step::Clear(text) => {
                    notices.push((local_id.clone(), text.to_owned()));
                    false
                }
            }
        });
        notices
    }
}

enum Step {
    Wait,
    Done,
    Flag(Verdict),
    Clear(&'static str),
}

fn step(
    watch: &mut Watch,
    job: Option<&Observed>,
    now: Instant,
    mcp_ready: &impl Fn(&str) -> bool,
) -> Step {
    let mcp_up = watch.mcp_session.as_deref().is_none_or(mcp_ready);
    if let Some(flag) = &watch.flagged {
        let recovered = match flag {
            Verdict::HooksBlocked => watch.pinged,
            Verdict::McpBlocked => mcp_up,
            Verdict::ExitedEarly(_) => true,
        };
        return match (recovered, flag.cleared_notice()) {
            (true, Some(text)) => Step::Clear(text),
            (true, None) => Step::Done,
            (false, _) if job.is_none() => Step::Done,
            (false, _) => Step::Wait,
        };
    }
    if watch.pinged && mcp_up {
        return Step::Done;
    }
    let Some(job) = job else {
        if watch.live_since.is_some() && !watch.pinged {
            return Step::Flag(Verdict::ExitedEarly(None));
        }
        let stale = watch.live_since.is_none() && now.duration_since(watch.armed_at) >= STALE;
        return if stale { Step::Done } else { Step::Wait };
    };
    if job.exited {
        let this_launch =
            watch.live_since.is_some() || job.started_at_ms.is_some_and(|t| t >= watch.armed_at_ms);
        return match (this_launch, watch.pinged) {
            (true, false) => Step::Flag(Verdict::ExitedEarly(job.detail.clone())),
            (true, true) => Step::Done,
            (false, _) => Step::Wait,
        };
    }
    let since = *watch.live_since.get_or_insert(now);
    if now.duration_since(since) < WINDOW {
        return Step::Wait;
    }
    Step::Flag(if watch.pinged { Verdict::McpBlocked } else { Verdict::HooksBlocked })
}

static CANARY: LazyLock<Mutex<Canary>> = LazyLock::new(|| Mutex::new(Canary::default()));

pub(super) fn arm(local_id: &str, short: &str, claude_session: &str, mcp_session: Option<&str>) {
    let now_ms = super::diagnose::now_unix_ms();
    if let Ok(mut c) = CANARY.lock() {
        c.arm(local_id, short, claude_session, mcp_session, Instant::now(), now_ms);
    }
}

pub(super) fn ping(id: &str) {
    if let Ok(mut c) = CANARY.lock() {
        c.ping(id);
    }
}

pub(super) fn flagged_notice(local_id: &str) -> Option<String> {
    CANARY.lock().ok()?.flagged(local_id).map(Verdict::notice)
}

pub(super) fn observe(roster: &[Observed]) -> Vec<(String, String)> {
    CANARY.lock().map_or_else(
        |_| Vec::new(),
        |mut c| c.observe(roster, Instant::now(), &crate::mcpready::is_ready),
    )
}

/// The `Message` payload the session timeline renders as a marker.
pub(super) fn marker_payload(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "system_marker", "marker": "cctui/canary", "text": text })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0_MS: i64 = 1_000_000;

    fn live(short: &str) -> Observed {
        Observed { short: short.to_owned(), exited: false, detail: None, started_at_ms: None }
    }

    fn armed(mcp: Option<&str>) -> (Canary, Instant) {
        let mut c = Canary::default();
        let t0 = Instant::now();
        c.arm("loc-1", "abcd1234", "claude-1", mcp, t0, T0_MS);
        (c, t0)
    }

    fn never(_: &str) -> bool {
        false
    }

    #[test]
    fn a_live_worker_without_any_hook_ping_is_flagged_after_the_window() {
        let (mut c, t0) = armed(None);
        let roster = [live("abcd1234")];
        assert!(c.observe(&roster, t0, &never).is_empty());
        assert!(c.observe(&roster, t0 + WINDOW - Duration::from_secs(1), &never).is_empty());
        let notices = c.observe(&roster, t0 + WINDOW, &never);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].0, "loc-1");
        assert!(notices[0].1.contains("cctui hooks not loaded"), "{}", notices[0].1);
        assert!(notices[0].1.contains("/status"), "{}", notices[0].1);
        assert_eq!(c.flagged("loc-1"), Some(&Verdict::HooksBlocked));
        assert!(c.observe(&roster, t0 + WINDOW * 2, &never).is_empty(), "flagged once");
    }

    #[test]
    fn a_session_start_ping_keeps_the_session_healthy() {
        let (mut c, t0) = armed(None);
        let roster = [live("abcd1234")];
        assert!(c.observe(&roster, t0, &never).is_empty());
        c.ping("claude-1");
        assert!(c.observe(&roster, t0 + WINDOW * 2, &never).is_empty());
        assert_eq!(c.flagged("loc-1"), None);
        assert!(c.watches.is_empty(), "a healthy launch stops being watched");
    }

    #[test]
    fn a_ping_by_local_id_counts_too() {
        let (mut c, t0) = armed(None);
        c.ping("loc-1");
        assert!(c.observe(&[live("abcd1234")], t0 + WINDOW, &never).is_empty());
        assert_eq!(c.flagged("loc-1"), None);
    }

    #[test]
    fn hooks_firing_without_a_relay_initialize_flag_the_mcp_server_as_blocked() {
        let (mut c, t0) = armed(Some("loc-1"));
        let roster = [live("abcd1234")];
        c.ping("claude-1");
        assert!(c.observe(&roster, t0, &never).is_empty());
        let notices = c.observe(&roster, t0 + WINDOW, &never);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].1.contains("cctui MCP server not connected"), "{}", notices[0].1);
        assert_eq!(c.flagged("loc-1"), Some(&Verdict::McpBlocked));
    }

    #[test]
    fn a_relay_that_connects_keeps_the_session_healthy() {
        let (mut c, t0) = armed(Some("loc-1"));
        c.ping("claude-1");
        assert!(c.observe(&[live("abcd1234")], t0 + WINDOW, &|s| s == "loc-1").is_empty());
        assert_eq!(c.flagged("loc-1"), None);
    }

    #[test]
    fn a_late_hook_clears_the_hooks_flag() {
        let (mut c, t0) = armed(None);
        let roster = [live("abcd1234")];
        c.observe(&roster, t0, &never);
        assert_eq!(c.observe(&roster, t0 + WINDOW, &never).len(), 1);
        c.ping("claude-1");
        let notices = c.observe(&roster, t0 + WINDOW * 2, &never);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].1.contains("cleared"), "{}", notices[0].1);
        assert_eq!(c.flagged("loc-1"), None);
    }

    #[test]
    fn a_late_relay_clears_the_mcp_flag() {
        let (mut c, t0) = armed(Some("loc-1"));
        let roster = [live("abcd1234")];
        c.ping("claude-1");
        c.observe(&roster, t0, &never);
        assert_eq!(c.observe(&roster, t0 + WINDOW, &never).len(), 1);
        let notices = c.observe(&roster, t0 + WINDOW * 2, &|s| s == "loc-1");
        assert_eq!(notices.len(), 1);
        assert!(notices[0].1.contains("MCP server connected"), "{}", notices[0].1);
    }

    #[test]
    fn the_window_starts_when_the_worker_goes_live_not_at_arm() {
        let (mut c, t0) = armed(None);
        assert!(c.observe(&[], t0 + WINDOW * 2, &never).is_empty(), "not listed yet");
        let roster = [live("abcd1234")];
        let live_at = t0 + WINDOW * 2;
        assert!(c.observe(&roster, live_at, &never).is_empty());
        assert!(c.observe(&roster, live_at + WINDOW - Duration::from_secs(1), &never).is_empty());
        assert_eq!(c.observe(&roster, live_at + WINDOW, &never).len(), 1);
    }

    #[test]
    fn a_worker_that_exits_before_any_hook_reports_its_detail() {
        let (mut c, t0) = armed(None);
        let roster = [Observed {
            short: "abcd1234".into(),
            exited: true,
            detail: Some("--mcp-config is disabled by policy".into()),
            started_at_ms: Some(T0_MS + 10),
        }];
        let notices = c.observe(&roster, t0 + Duration::from_secs(1), &never);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].1,
            "worker exited before cctui hooks loaded: --mcp-config is disabled by policy"
        );
        assert!(c.watches.is_empty());
    }

    #[test]
    fn a_dead_listing_from_before_the_relaunch_is_not_this_launch() {
        let (mut c, t0) = armed(None);
        let stale = [Observed {
            short: "abcd1234".into(),
            exited: true,
            detail: Some("process gone".into()),
            started_at_ms: Some(T0_MS - 60_000),
        }];
        assert!(c.observe(&stale, t0 + Duration::from_secs(1), &never).is_empty());
        assert!(c.observe(&[live("abcd1234")], t0 + Duration::from_secs(2), &never).is_empty());
    }

    #[test]
    fn a_worker_that_vanishes_after_going_live_without_a_hook_is_reported() {
        let (mut c, t0) = armed(None);
        c.observe(&[live("abcd1234")], t0, &never);
        let notices = c.observe(&[], t0 + Duration::from_secs(3), &never);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].1.contains("disableSideloadFlags"), "{}", notices[0].1);
    }

    #[test]
    fn a_launch_that_never_lists_is_dropped_without_a_notice() {
        let (mut c, t0) = armed(None);
        assert!(c.observe(&[], t0 + STALE, &never).is_empty());
        assert!(c.watches.is_empty());
    }

    #[test]
    fn rearming_resets_an_earlier_flag() {
        let (mut c, t0) = armed(None);
        let roster = [live("abcd1234")];
        c.observe(&roster, t0, &never);
        c.observe(&roster, t0 + WINDOW, &never);
        assert!(c.flagged("loc-1").is_some());
        c.arm("loc-1", "abcd1234", "claude-1", None, t0 + WINDOW, T0_MS + 30_000);
        assert_eq!(c.flagged("loc-1"), None);
    }

    #[test]
    fn the_marker_payload_is_a_timeline_system_marker() {
        let p = marker_payload("hello");
        assert_eq!(p["role"], "system_marker");
        assert_eq!(p["text"], "hello");
    }
}
