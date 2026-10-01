//! The diagnose panel and the session info popup: one row model, two modes.
//!
//! Which reasons apply is the server's call ([`cctui_proto::silence`]); this
//! module only words them.

use cctui_proto::api::SessionListItem;
use cctui_proto::diagnose::{DiagnoseFact, SessionDiagnoseResponse};
use cctui_proto::silence::SilenceReason;

use super::action::Effect;
use super::state::{App, View};

/// Which face of the one widget is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnoseMode {
    /// `D`: the dated facts plus why the session looks silent.
    Facts,
    /// `i`: identity and timestamps first, the facts underneath.
    Info,
}

impl DiagnoseMode {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Facts => " Diagnose ",
            Self::Info => " Session info ",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosePanel {
    pub session_id: String,
    pub mode: DiagnoseMode,
    pub report: Option<Box<SessionDiagnoseResponse>>,
    /// Why the fetch failed; the identity rows still render without it.
    pub error: Option<String>,
    pub loading: bool,
    pub scroll: usize,
}

impl DiagnosePanel {
    const fn new(session_id: String, mode: DiagnoseMode) -> Self {
        Self { session_id, mode, report: None, error: None, loading: true, scroll: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Codex,
    Opencode,
}

impl Harness {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Codex => "codex_",
            Self::Opencode => "opencode_",
        }
    }
}

pub fn fmt_age(ms: i64) -> String {
    match ms {
        ms if ms < 1_000 => format!("{ms}ms ago"),
        ms if ms < 60_000 => format!("{}s ago", ms / 1_000),
        ms if ms < 3_600_000 => format!("{}m ago", ms / 60_000),
        ms => format!("{}h ago", ms / 3_600_000),
    }
}

#[must_use]
pub fn fmt_age_opt(ms: Option<i64>) -> String {
    ms.map_or_else(|| "undated".to_owned(), fmt_age)
}

fn fmt_since(at: Option<chrono::DateTime<chrono::Utc>>, now_ms: i64) -> String {
    at.map_or_else(|| "—".to_owned(), |t| fmt_age((now_ms - t.timestamp_millis()).max(0)))
}

/// The wording of one server-decided reason.
#[must_use]
pub fn silence_text(reason: &SilenceReason) -> String {
    match reason {
        SilenceReason::CodexStalledRpc { count, age_ms } => format!(
            "{count} JSON-RPC request(s) outstanding and no frame for {}.",
            fmt_age(*age_ms)
        ),
        SilenceReason::CodexSharedDropped { count, age_ms } => format!(
            "The shared app-server connection dropped {count} in-flight request(s), last {} ago.",
            fmt_age(*age_ms)
        ),
        SilenceReason::CodexSharedNoFrames => "No JSON-RPC frames on the shared app-server \
             connection: inventory, history and lifecycle traffic is not flowing."
            .to_owned(),
        SilenceReason::CodexNoTurn => {
            "No turn is in flight, so codex has nothing to answer.".to_owned()
        }
        SilenceReason::CodexAuth { state } => {
            format!("Auth posture is not the gateway-bound one: {state}.")
        }
        SilenceReason::CodexRegistryMismatch { detail } => {
            format!("Registry and live state disagree: {detail}.")
        }
        SilenceReason::CodexNotLive => "No live app-server child owns this session.".to_owned(),
        SilenceReason::OpencodeSseDown => "The opencode event stream is not connected: nothing a \
             turn produces can reach cctui."
            .to_owned(),
        SilenceReason::OpencodeSseStalled { age_ms } => {
            format!("A turn is in flight but no event has arrived for {}.", fmt_age(*age_ms))
        }
        SilenceReason::OpencodeSseNoFrames => "HTTP calls are flowing but no event ever arrived \
             on the stream: the `GET /event` path is the blind spot."
            .to_owned(),
        SilenceReason::OpencodeHttpErrors { count, age_ms, message } => {
            format!("The server rejected {count} call(s), last {} ago: {message}", fmt_age(*age_ms))
        }
        SilenceReason::OpencodeAwaitingPermission { count } => {
            format!("{count} permission prompt(s) awaiting an answer.")
        }
        SilenceReason::OpencodeIdle => {
            "No turn is in flight, so opencode has nothing to answer.".to_owned()
        }
        SilenceReason::OpencodeVersion { version, pinned } => {
            format!("The running server is {version}, not the pinned {pinned}.")
        }
        SilenceReason::OpencodeNotLive => "No live `opencode serve` owns this session.".to_owned(),
    }
}

/// The wire tag of a reason, which is also what groups it by harness.
fn reason_kind(reason: &SilenceReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str().map(str::to_owned)))
        .unwrap_or_default()
}

