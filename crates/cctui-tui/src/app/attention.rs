//! What a session wants from you: pending permission cards and the
//! needs-you indicator.

use super::action::Effect;
use super::conversation;
use super::state::{App, PendingPermission};
use super::toast::Level;

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
    PermissionResolved { session_id: String, request_id: String },
    /// The server's own pending list, fetched on start and after a reconnect.
    PendingPermissionsLoaded(Vec<PendingPermission>),
    /// Answer the card the focused session is showing.
    Respond(Decision),
    JumpToPending,
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

#[cfg(test)]
mod tests {
    use super::{AttentionAction, Decision, PermissionInbox};
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
}
