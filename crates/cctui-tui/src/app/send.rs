//! Tracked outbound sends: ack correlation, bounded auto-retry, and the
//! delivery state a user line carries until the agent has it.

use crossterm::event::KeyEvent;
use uuid::Uuid;

use super::action::{Action, Effect};
use super::state::{App, ConversationLine, LineKind, LineStatus};
use super::toast::Level;

/// How long a dispatched frame may go unacked before it counts as lost.
pub const ACK_TIMEOUT_MS: i64 = 8_000;
/// An adapter that never reports a delivery result leaves the send
/// unconfirmed rather than failed: a slow agent must not invite a resend.
pub const DELIVERY_TIMEOUT_MS: i64 = 20_000;
pub const MAX_ATTEMPTS: u32 = 5;
/// A send parked on a missing socket is waiting for a transport, not backing
/// off a server, so it spends no retry budget.
pub const RECONNECT_PARK_MS: i64 = 1_000;
/// How long a delivered send keeps its mark before the server's own echo is
/// the only copy of it.
pub const DELIVERED_LINGER_MS: i64 = 1_500;

const BACKOFF_BASE_MS: i64 = 1_000;
const BACKOFF_CAP_MS: i64 = 30_000;

/// Exponential, capped, and deliberately without jitter: the reducer is pure
/// and its retry schedule has to be assertable.
#[must_use]
pub fn backoff_ms(attempt: u32) -> i64 {
    let shift = attempt.saturating_sub(1).min(20);
    BACKOFF_BASE_MS.saturating_mul(1_i64 << shift).min(BACKOFF_CAP_MS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Dispatched, waiting for `message_ack`.
    Pending,
    /// Acked as queued toward a daemon, waiting for `command_result`.
    AwaitingDelivery,
    /// Waiting out a backoff or a reconnect before the next attempt.
    Backoff,
    Delivered,
    /// Out of attempts: red, with the manual retry/edit/drop affordance.
    Failed,
}

/// One message the composer let go of, with its retry lifecycle.
pub struct TrackedSend {
    pub id: u64,
    pub session_id: String,
    pub content: String,
    /// Stable across retries, so a retried frame the server did receive
    /// produces the same turn as the first attempt.
    pub turn_id: Option<Uuid>,
    /// Correlation id of the current attempt; rotates on every dispatch.
    pub client_msg_id: Option<String>,
    pub command_id: Option<Uuid>,
    pub attempt: u32,
    pub phase: Phase,
    pub reason: Option<String>,
    /// When the current phase runs out, in reducer-clock milliseconds.
    pub deadline_ms: i64,
}

impl TrackedSend {
    fn status(&self) -> LineStatus {
        match self.phase {
            Phase::Pending if self.attempt > 1 => {
                LineStatus::Retrying { attempt: self.attempt, max: MAX_ATTEMPTS }
            }
            Phase::Pending | Phase::AwaitingDelivery => LineStatus::Sending,
            Phase::Backoff => LineStatus::Retrying {
                attempt: self.attempt.saturating_add(1).min(MAX_ATTEMPTS),
                max: MAX_ATTEMPTS,
            },
            Phase::Delivered => LineStatus::Delivered,
            Phase::Failed => {
                let reason = self.reason.clone();
                LineStatus::Failed(reason.unwrap_or_else(|| "not delivered".to_owned()))
            }
        }
    }

    fn line(&self) -> ConversationLine {
        ConversationLine::new(LineKind::User, self.content.clone(), 0).with_status(self.status())
    }
}

/// Everything in flight, newest last. Small by construction: a send leaves as
/// soon as it is delivered.
#[derive(Default)]
pub struct Outbox {
    sends: Vec<TrackedSend>,
    next_id: u64,
    /// Acks that beat their own dispatch report home.
    orphans: Vec<Orphan>,
}

struct Orphan {
    client_msg_id: String,
    ok: bool,
    error: Option<String>,
    command_id: Option<Uuid>,
}

const MAX_ORPHANS: usize = 8;

impl Outbox {
    #[must_use]
    pub fn tracked(&self) -> impl Iterator<Item = &TrackedSend> {
        self.sends.iter()
    }

    fn find_mut(&mut self, send_id: u64) -> Option<&mut TrackedSend> {
        self.sends.iter_mut().find(|s| s.id == send_id)
    }

    fn by_client_msg_id(&mut self, client_msg_id: &str) -> Option<&mut TrackedSend> {
        self.sends.iter_mut().find(|s| s.client_msg_id.as_deref() == Some(client_msg_id))
    }

    fn by_command_id(&mut self, command_id: Uuid) -> Option<&mut TrackedSend> {
        self.sends.iter_mut().find(|s| s.command_id == Some(command_id))
    }

    /// The failed send the manual affordances act on: the newest one in this
    /// session, since the transcript has no per-line cursor.
    #[must_use]
    pub fn failed_id(&self, session_id: &str) -> Option<u64> {
        self.sends
            .iter()
            .rev()
            .find(|s| s.session_id == session_id && s.phase == Phase::Failed)
            .map(|s| s.id)
    }

    fn take(&mut self, send_id: u64) -> Option<TrackedSend> {
        let at = self.sends.iter().position(|s| s.id == send_id)?;
        Some(self.sends.remove(at))
    }
}

fn dispatch(send: &mut TrackedSend, now: i64) -> Effect {
    send.attempt = send.attempt.saturating_add(1);
    send.phase = Phase::Pending;
    send.reason = None;
    send.client_msg_id = None;
    send.command_id = None;
    send.deadline_ms = now + ACK_TIMEOUT_MS;
    Effect::SendMessage {
        send_id: send.id,
        session_id: send.session_id.clone(),
        content: send.content.clone(),
        turn_id: send.turn_id,
    }
}

fn attempt_failed(send: &mut TrackedSend, reason: String, now: i64) {
    send.command_id = None;
    send.reason = Some(reason);
    if send.attempt >= MAX_ATTEMPTS {
        send.phase = Phase::Failed;
        return;
    }
    send.phase = Phase::Backoff;
    send.deadline_ms = now + backoff_ms(send.attempt);
}

fn park_for_reconnect(send: &mut TrackedSend, now: i64) {
    send.attempt = send.attempt.saturating_sub(1);
    send.phase = Phase::Backoff;
    send.reason = Some("not connected — reconnecting".to_owned());
    send.deadline_ms = now + RECONNECT_PARK_MS;
}

fn delivered(send: &mut TrackedSend, now: i64) {
    send.phase = Phase::Delivered;
    send.reason = None;
    send.deadline_ms = now + DELIVERED_LINGER_MS;
}

/// Hands one message to the outbox and dispatches its first attempt.
pub fn submit(app: &mut App, session_id: String, content: String) -> Vec<Effect> {
    app.outbox.next_id += 1;
    let mut send = TrackedSend {
        id: app.outbox.next_id,
        session_id,
        content,
        turn_id: None,
        client_msg_id: None,
        command_id: None,
        attempt: 0,
        phase: Phase::Pending,
        reason: None,
        deadline_ms: 0,
    };
    let effect = dispatch(&mut send, app.clock_ms);
    app.outbox.sends.push(send);
    vec![effect]
}

/// The optimistic lines a session's transcript shows below its history.
///
/// A delivered send whose own echo already landed is dropped here rather than
/// rendered twice.
#[must_use]
pub fn pending_lines(app: &App, session_id: &str) -> Vec<ConversationLine> {
    app.outbox
        .tracked()
        .filter(|s| s.session_id == session_id)
        .filter(|s| s.phase != Phase::Delivered || !echoed(app, session_id, &s.content))
        .map(TrackedSend::line)
        .collect()
}

/// Whether the session's own transcript already ends with this user message.
fn echoed(app: &App, session_id: &str, content: &str) -> bool {
    let Some(store) = app.conversations.get(session_id) else { return false };
    store
        .entries()
        .iter()
        .rev()
        .take(10)
        .any(|e| e.line.kind == LineKind::User && e.line.text.trim() == content.trim())
}

/// Advances every deadline the clock has passed.
pub fn tick(app: &mut App) -> Vec<Effect> {
    let now = app.clock_ms;
    let mut effects = Vec::new();
    let mut done = Vec::new();
    for send in &mut app.outbox.sends {
        if now < send.deadline_ms {
            continue;
        }
        match send.phase {
            Phase::Pending => {
                attempt_failed(send, "no response from server".to_owned(), now);
            }
            // An adapter that reports nothing is not evidence of a failure.
            Phase::AwaitingDelivery | Phase::Delivered => done.push(send.id),
            Phase::Backoff => effects.push(dispatch(send, now)),
            Phase::Failed => {}
        }
    }
    app.outbox.sends.retain(|s| !done.contains(&s.id));
    effects
}

/// A fresh socket re-attempts everything parked on the old one at once.
pub fn redispatch_parked(app: &mut App) -> Vec<Effect> {
    let now = app.clock_ms;
    app.outbox
        .sends
        .iter_mut()
        .filter(|s| s.phase == Phase::Backoff)
        .map(|send| dispatch(send, now))
        .collect()
}

/// Seeds a tracked send in a given phase, for the snapshot cases.
#[cfg(test)]
pub fn seed(app: &mut App, session_id: &str, content: &str, phase: Phase, reason: Option<&str>) {
    app.outbox.next_id += 1;
    app.outbox.sends.push(TrackedSend {
        id: app.outbox.next_id,
        session_id: session_id.to_owned(),
        content: content.to_owned(),
        turn_id: None,
        client_msg_id: None,
        command_id: None,
        attempt: 1,
        phase,
        reason: reason.map(str::to_owned),
        deadline_ms: i64::MAX,
    });
}

pub enum SendAction {
    Dispatched { send_id: u64, client_msg_id: String, turn_id: Uuid },
    DispatchFailed { send_id: u64, reason: String },
    Acked { client_msg_id: String, ok: bool, error: Option<String>, command_id: Option<Uuid> },
    DeliveryResult { command_id: Uuid, ok: bool, error: Option<String> },
    /// The key that asked for it, so a session with nothing failed still opens
    /// the composer on that character instead of swallowing it.
    Retry(KeyEvent),
    Discard(KeyEvent),
    Edit(KeyEvent),
}

pub fn reduce_send(app: &mut App, action: SendAction) -> Vec<Effect> {
    let now = app.clock_ms;
    match action {
        SendAction::Dispatched { send_id, client_msg_id, turn_id } => {
            let Some(send) = app.outbox.find_mut(send_id) else { return Vec::new() };
            send.client_msg_id = Some(client_msg_id.clone());
            send.turn_id = Some(turn_id);
            let at = app.outbox.orphans.iter().position(|o| o.client_msg_id == client_msg_id);
            let Some(at) = at else { return Vec::new() };
            let orphan = app.outbox.orphans.remove(at);
            apply_ack(app, &orphan, now);
            Vec::new()
        }
        SendAction::DispatchFailed { send_id, reason } => {
            tracing::warn!(%reason, "message dispatch failed");
            if let Some(send) = app.outbox.find_mut(send_id) {
                park_for_reconnect(send, now);
            }
            Vec::new()
        }
        SendAction::Acked { client_msg_id, ok, error, command_id } => {
            let ack = Orphan { client_msg_id, ok, error, command_id };
            if app.outbox.by_client_msg_id(&ack.client_msg_id).is_some() {
                apply_ack(app, &ack, now);
            } else {
                if app.outbox.orphans.len() >= MAX_ORPHANS {
                    app.outbox.orphans.remove(0);
                }
                app.outbox.orphans.push(ack);
            }
            Vec::new()
        }
        SendAction::DeliveryResult { command_id, ok, error } => {
            let Some(send) = app.outbox.by_command_id(command_id) else { return Vec::new() };
            if ok {
                delivered(send, now);
            } else {
                let reason =
                    error.unwrap_or_else(|| "the agent did not accept the message".to_owned());
                attempt_failed(send, reason, now);
            }
            Vec::new()
        }
        SendAction::Retry(key) => retry(app, key),
        SendAction::Discard(key) => discard(app, key),
        SendAction::Edit(key) => edit(app, key),
    }
}

fn apply_ack(app: &mut App, ack: &Orphan, now: i64) {
    let Some(send) = app.outbox.by_client_msg_id(&ack.client_msg_id) else { return };
    if !ack.ok {
        let reason =
            ack.error.clone().unwrap_or_else(|| "could not deliver to the agent".to_owned());
        attempt_failed(send, reason, now);
        return;
    }
    let Some(command_id) = ack.command_id else {
        delivered(send, now);
        return;
    };
    send.command_id = Some(command_id);
    send.phase = Phase::AwaitingDelivery;
    send.reason = None;
    send.deadline_ms = now + DELIVERY_TIMEOUT_MS;
}

/// The failed send of the selected session, or `None` when the key should just
/// reach the composer.
fn failed_send(app: &App) -> Option<u64> {
    let session_id = app.selected_session_id()?;
    app.outbox.failed_id(&session_id)
}

fn retry(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    let Some(send_id) = failed_send(app) else { return fall_through(app, key) };
    let now = app.clock_ms;
    let Some(send) = app.outbox.find_mut(send_id) else { return Vec::new() };
    send.attempt = 0;
    vec![dispatch(send, now)]
}

fn discard(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    let Some(send_id) = failed_send(app) else { return fall_through(app, key) };
    app.outbox.take(send_id);
    app.toast(Level::Info, "dropped the undelivered message");
    Vec::new()
}

fn edit(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    let Some(send_id) = failed_send(app) else { return fall_through(app, key) };
    let Some(send) = app.outbox.take(send_id) else { return Vec::new() };
    app.reset_input();
    app.message_input.insert_str(&send.content);
    app.input_active = true;
    Vec::new()
}

fn fall_through(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    super::reduce(app, Action::ActivateInputWith(key))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use uuid::Uuid;

    use super::{
        ACK_TIMEOUT_MS, DELIVERED_LINGER_MS, DELIVERY_TIMEOUT_MS, MAX_ATTEMPTS, Phase, SendAction,
        backoff_ms, pending_lines, redispatch_parked, tick,
    };
    use crate::app::state::{App, LineStatus, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app.router.push(View::Conversation);
        app
    }

    fn send_one(app: &mut App) -> u64 {
        app.message_input.insert_str("ship it");
        let effects = reduce(app, Action::SubmitInput);
        match effects.as_slice() {
            [crate::app::action::Effect::SendMessage { send_id, content, .. }] => {
                assert_eq!(content, "ship it");
                *send_id
            }
            _ => panic!("expected a send effect"),
        }
    }

    fn dispatched(app: &mut App, send_id: u64, client_msg_id: &str) {
        reduce(
            app,
            Action::Send(SendAction::Dispatched {
                send_id,
                client_msg_id: client_msg_id.to_owned(),
                turn_id: Uuid::nil(),
            }),
        );
    }

    fn ack(app: &mut App, client_msg_id: &str, ok: bool, command_id: Option<Uuid>) {
        reduce(
            app,
            Action::Send(SendAction::Acked {
                client_msg_id: client_msg_id.to_owned(),
                ok,
                error: (!ok).then(|| "no daemon connected".to_owned()),
                command_id,
            }),
        );
    }

    fn status(app: &App) -> Option<LineStatus> {
        pending_lines(app, "s-a").into_iter().next().and_then(|l| l.status)
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn a_submitted_message_shows_as_sending_until_it_is_acked() {
        let mut app = app();
        let id = send_one(&mut app);
        assert_eq!(status(&app), Some(LineStatus::Sending));
        dispatched(&mut app, id, "cid-1");
        ack(&mut app, "cid-1", true, None);
        assert_eq!(status(&app), Some(LineStatus::Delivered));

        app.clock_ms += DELIVERED_LINGER_MS;
        tick(&mut app);
        assert!(app.outbox.tracked().next().is_none(), "a delivered send stops being tracked");
    }

    #[test]
    fn an_ok_ack_with_a_command_waits_for_the_adapter_to_confirm() {
        let mut app = app();
        let id = send_one(&mut app);
        let command_id = Uuid::from_u128(7);
        dispatched(&mut app, id, "cid-1");
        ack(&mut app, "cid-1", true, Some(command_id));
        assert_eq!(status(&app), Some(LineStatus::Sending));

        reduce(
            &mut app,
            Action::Send(SendAction::DeliveryResult { command_id, ok: true, error: None }),
        );
        assert_eq!(status(&app), Some(LineStatus::Delivered));
    }

    #[test]
    fn an_adapter_that_never_answers_leaves_the_send_unconfirmed_not_failed() {
        let mut app = app();
        let id = send_one(&mut app);
        let command_id = Uuid::from_u128(7);
        dispatched(&mut app, id, "cid-1");
        ack(&mut app, "cid-1", true, Some(command_id));

        app.clock_ms += DELIVERY_TIMEOUT_MS;
        assert!(tick(&mut app).is_empty());
        assert!(app.outbox.tracked().next().is_none());
    }

    #[test]
    fn a_rejected_send_retries_with_backoff_and_then_goes_red() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        ack(&mut app, "cid-1", false, None);
        assert_eq!(status(&app), Some(LineStatus::Retrying { attempt: 2, max: MAX_ATTEMPTS }));

        for attempt in 1..MAX_ATTEMPTS {
            app.clock_ms += backoff_ms(attempt);
            let effects = tick(&mut app);
            assert_eq!(effects.len(), 1, "attempt {attempt} must be re-dispatched");
            let cid = format!("cid-{}", attempt + 1);
            dispatched(&mut app, id, &cid);
            ack(&mut app, &cid, false, None);
        }

        match status(&app) {
            Some(LineStatus::Failed(reason)) => assert_eq!(reason, "no daemon connected"),
            other => panic!("expected the server error, got {other:?}"),
        }
        app.clock_ms += 60_000;
        assert!(tick(&mut app).is_empty(), "a red send never retries on its own");
    }

    #[test]
    fn an_unacked_frame_is_retried_once_its_ack_window_closes() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        app.clock_ms += ACK_TIMEOUT_MS;
        assert!(tick(&mut app).is_empty(), "the failed attempt backs off first");
        assert_eq!(status(&app), Some(LineStatus::Retrying { attempt: 2, max: MAX_ATTEMPTS }));

        app.clock_ms += backoff_ms(1);
        assert_eq!(tick(&mut app).len(), 1);
    }

    #[test]
    fn a_send_parked_on_a_dead_socket_keeps_its_retry_budget() {
        let mut app = app();
        let id = send_one(&mut app);
        reduce(
            &mut app,
            Action::Send(SendAction::DispatchFailed {
                send_id: id,
                reason: "disconnected".to_owned(),
            }),
        );
        assert_eq!(app.outbox.tracked().next().expect("the send").attempt, 0);

        let effects = reduce(&mut app, Action::Reconnected);
        assert!(
            effects.iter().any(|e| matches!(e, crate::app::action::Effect::SendMessage { .. })),
            "a fresh socket re-dispatches what was parked"
        );
        assert_eq!(app.outbox.tracked().next().expect("the send").attempt, 1);
    }

    #[test]
    fn an_ack_that_arrives_before_its_dispatch_report_still_lands() {
        let mut app = app();
        let id = send_one(&mut app);
        ack(&mut app, "cid-1", true, None);
        assert_eq!(status(&app), Some(LineStatus::Sending));
        dispatched(&mut app, id, "cid-1");
        assert_eq!(status(&app), Some(LineStatus::Delivered));
    }

    #[test]
    fn a_stale_ack_for_a_superseded_attempt_is_ignored() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        ack(&mut app, "cid-1", false, None);
        app.clock_ms += backoff_ms(1);
        tick(&mut app);
        dispatched(&mut app, id, "cid-2");

        ack(&mut app, "cid-1", true, None);
        assert_eq!(status(&app), Some(LineStatus::Sending), "the old id resolves nothing");
    }

    #[test]
    fn a_failed_send_can_be_retried_dropped_or_edited() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        app.outbox.find_mut(id).expect("the send").phase = Phase::Failed;

        let effects = reduce(&mut app, Action::Send(SendAction::Retry(key('R'))));
        assert_eq!(effects.len(), 1);
        assert_eq!(status(&app), Some(LineStatus::Sending));

        app.outbox.find_mut(id).expect("the send").phase = Phase::Failed;
        reduce(&mut app, Action::Send(SendAction::Edit(key('e'))));
        assert_eq!(app.message_input.lines().join("\n"), "ship it");
        assert!(app.input_active);
        assert!(app.outbox.tracked().next().is_none(), "an edited send is no longer tracked");
    }

    #[test]
    fn dropping_a_failed_send_forgets_it() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        app.outbox.find_mut(id).expect("the send").phase = Phase::Failed;
        reduce(&mut app, Action::Send(SendAction::Discard(key('x'))));
        assert!(app.outbox.tracked().next().is_none());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn with_nothing_failed_those_keys_still_type_into_the_composer() {
        let mut app = app();
        reduce(&mut app, Action::Send(SendAction::Discard(key('x'))));
        assert!(app.input_active);
        assert_eq!(app.message_input.lines().join("\n"), "x");
    }

    #[test]
    fn a_delivered_send_is_not_drawn_twice_once_its_echo_lands() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        ack(&mut app, "cid-1", true, None);
        reduce(
            &mut app,
            Action::StreamLine {
                session_id: "s-a".to_owned(),
                seq: Some(1),
                line: crate::app::state::ConversationLine::new(
                    crate::app::LineKind::User,
                    "ship it",
                    0,
                ),
                usage: None,
            },
        );
        assert!(pending_lines(&app, "s-a").is_empty());
    }

    #[test]
    fn the_backoff_ladder_is_exponential_and_capped() {
        assert_eq!(backoff_ms(1), 1_000);
        assert_eq!(backoff_ms(2), 2_000);
        assert_eq!(backoff_ms(5), 16_000);
        assert_eq!(backoff_ms(99), 30_000);
    }

    #[test]
    fn sends_of_another_session_are_not_shown_here() {
        let mut app = app();
        let _ = send_one(&mut app);
        assert!(pending_lines(&app, "s-other").is_empty());
        assert_eq!(app.outbox.tracked().count(), 1);
    }

    #[test]
    fn redispatching_nothing_is_free() {
        let mut app = app();
        assert!(redispatch_parked(&mut app).is_empty());
    }
}