/// Codex reasons keep their own section, opencode reasons theirs.
#[must_use]
pub fn silence_messages(reasons: &[SilenceReason], harness: Harness) -> Vec<String> {
    reasons
        .iter()
        .filter(|r| reason_kind(r).starts_with(harness.prefix()))
        .map(silence_text)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Heading,
    Normal,
    Dim,
    Warn,
    Error,
}

/// One rendered line of the panel. A heading carries an empty `value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

impl Row {
    fn new(label: impl Into<String>, value: impl Into<String>, tone: Tone) -> Self {
        Self { label: label.into(), value: value.into(), tone }
    }

    fn heading(label: impl Into<String>) -> Self {
        Self::new(label, "", Tone::Heading)
    }
}

/// `name [source, age]: value-or-reason`, the shape `cctui diagnose` prints.
fn fact_row<T: serde::Serialize>(name: &str, fact: &DiagnoseFact<T>) -> Row {
    let age = fmt_age_opt(fact.age_ms);
    let label = format!("{name} [{}, {age}]", fact.source);
    match &fact.value {
        Some(v) => Row::new(
            label,
            serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".to_owned()),
            Tone::Normal,
        ),
        None => Row::new(
            label,
            format!("— ({})", fact.missing_reason.as_deref().unwrap_or("missing")),
            Tone::Dim,
        ),
    }
}

/// Identity and timestamps: what the operator reads off a silent row before
/// asking the daemon anything.
#[must_use]
pub fn info_rows(s: &SessionListItem, now_ms: i64) -> Vec<Row> {
    let model = match (&s.model, &s.effort) {
        (Some(model), Some(effort)) => format!("{model} ({effort})"),
        (Some(model), None) => model.clone(),
        _ => "—".to_owned(),
    };
    let mut rows = vec![
        Row::heading("session"),
        Row::new("id", s.id.clone(), Tone::Normal),
        Row::new("parent", s.parent_id.clone().unwrap_or_else(|| "—".to_owned()), Tone::Dim),
        Row::new(
            "machine",
            s.machine_name.clone().unwrap_or_else(|| s.machine_id.clone()),
            Tone::Normal,
        ),
        Row::new("adapter", s.adapter_id.as_ref().map_or("claude-code", |a| a.as_str()), Tone::Dim),
        Row::new(
            "origin",
            if s.origin.is_foreign() { "foreign — cctui did not start it" } else { "cctui" },
            if s.origin.is_foreign() { Tone::Warn } else { Tone::Dim },
        ),
        Row::new(
            "account",
            s.account_name.clone().unwrap_or_else(|| "ambient".to_owned()),
            if s.account_name.is_some() && !s.account_traffic_observed {
                Tone::Warn
            } else {
                Tone::Normal
            },
        ),
        Row::new("model", model, Tone::Normal),
        Row::new(
            "permission mode",
            super::controls::permission_badge(s)
                .map(str::to_owned)
                .or_else(|| s.permission_mode.clone())
                .unwrap_or_else(|| "—".to_owned()),
            Tone::Dim,
        ),
        Row::new("registered", fmt_since(s.registered_at, now_ms), Tone::Dim),
        Row::new("last heartbeat", fmt_since(s.last_heartbeat, now_ms), Tone::Dim),
        Row::new("last tool", fmt_since(s.last_tool_at, now_ms), Tone::Dim),
    ];
    if let Some(reason) = s.end_reason {
        let detail = s.end_detail.as_ref().map_or_else(String::new, |d| format!(" — {d}"));
        let label = super::attention::end_reason_label(reason);
        rows.push(Row::new("ended", format!("{label}{detail}"), Tone::Error));
    }
    rows
}

