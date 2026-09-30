//! What a session wants from you: pending permission cards, the needs-you
//! indicator, and the activity/end state the banner renders.

use cctui_proto::api::SessionListItem;
use cctui_proto::classifier::Bucket;
use cctui_proto::models::{SessionEndReason, SessionStatus};
use cctui_proto::session_end::{EndTone, end_badge_detail};

use super::action::Effect;
use super::conversation;
use super::state::{App, PendingPermission};
use super::toast::Level;

/// Nothing for this long and the banner calls the session silent.
pub const SILENT_AFTER_SECS: i64 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    /// Allow this one and put the session into auto-approve.
    AllowAlways,
}

impl Decision {
    const fn behavior(self) -> &'static str {
        match self {
            Self::Allow | Self::AllowAlways => "allow",
            Self::Deny => "deny",
        }
    }
}

pub enum AttentionAction {
    PermissionRequested(PendingPermission),
    PermissionResolved {
        session_id: String,
        request_id: String,
    },
    /// The server's own pending list, fetched on start and after a reconnect.
    PendingPermissionsLoaded(Vec<PendingPermission>),
    /// Answer the card the focused session is showing.
    Respond(Decision),
    JumpToPending,
    SessionEnded {
        session_id: String,
        reason: SessionEndReason,
        detail: Option<String>,
    },
}

/// Every pending permission request, in arrival order, whatever session it
/// belongs to. Requests are never modal: a card renders inside its own
/// session's transcript and only that session's keys can answer it.
#[derive(Debug, Default)]
pub struct PermissionInbox {
    items: Vec<PendingPermission>,
}

impl PermissionInbox {
    pub fn push(&mut self, req: PendingPermission) {
        if self.contains(&req.session_id, &req.request_id) {
            return;
        }
        self.items.push(req);
    }

    /// The server's list is authoritative: it has both what we missed while
    /// disconnected and nothing that was resolved elsewhere.
    pub fn replace_all(&mut self, items: Vec<PendingPermission>) {
        self.items = items;
    }

    pub fn resolve(&mut self, session_id: &str, request_id: &str) {
        self.items.retain(|p| !(p.session_id == session_id && p.request_id == request_id));
    }

    pub fn drop_session(&mut self, session_id: &str) {
        self.items.retain(|p| p.session_id != session_id);
    }

    fn contains(&self, session_id: &str, request_id: &str) -> bool {
        self.items.iter().any(|p| p.session_id == session_id && p.request_id == request_id)
    }

    pub fn for_session<'a>(
        &'a self,
        session_id: &'a str,
    ) -> impl Iterator<Item = &'a PendingPermission> {
        self.items.iter().filter(move |p| p.session_id == session_id)
    }

    /// The card a keystroke answers: the session's oldest request.
    pub fn head(&self, session_id: &str) -> Option<&PendingPermission> {
        self.for_session(session_id).next()
    }

    /// The hook a session row reads to mark itself as waiting on you.
    pub fn has(&self, session_id: &str) -> bool {
        self.items.iter().any(|p| p.session_id == session_id)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Waiting sessions in arrival order, each listed once.
    pub fn sessions(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for item in &self.items {
            if !out.contains(&item.session_id.as_str()) {
                out.push(&item.session_id);
            }
        }
        out
    }

    /// The next waiting session after `current`, wrapping round.
    pub fn next_after(&self, current: Option<&str>) -> Option<String> {
        let sessions = self.sessions();
        if sessions.is_empty() {
            return None;
        }
        let start = current
            .and_then(|id| sessions.iter().position(|s| *s == id))
            .map_or(0, |index| (index + 1) % sessions.len());
        sessions.get(start).map(|id| (*id).to_owned())
    }
}