/// Server facts, daemon facts and silence reasons — the panel's shared body.
#[must_use]
pub fn report_rows(resp: &SessionDiagnoseResponse, now_ms: i64) -> Vec<Row> {
    let mut rows = vec![Row::heading("server")];
    let s = &resp.server;
    rows.push(Row::new("status", s.status.clone().unwrap_or_else(|| "?".to_owned()), Tone::Normal));
    rows.push(Row::new(
        "account",
        if s.accounts.is_empty() { "—".to_owned() } else { s.accounts.join(", ") },
        if s.account_bound { Tone::Normal } else { Tone::Warn },
    ));
    rows.push(Row::new(
        "account bound",
        s.account_bound.to_string(),
        if s.account_bound { Tone::Normal } else { Tone::Warn },
    ));
    if let Some(seen) = s.machine_last_seen_ms {
        rows.push(Row::new("machine last seen", fmt_age((now_ms - seen).max(0)), Tone::Dim));
    }

    if let Some(err) = &resp.daemon_error {
        rows.push(Row::heading("daemon"));
        rows.push(Row::new("UNAVAILABLE", err.clone(), Tone::Error));
    }

    let Some(d) = &resp.daemon else { return rows };
    rows.push(Row::heading("daemon report"));
    rows.push(Row::new("adapter", d.adapter.clone(), Tone::Dim));
    rows.push(Row::new("short", d.short.clone().unwrap_or_else(|| "—".to_owned()), Tone::Dim));
    rows.push(fact_row("effective_state", &d.effective_state));
    rows.push(fact_row("last_hook_event", &d.last_hook_event));
    rows.push(fact_row("attach", &d.attach));
    rows.push(fact_row("pty_output", &d.pty_output));
    rows.push(fact_row("claude_socket", &d.claude_socket));
    rows.push(fact_row("transcript", &d.transcript));
    rows.push(fact_row("prompts", &d.prompts));
    rows.push(fact_row("permission_mode", &d.permission_mode));
    rows.push(fact_row("dispatch", &d.dispatch));
    rows.push(fact_row("gateway", &d.gateway));

    for (harness, present) in
        [(Harness::Codex, d.codex.is_some()), (Harness::Opencode, d.opencode.is_some())]
    {
        if !present {
            continue;
        }
        let messages = silence_messages(&resp.silence, harness);
        rows.push(Row::heading("why is it silent?"));
        if messages.is_empty() {
            rows.push(Row::new("", "live — nothing looks stuck", Tone::Dim));
        }
        // Full width: a reason is a sentence, not a value in a label column.
        for message in messages {
            rows.push(Row::new("", format!("· {message}"), Tone::Warn));
        }
    }
    rows
}

/// Everything the open panel shows, in order.
#[must_use]
pub fn panel_rows(app: &App) -> Vec<Row> {
    let Some(panel) = app.diagnose.as_ref() else { return Vec::new() };
    let mut rows = Vec::new();
    if panel.mode == DiagnoseMode::Info
        && let Some(s) = app.sessions.iter().find(|s| s.id == panel.session_id)
    {
        rows.extend(info_rows(s, app.clock_ms));
    }
    if let Some(report) = panel.report.as_ref() {
        rows.extend(report_rows(report, app.clock_ms));
    } else if panel.loading {
        rows.push(Row::new("", "fetching…", Tone::Dim));
    }
    if let Some(error) = panel.error.as_ref() {
        rows.push(Row::new("diagnose failed", error.clone(), Tone::Error));
    }
    rows
}

pub enum DiagnoseAction {
    /// Pressing the mode's own key again closes the panel.
    Open(DiagnoseMode),
    Close,
    Refresh,
    Scroll(i32),
    ScrollTop,
    CopyId,
    Loaded {
        session_id: String,
        report: Box<SessionDiagnoseResponse>,
    },
    Failed {
        session_id: String,
        error: String,
    },
    /// A `soft_limit_reached`/`cleared` frame for one session.
    SoftLimit {
        session_id: String,
        active: bool,
    },
}

pub fn reduce_diagnose(app: &mut App, action: DiagnoseAction) -> Vec<Effect> {
    match action {
        DiagnoseAction::Open(mode) => open(app, mode),
        DiagnoseAction::Close => {
            close(app);
            Vec::new()
        }
        DiagnoseAction::Refresh => {
            let Some(panel) = app.diagnose.as_mut() else { return Vec::new() };
            panel.loading = true;
            panel.error = None;
            vec![Effect::FetchDiagnose { session_id: panel.session_id.clone() }]
        }
        DiagnoseAction::Scroll(lines) => {
            let Some(panel) = app.diagnose.as_mut() else { return Vec::new() };
            panel.scroll = if lines < 0 {
                panel.scroll.saturating_sub(lines.unsigned_abs() as usize)
            } else {
                panel.scroll.saturating_add(lines as usize)
            };
            Vec::new()
        }
        DiagnoseAction::ScrollTop => {
            if let Some(panel) = app.diagnose.as_mut() {
                panel.scroll = 0;
            }
            Vec::new()
        }
        DiagnoseAction::CopyId => {
            let Some(panel) = app.diagnose.as_ref() else { return Vec::new() };
            let id = panel.session_id.clone();
            app.toast(super::toast::Level::Info, format!("copied {id}"));
            vec![Effect::Copy { text: id, label: "session id" }]
        }
        // A late reply for a panel that was closed or retargeted is dropped.
        DiagnoseAction::Loaded { session_id, report } => {
            if let Some(panel) = app.diagnose.as_mut().filter(|p| p.session_id == session_id) {
                panel.report = Some(report);
                panel.error = None;
                panel.loading = false;
            }
            Vec::new()
        }
        DiagnoseAction::Failed { session_id, error } => {
            if let Some(panel) = app.diagnose.as_mut().filter(|p| p.session_id == session_id) {
                panel.error = Some(error);
                panel.loading = false;
            }
            Vec::new()
        }
        DiagnoseAction::SoftLimit { session_id, active } => {
            if active {
                app.soft_limited.insert(session_id);
            } else {
                app.soft_limited.remove(&session_id);
            }
            Vec::new()
        }
    }
}

fn open(app: &mut App, mode: DiagnoseMode) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    if let Some(panel) = app.diagnose.as_ref()
        && panel.mode == mode
        && panel.session_id == session_id
    {
        close(app);
        return Vec::new();
    }
    // Switching face over the same session keeps the report already fetched.
    if let Some(panel) = app.diagnose.as_mut().filter(|p| p.session_id == session_id) {
        panel.mode = mode;
        panel.scroll = 0;
        return Vec::new();
    }
    app.diagnose = Some(DiagnosePanel::new(session_id.clone(), mode));
    app.router.push(View::Diagnose);
    vec![Effect::FetchDiagnose { session_id }]
}