pub fn reduce_attention(app: &mut App, action: AttentionAction) -> Vec<Effect> {
    match action {
        AttentionAction::PermissionRequested(req) => {
            app.permissions.push(req);
            Vec::new()
        }
        AttentionAction::PermissionResolved { session_id, request_id } => {
            app.permissions.resolve(&session_id, &request_id);
            Vec::new()
        }
        AttentionAction::PendingPermissionsLoaded(items) => {
            app.permissions.replace_all(items);
            Vec::new()
        }
        AttentionAction::Respond(decision) => respond(app, decision),
        AttentionAction::JumpToPending => jump_to_pending(app),
        AttentionAction::SessionEnded { session_id, reason, detail } => {
            end_session(app, &session_id, reason, detail)
        }
    }
}

/// A keystroke only ever answers the session on screen, so a request that
/// arrives for another session cannot hijack the answer.
fn respond(app: &mut App, decision: Decision) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let Some(request_id) = app.permissions.head(&session_id).map(|p| p.request_id.clone()) else {
        return Vec::new();
    };
    app.permissions.resolve(&session_id, &request_id);

    let mut effects = vec![Effect::RespondPermission {
        session_id: session_id.clone(),
        request_id,
        behavior: decision.behavior(),
    }];
    if decision == Decision::AllowAlways {
        effects.push(Effect::SetAutoApprove { session_id, enabled: true });
    }
    effects
}

fn jump_to_pending(app: &mut App) -> Vec<Effect> {
    let current = app.selected_session_id();
    let Some(target) = app.permissions.next_after(current.as_deref()) else {
        app.toast(Level::Info, "nothing is waiting for approval");
        return Vec::new();
    };
    let Some(index) = app.flattened_sessions().iter().position(|s| s.id == target) else {
        return Vec::new();
    };
    app.selected_index = index;
    let mut effects = conversation::leave(app);
    effects.extend(conversation::open(app, target));
    effects
}

fn end_session(
    app: &mut App,
    session_id: &str,
    reason: SessionEndReason,
    detail: Option<String>,
) -> Vec<Effect> {
    app.permissions.drop_session(session_id);
    let ended_at = chrono::DateTime::from_timestamp_millis(app.clock_ms);
    if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
        session.status = SessionStatus::Inactive;
        session.end_reason = Some(reason);
        session.end_detail = detail.clone();
        session.ended_at = ended_at;
    }
    app.update_aggregates();

    let badge = EndBadge::new(reason, detail.as_deref());
    if badge.muted {
        return Vec::new();
    }
    let suffix = badge.detail.map_or_else(String::new, |d| format!(" — {d}"));
    let short = session_id.get(..8).unwrap_or(session_id);
    app.toast(Level::Info, format!("{short} ended: {}{suffix}", badge.label));
    Vec::new()
}

/// How an ended session is announced. Tone and muting come from the shared
/// `cctui_proto` table; only the wording is the client's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndBadge {
    pub label: &'static str,
    pub tone: EndTone,
    pub muted: bool,
    /// The first line of a failed start's detail — the whole story for those.
    pub detail: Option<String>,
}

impl EndBadge {
    pub fn new(reason: SessionEndReason, detail: Option<&str>) -> Self {
        Self {
            label: end_reason_label(reason),
            tone: reason.tone(),
            muted: reason.muted(),
            detail: end_badge_detail(reason, detail),
        }
    }

    /// `None` while the session is alive.
    pub fn of(session: &SessionListItem) -> Option<Self> {
        let reason = session.end_reason?;
        Some(Self::new(reason, session.end_detail.as_deref()))
    }
}

pub const fn end_reason_label(reason: SessionEndReason) -> &'static str {
    match reason {
        SessionEndReason::Completed => "completed",
        SessionEndReason::Killed => "killed",
        SessionEndReason::Crashed => "crashed",
        SessionEndReason::DaemonLost => "daemon lost",
        SessionEndReason::MachineOffline => "machine offline",
        SessionEndReason::ReapedInactive => "reaped",
        SessionEndReason::ResumeFailed => "resume failed",
        SessionEndReason::SpawnFailed => "failed",
        SessionEndReason::Other => "ended",
    }
}

/// What the banner above the composer says about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Ended(EndBadge),
    /// The agent is blocked on you: a card is up, or the classifier says so.
    NeedsInput,
    Working {
        detail: Option<String>,
        /// Age of the last thing that happened; `None` when unreported.
        age_secs: Option<i64>,
    },
    Silent {
        secs: i64,
    },
    Idle,
}

/// Pure: the clock comes from [`App::clock_ms`], never from the system.
pub fn activity(app: &App, session: &SessionListItem) -> Activity {
    if let Some(badge) = EndBadge::of(session) {
        return Activity::Ended(badge);
    }
    if app.permissions.has(&session.id) || session.bucket == Bucket::Blocked {
        return Activity::NeedsInput;
    }
    if session.status != SessionStatus::Active {
        return Activity::Idle;
    }
    let age = last_activity_age(app, session);
    match age {
        Some(secs) if secs >= SILENT_AFTER_SECS => Activity::Silent { secs },
        _ if session.bucket == Bucket::Working => {
            Activity::Working { detail: working_detail(session), age_secs: age }
        }
        _ => Activity::Idle,
    }
}

fn working_detail(session: &SessionListItem) -> Option<String> {
    session
        .activity_detail
        .clone()
        .or_else(|| session.last_tool_name.clone())
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty())
}

fn last_activity_age(app: &App, session: &SessionListItem) -> Option<i64> {
    let at = session.last_activity_at.or(session.last_tool_at)?;
    Some((app.clock_ms - at.timestamp_millis()).max(0) / 1_000)
}

#[cfg(test)]
mod tests {
    use cctui_proto::classifier::Bucket;
    use cctui_proto::models::{SessionEndReason, SessionStatus};
    use cctui_proto::session_end::EndTone;