fn close(app: &mut App) {
    app.diagnose = None;
    if app.view() == View::Diagnose {
        app.router.pop();
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::diagnose::SessionDiagnoseResponse;
    use cctui_proto::silence::SilenceReason;

    use super::{
        DiagnoseAction, DiagnoseMode, Harness, Tone, fmt_age, info_rows, panel_rows, report_rows,
        silence_messages, silence_text,
    };
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    const NOW: i64 = crate::testsupport::CLOCK_MS;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.clock_ms = NOW;
        app.update_aggregates();
        app
    }

    fn response() -> SessionDiagnoseResponse {
        crate::testsupport::diagnose_response()
    }

    fn dispatch(app: &mut App, action: DiagnoseAction) -> Vec<Effect> {
        reduce(app, Action::Diagnose(action))
    }

    fn every_reason() -> Vec<SilenceReason> {
        vec![
            SilenceReason::CodexStalledRpc { count: 2, age_ms: 120_000 },
            SilenceReason::CodexSharedDropped { count: 1, age_ms: 2_000 },
            SilenceReason::CodexSharedNoFrames,
            SilenceReason::CodexNoTurn,
            SilenceReason::CodexAuth { state: "no gateway env".into() },
            SilenceReason::CodexRegistryMismatch { detail: "registered but not live".into() },
            SilenceReason::CodexNotLive,
            SilenceReason::OpencodeSseDown,
            SilenceReason::OpencodeSseStalled { age_ms: 120_000 },
            SilenceReason::OpencodeSseNoFrames,
            SilenceReason::OpencodeHttpErrors {
                count: 1,
                age_ms: 2_000,
                message: "POST /prompt: 500".into(),
            },
            SilenceReason::OpencodeAwaitingPermission { count: 2 },
            SilenceReason::OpencodeIdle,
            SilenceReason::OpencodeVersion { version: "1.20.0".into(), pinned: "1.18.7".into() },
            SilenceReason::OpencodeNotLive,
        ]
    }

    #[test]
    fn every_code_the_server_can_send_is_worded() {
        for reason in every_reason() {
            assert!(!silence_text(&reason).is_empty(), "{reason:?} has no wording");
        }
    }

    #[test]
    fn the_numbers_a_code_supplies_reach_the_sentence() {
        let stalled = silence_text(&SilenceReason::CodexStalledRpc { count: 2, age_ms: 120_000 });
        assert!(stalled.contains('2'), "{stalled}");
        assert!(stalled.contains(&fmt_age(120_000)), "{stalled}");
        assert!(
            silence_text(&SilenceReason::OpencodeHttpErrors {
                count: 1,
                age_ms: 2_000,
                message: "POST /prompt: 500".into(),
            })
            .contains("500")
        );
        assert!(
            silence_text(&SilenceReason::OpencodeVersion {
                version: "1.20.0".into(),
                pinned: "1.18.7".into(),
            })
            .contains("1.20.0")
        );
    }

    #[test]
    fn each_harness_keeps_its_own_section() {
        let reasons = vec![
            SilenceReason::CodexNoTurn,
            SilenceReason::OpencodeIdle,
            SilenceReason::OpencodeNotLive,
        ];
        assert_eq!(silence_messages(&reasons, Harness::Codex).len(), 1);
        assert_eq!(silence_messages(&reasons, Harness::Opencode).len(), 2);
        assert!(silence_messages(&[], Harness::Codex).is_empty());
    }

    #[test]
    fn ages_read_in_the_largest_unit_that_fits() {
        assert_eq!(fmt_age(500), "500ms ago");
        assert_eq!(fmt_age(2_000), "2s ago");
        assert_eq!(fmt_age(120_000), "2m ago");
        assert_eq!(fmt_age(7_200_000), "2h ago");
    }

    /// The panel is the CLI printer as widgets, so it must name every fact
    /// `cctui diagnose` prints.
    #[test]
    fn the_facts_match_what_the_cli_prints() {
        let rows = report_rows(&response(), NOW);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        for fact in [
            "effective_state",
            "last_hook_event",
            "attach",
            "pty_output",
            "claude_socket",
            "transcript",
            "prompts",
            "permission_mode",
            "dispatch",
            "gateway",
        ] {
            assert!(
                labels.iter().any(|l| l.starts_with(fact)),
                "the panel drops the `{fact}` fact"
            );
        }
        let state = rows.iter().find(|r| r.label.starts_with("effective_state")).expect("the fact");
        assert!(state.label.contains("activity"), "the source belongs on the row: {state:?}");
        assert!(state.label.contains("1s ago"), "the age belongs on the row: {state:?}");
        assert!(state.value.contains("active/working"));
    }

    #[test]
    fn a_missing_fact_shows_its_reason_not_a_blank() {
        let rows = report_rows(&response(), NOW);
        let pty = rows.iter().find(|r| r.label.starts_with("pty_output")).expect("the fact");
        assert_eq!(pty.value, "— (PTY capture not implemented)");
        assert_eq!(pty.tone, Tone::Dim);
        assert!(pty.label.contains("undated"));
    }

    #[test]
    fn an_unreachable_daemon_still_reports_the_server_facts() {
        let mut resp = response();
        resp.daemon = None;
        resp.daemon_error = Some("no daemon connected".into());
        let rows = report_rows(&resp, NOW);
        assert!(rows.iter().any(|r| r.label == "status" && r.value == "active"));
        assert!(rows.iter().any(|r| r.value == "no daemon connected" && r.tone == Tone::Error));
    }

    #[test]
    fn silence_reasons_render_under_their_own_heading() {
        let mut resp = response();
        resp.daemon.as_mut().expect("a report").codex = Some(codex_section());
        resp.silence = vec![SilenceReason::CodexNoTurn];
        let rows = report_rows(&resp, NOW);
        assert!(rows.iter().any(|r| r.label == "why is it silent?"));
        let reason =
            rows.iter().find(|r| r.value.contains("nothing to answer")).expect("the worded reason");
        assert_eq!(reason.tone, Tone::Warn);
        assert!(reason.label.is_empty(), "a reason takes the whole row, not a value column");
    }

    #[test]
    fn a_healthy_harness_says_so_instead_of_listing_nothing() {
        let mut resp = response();
        resp.daemon.as_mut().expect("a report").codex = Some(codex_section());
        let rows = report_rows(&resp, NOW);
        assert!(rows.iter().any(|r| r.value.contains("nothing looks stuck")));
    }

    fn codex_section() -> cctui_proto::diagnose::CodexDiagnose {
        cctui_proto::diagnose::CodexDiagnose {
            codex_version: None,
            min_version: "0.153.0".into(),
            version_supported: None,
            transport: "stdio".into(),
            app_server_pid: None,
            live: true,
            registered: true,
            thread_id: None,
            active_turn_id: None,
            turn_status: "idle".into(),
            pending_rpc_count: 0,
            pending_rpc_methods: vec![],
            protocol_errors: vec![],
            stderr_tail: vec![],
            rpc_tail: vec![],
            rollout_path: None,
            rollout_size_bytes: None,
            auth_state: None,
            registry_live_mismatch: None,
        }
    }

    #[test]
    fn the_info_rows_carry_the_identity_and_the_timestamps() {
        let mut s = session("s-a", "alpha", "active", "working");
        s.parent_id = Some("s-parent".into());
        s.model = Some("opus".into());
        s.effort = Some("high".into());
        s.permission_mode = Some("yolo".into());
        s.account_name = Some("main".into());
        s.registered_at = chrono::DateTime::from_timestamp_millis(NOW - 3_600_000);
        s.last_heartbeat = chrono::DateTime::from_timestamp_millis(NOW - 2_000);
        s.last_tool_at = chrono::DateTime::from_timestamp_millis(NOW - 60_000);

        let rows = info_rows(&s, NOW);
        let get = |label: &str| {
            rows.iter().find(|r| r.label == label).map(|r| r.value.clone()).unwrap_or_default()
        };
        assert_eq!(get("id"), "s-a");
        assert_eq!(get("parent"), "s-parent");
        assert_eq!(get("model"), "opus (high)");
        assert_eq!(get("permission mode"), "yolo");
        assert_eq!(get("registered"), "1h ago");
        assert_eq!(get("last heartbeat"), "2s ago");
        assert_eq!(get("last tool"), "1m ago");
        assert!(rows.iter().all(|r| r.label != "ended"), "a live session has no end row");
    }

    #[test]
    fn an_account_with_no_observed_traffic_is_flagged_warn() {
        let mut s = session("s-a", "alpha", "active", "working");
        s.account_name = Some("main".into());
        s.account_traffic_observed = false;
        let rows = info_rows(&s, NOW);
        let account = rows.iter().find(|r| r.label == "account").expect("the row");
        assert_eq!(account.tone, Tone::Warn);

        s.account_traffic_observed = true;
        let rows = info_rows(&s, NOW);
        assert_eq!(rows.iter().find(|r| r.label == "account").expect("the row").tone, Tone::Normal);
    }

    #[test]
    fn an_ended_session_names_its_reason() {
        let mut s = session("s-a", "alpha", "inactive", "done");
        s.end_reason = Some(cctui_proto::models::SessionEndReason::DaemonLost);
        s.end_detail = Some("machine went away".into());
        let rows = info_rows(&s, NOW);
        let ended = rows.iter().find(|r| r.label == "ended").expect("the row");
        assert_eq!(
            ended.value, "daemon lost — machine went away",
            "the reason is worded, not Debug"
        );
        assert_eq!(ended.tone, Tone::Error);
    }

    #[test]
    fn opening_pushes_the_overlay_and_asks_the_server_once() {
        let mut app = app();
        let effects = dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        assert!(
            matches!(effects.as_slice(), [Effect::FetchDiagnose { session_id }] if session_id == "s-a")
        );
        assert_eq!(app.view(), View::Diagnose);
        assert!(app.diagnose.as_ref().expect("a panel").loading);
    }

    #[test]
    fn the_same_key_again_closes_the_panel() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        assert!(dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts)).is_empty());
        assert!(app.diagnose.is_none());
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn switching_face_over_the_same_session_refetches_nothing() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        dispatch(
            &mut app,
            DiagnoseAction::Loaded { session_id: "s-a".to_owned(), report: Box::new(response()) },
        );
        let effects = dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Info));
        assert!(effects.is_empty(), "the report already in hand is reused");
        let panel = app.diagnose.as_ref().expect("a panel");
        assert_eq!(panel.mode, DiagnoseMode::Info);
        assert!(panel.report.is_some());
    }

    #[test]
    fn the_info_face_puts_the_identity_above_the_report() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Info));
        dispatch(
            &mut app,
            DiagnoseAction::Loaded { session_id: "s-a".to_owned(), report: Box::new(response()) },
        );
        let rows = panel_rows(&app);
        let id = rows.iter().position(|r| r.label == "id").expect("the id row");
        let server = rows.iter().position(|r| r.label == "server").expect("the server heading");
        assert!(id < server, "identity comes first in the info face");
    }

    #[test]
    fn the_facts_face_leaves_the_identity_out() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        dispatch(
            &mut app,
            DiagnoseAction::Loaded { session_id: "s-a".to_owned(), report: Box::new(response()) },
        );
        let rows = panel_rows(&app);
        assert!(rows.iter().all(|r| r.label != "id"));
        assert!(rows.iter().any(|r| r.label == "server"));
    }

    #[test]
    fn refresh_asks_again_and_clears_the_last_error() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        dispatch(
            &mut app,
            DiagnoseAction::Failed { session_id: "s-a".to_owned(), error: "boom".to_owned() },
        );
        assert_eq!(app.diagnose.as_ref().expect("a panel").error.as_deref(), Some("boom"));
        assert!(panel_rows(&app).iter().any(|r| r.label == "diagnose failed"));

        let effects = dispatch(&mut app, DiagnoseAction::Refresh);
        assert!(matches!(effects.as_slice(), [Effect::FetchDiagnose { .. }]));
        let panel = app.diagnose.as_ref().expect("a panel");
        assert!(panel.error.is_none() && panel.loading);
    }

    #[test]
    fn a_reply_for_another_session_is_dropped() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        dispatch(
            &mut app,
            DiagnoseAction::Loaded {
                session_id: "s-other".to_owned(),
                report: Box::new(response()),
            },
        );
        let panel = app.diagnose.as_ref().expect("a panel");
        assert!(panel.report.is_none() && panel.loading);
    }

    #[test]
    fn scrolling_never_goes_below_the_top() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts));
        dispatch(&mut app, DiagnoseAction::Scroll(-5));
        assert_eq!(app.diagnose.as_ref().expect("a panel").scroll, 0);
        dispatch(&mut app, DiagnoseAction::Scroll(3));
        assert_eq!(app.diagnose.as_ref().expect("a panel").scroll, 3);
    }

    #[test]
    fn copying_the_id_hands_it_to_the_clipboard_effect() {
        let mut app = app();
        dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Info));
        let effects = dispatch(&mut app, DiagnoseAction::CopyId);
        assert!(
            matches!(effects.as_slice(), [Effect::Copy { text, label }] if text == "s-a" && *label == "session id")
        );
    }

    #[test]
    fn a_soft_limit_frame_marks_and_clears_the_session() {
        let mut app = app();
        dispatch(
            &mut app,
            DiagnoseAction::SoftLimit { session_id: "s-a".to_owned(), active: true },
        );
        assert!(app.soft_limited.contains("s-a"));
        dispatch(
            &mut app,
            DiagnoseAction::SoftLimit { session_id: "s-a".to_owned(), active: false },
        );
        assert!(app.soft_limited.is_empty());
    }

    #[test]
    fn opening_with_no_session_selected_does_nothing() {
        let mut app = App::new();
        assert!(dispatch(&mut app, DiagnoseAction::Open(DiagnoseMode::Facts)).is_empty());
        assert!(app.diagnose.is_none());
    }
}