    use super::{
        Activity, AttentionAction, Decision, EndBadge, PermissionInbox, SILENT_AFTER_SECS,
    };
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};
    use crate::testsupport::{permission_request, session};

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn request(session_id: &str, request_id: &str) -> crate::app::PendingPermission {
        let mut req = permission_request();
        req.session_id = session_id.to_owned();
        req.request_id = request_id.to_owned();
        req
    }

    fn attention(app: &mut App, action: AttentionAction) -> Vec<Effect> {
        reduce(app, Action::Attention(action))
    }

    #[test]
    fn a_request_is_queued_per_session_and_deduped() {
        let mut inbox = PermissionInbox::default();
        inbox.push(request("s-a", "r1"));
        inbox.push(request("s-a", "r1"));
        inbox.push(request("s-a", "r2"));
        inbox.push(request("s-b", "r3"));
        assert_eq!(inbox.len(), 3);
        assert_eq!(inbox.for_session("s-a").count(), 2);
        assert_eq!(inbox.head("s-a").map(|p| p.request_id.as_str()), Some("r1"));
        assert_eq!(inbox.sessions(), vec!["s-a", "s-b"]);
        assert!(inbox.has("s-b"));
        assert!(!inbox.has("s-c"));
    }

    #[test]
    fn jumping_cycles_through_the_waiting_sessions() {
        let mut inbox = PermissionInbox::default();
        inbox.push(request("s-a", "r1"));
        inbox.push(request("s-b", "r2"));
        assert_eq!(inbox.next_after(None).as_deref(), Some("s-a"));
        assert_eq!(inbox.next_after(Some("s-a")).as_deref(), Some("s-b"));
        assert_eq!(inbox.next_after(Some("s-b")).as_deref(), Some("s-a"));
        assert_eq!(inbox.next_after(Some("s-z")).as_deref(), Some("s-a"));
        assert!(PermissionInbox::default().next_after(None).is_none());
    }

    #[test]
    fn a_request_for_another_session_never_becomes_modal() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        attention(&mut app, AttentionAction::PermissionRequested(request("s-b", "r1")));
        assert_eq!(app.view(), crate::app::View::Conversation);
        assert!(!app.permissions.has("s-a"));

        // The key belongs to the session on screen, which has nothing pending.
        assert!(attention(&mut app, AttentionAction::Respond(Decision::Allow)).is_empty());
        assert!(app.permissions.has("s-b"), "the other session keeps its request");
    }

    #[test]
    fn answering_the_focused_card_responds_once() {
        let mut app = app();
        attention(&mut app, AttentionAction::PermissionRequested(request("s-a", "r1")));
        let effects = attention(&mut app, AttentionAction::Respond(Decision::Deny));
        match effects.as_slice() {
            [Effect::RespondPermission { session_id, request_id, behavior }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(request_id, "r1");
                assert_eq!(*behavior, "deny");
            }
            _ => panic!("expected one permission response"),
        }
        assert!(app.permissions.is_empty());
        assert!(attention(&mut app, AttentionAction::Respond(Decision::Deny)).is_empty());
    }

    #[test]
    fn allow_always_also_turns_on_auto_approve() {
        let mut app = app();
        attention(&mut app, AttentionAction::PermissionRequested(request("s-a", "r1")));
        let effects = attention(&mut app, AttentionAction::Respond(Decision::AllowAlways));
        match effects.as_slice() {
            [
                Effect::RespondPermission { behavior, .. },
                Effect::SetAutoApprove { session_id, enabled: true },
            ] => {
                assert_eq!(*behavior, "allow");
                assert_eq!(session_id, "s-a");
            }
            _ => panic!("expected an allow plus an auto-approve"),
        }
    }

    #[test]
    fn resolving_elsewhere_clears_the_card() {
        let mut app = app();
        attention(&mut app, AttentionAction::PermissionRequested(request("s-a", "r1")));
        attention(
            &mut app,
            AttentionAction::PermissionResolved {
                session_id: "s-a".to_owned(),
                request_id: "r1".to_owned(),
            },
        );
        assert!(app.permissions.is_empty());
    }

    #[test]
    fn the_servers_pending_list_replaces_what_we_had() {
        let mut app = app();
        attention(&mut app, AttentionAction::PermissionRequested(request("s-a", "stale")));
        attention(
            &mut app,
            AttentionAction::PendingPermissionsLoaded(vec![request("s-b", "fresh")]),
        );
        assert!(!app.permissions.has("s-a"));
        assert!(app.permissions.has("s-b"));
    }

    #[test]
    fn jumping_opens_the_waiting_sessions_conversation() {
        let mut app = app();
        attention(&mut app, AttentionAction::PermissionRequested(request("s-b", "r1")));
        let effects = attention(&mut app, AttentionAction::JumpToPending);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-b"));
        assert_eq!(app.view(), crate::app::View::Conversation);
        assert!(effects.iter().any(|e| matches!(e, Effect::Subscribe { .. })));
    }

    #[test]
    fn jumping_with_nothing_pending_only_says_so() {
        let mut app = app();
        assert!(attention(&mut app, AttentionAction::JumpToPending).is_empty());
        assert!(app.toasts.latest().is_some());
        assert_eq!(app.view(), crate::app::View::SessionList);
    }

    #[test]
    fn ending_a_session_records_the_reason_and_drops_its_cards() {
        let mut app = app();
        app.clock_ms = 1_700_000_000_000;
        attention(&mut app, AttentionAction::PermissionRequested(request("s-a", "r1")));
        attention(
            &mut app,
            AttentionAction::SessionEnded {
                session_id: "s-a".to_owned(),
                reason: SessionEndReason::SpawnFailed,
                detail: Some("unknown model gpt-nope\nmore".to_owned()),
            },
        );
        let s = &app.sessions[0];
        assert_eq!(s.end_reason, Some(SessionEndReason::SpawnFailed));
        assert_eq!(s.status, SessionStatus::Inactive);
        assert!(s.ended_at.is_some());
        assert!(!app.permissions.has("s-a"));
        assert!(app.toasts.latest().expect("a toast").text.contains("unknown model gpt-nope"));
    }

    #[test]
    fn a_reaped_session_ends_without_a_toast() {
        let mut app = app();
        attention(
            &mut app,
            AttentionAction::SessionEnded {
                session_id: "s-a".to_owned(),
                reason: SessionEndReason::ReapedInactive,
                detail: None,
            },
        );
        assert!(app.toasts.latest().is_none());
        assert_eq!(app.sessions[0].end_reason, Some(SessionEndReason::ReapedInactive));
    }

    #[test]
    fn every_reason_has_a_label_and_the_shared_tone() {
        let badge = EndBadge::new(SessionEndReason::Crashed, Some("segfault"));
        assert_eq!(badge.label, "crashed");
        assert_eq!(badge.tone, EndTone::Danger);
        assert!(!badge.muted);
        assert_eq!(badge.detail, None, "only a failed start carries its detail");

        assert_eq!(EndBadge::new(SessionEndReason::Completed, None).tone, EndTone::Ok);
        assert_eq!(EndBadge::new(SessionEndReason::DaemonLost, None).tone, EndTone::Warn);
        assert!(EndBadge::new(SessionEndReason::ReapedInactive, None).muted);
        assert_eq!(
            EndBadge::new(SessionEndReason::ResumeFailed, Some("auth")).detail.as_deref(),
            Some("auth")
        );
    }

    #[test]
    fn the_banner_reports_the_end_state_over_everything_else() {
        let mut app = app();
        app.sessions[0].end_reason = Some(SessionEndReason::Killed);
        app.sessions[0].bucket = Bucket::Working;
        match super::activity(&app, &app.sessions[0]) {
            Activity::Ended(badge) => assert_eq!(badge.label, "killed"),
            other => panic!("expected an end badge, got {other:?}"),
        }
    }

    #[test]
    fn a_pending_card_outranks_the_working_bucket() {
        let mut app = app();
        attention(&mut app, AttentionAction::PermissionRequested(request("s-a", "r1")));
        assert_eq!(super::activity(&app, &app.sessions[0]), Activity::NeedsInput);
    }

    #[test]
    fn a_blocked_session_is_waiting_on_you() {
        let mut app = app();
        app.sessions[0].bucket = Bucket::Blocked;
        assert_eq!(super::activity(&app, &app.sessions[0]), Activity::NeedsInput);
    }

    #[test]
    fn working_carries_the_activity_detail_and_its_age() {
        let mut app = app();
        app.clock_ms = 60_000;
        app.sessions[0].activity_detail = Some("  reading main.rs  ".to_owned());
        app.sessions[0].last_activity_at = chrono::DateTime::from_timestamp_millis(48_000);
        assert_eq!(
            super::activity(&app, &app.sessions[0]),
            Activity::Working { detail: Some("reading main.rs".to_owned()), age_secs: Some(12) }
        );
    }

    #[test]
    fn a_long_quiet_working_session_reads_as_silent() {
        let mut app = app();
        app.clock_ms = 1_000_000;
        app.sessions[0].last_activity_at =
            chrono::DateTime::from_timestamp_millis(1_000_000 - (SILENT_AFTER_SECS + 5) * 1_000);
        assert_eq!(
            super::activity(&app, &app.sessions[0]),
            Activity::Silent { secs: SILENT_AFTER_SECS + 5 }
        );
    }

    #[test]
    fn an_inactive_session_with_no_reason_is_merely_idle() {
        let mut app = app();
        app.sessions[0].status = SessionStatus::Inactive;
        assert_eq!(super::activity(&app, &app.sessions[0]), Activity::Idle);
    }
}
