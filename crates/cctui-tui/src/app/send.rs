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

/// How long a frame may wait for the effect worker before the send gives up on
/// ever being dispatched. Longer than the ordered lane can block (one request
/// bound each), so a slow lane does not retry; without it a send whose effect
/// never ran would sit at "Sending" with no deadline at all.
pub const DISPATCH_TIMEOUT_MS: i64 = 120_000;
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
    /// Carried on every attempt so a retried answer still drives the agent's
    /// own form rather than dismissing it.
    pub ask_picks: Option<Vec<Vec<usize>>>,
    /// Whether the current attempt's frame has reached the socket. Cleared by
    /// every dispatch and set by its `Dispatched` report.
    pub on_the_wire: bool,
    /// Correlation id of this message, minted on the first dispatch and kept
    /// for every retry: the server dedupes on it, and a late ack for an
    /// earlier attempt still resolves the send instead of being orphaned.
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
    /// Delivery results that beat the ack carrying their own `command_id`.
    early_results: Vec<EarlyResult>,
}

struct EarlyResult {
    command_id: Uuid,
    ok: bool,
    error: Option<String>,
}

struct Orphan {
    client_msg_id: String,
    ok: bool,
    error: Option<String>,
    command_id: Option<Uuid>,
}

const MAX_ORPHANS: usize = 8;
const MAX_EARLY_RESULTS: usize = 8;

impl Outbox {
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

/// Emits one attempt. The ack clock does not start here: the effect worker is
/// serial, so the frame may be written seconds after this returns. `Dispatched`
/// is what the frame actually reaching the socket looks like, and that is where
/// the ack deadline is set; until then the deadline only has to outlast the lane.
fn dispatch(send: &mut TrackedSend, now: i64) -> Effect {
    send.attempt = send.attempt.saturating_add(1);
    send.phase = Phase::Pending;
    send.reason = None;
    send.command_id = None;
    send.on_the_wire = false;
    send.deadline_ms = now.saturating_add(DISPATCH_TIMEOUT_MS);
    Effect::SendMessage {
        send_id: send.id,
        session_id: send.session_id.clone(),
        content: send.content.clone(),
        ask_picks: send.ask_picks.clone(),
        turn_id: send.turn_id,
        client_msg_id: send.client_msg_id.clone(),
    }
}

impl TrackedSend {
    /// Whether the server certainly never saw this message, so quitting would
    /// lose it outright. An acked send is excluded: it is queued toward a daemon,
    /// and showing its text back to the user would look like it never ran.
    #[must_use]
    pub const fn never_reached_the_server(&self) -> bool {
        match self.phase {
            // Dispatched but unacked only counts when this attempt's frame never
            // went out. `client_msg_id` cannot say so: it is minted once and
            // replayed by every retry, so it is set while a retry is in flight.
            Phase::Pending => !self.on_the_wire,
            Phase::Backoff | Phase::Failed => true,
            Phase::AwaitingDelivery | Phase::Delivered => false,
        }
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
pub fn submit(
    app: &mut App,
    session_id: String,
    content: String,
    ask_picks: Option<Vec<Vec<usize>>>,
) -> Vec<Effect> {
    // Every send funnels through here — typed, ask answer, plan refine — so the
    // composer's draft and history are NOT touched here: only the composer path
    // spends a draft (`SubmitInput`), or a card answer would delete it.
    let mut effects = Vec::new();
    app.outbox.next_id += 1;
    let mut send = TrackedSend {
        id: app.outbox.next_id,
        session_id,
        content,
        turn_id: None,
        ask_picks,
        on_the_wire: false,
        client_msg_id: None,
        command_id: None,
        attempt: 0,
        phase: Phase::Pending,
        reason: None,
        deadline_ms: 0,
    };
    let effect = dispatch(&mut send, app.clock_ms);
    app.outbox.sends.push(send);
    effects.push(effect);
    effects
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

/// A session that ended will never deliver what is still in flight for it:
/// those sends fail now rather than spending four more attempts on a dead
/// agent. A delivered one keeps its mark.
pub fn session_ended(app: &mut App, session_id: &str) {
    for send in &mut app.outbox.sends {
        if send.session_id != session_id {
            continue;
        }
        if matches!(send.phase, Phase::Pending | Phase::AwaitingDelivery | Phase::Backoff) {
            send.attempt = MAX_ATTEMPTS;
            attempt_failed(send, "session ended".to_owned(), app.clock_ms);
        }
    }
}

/// A deregistered session has no transcript left to carry a failed line, so its
/// sends leave with it.
pub fn session_deregistered(app: &mut App, session_id: &str) {
    app.outbox.sends.retain(|s| s.session_id != session_id);
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
        ask_picks: None,
        on_the_wire: true,
        client_msg_id: None,
        command_id: None,
        attempt: 1,
        phase,
        reason: reason.map(str::to_owned),
        deadline_ms: i64::MAX,
    });
}

pub enum SendAction {
    Dispatched {
        send_id: u64,
        client_msg_id: String,
        turn_id: Uuid,
    },
    DispatchFailed {
        send_id: u64,
        reason: String,
    },
    Acked {
        client_msg_id: String,
        ok: bool,
        error: Option<String>,
        command_id: Option<Uuid>,
    },
    DeliveryResult {
        command_id: Uuid,
        ok: bool,
        error: Option<String>,
    },
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
            send.on_the_wire = true;
            if send.phase == Phase::Pending {
                send.deadline_ms = now + ACK_TIMEOUT_MS;
            }
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
            if !apply_delivery(app, command_id, ok, error.clone(), now) {
                if app.outbox.early_results.len() >= MAX_EARLY_RESULTS {
                    app.outbox.early_results.remove(0);
                }
                app.outbox.early_results.push(EarlyResult { command_id, ok, error });
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

    let at = app.outbox.early_results.iter().position(|r| r.command_id == command_id);
    if let Some(at) = at {
        let early = app.outbox.early_results.remove(at);
        apply_delivery(app, command_id, early.ok, early.error, now);
    }
}

/// A delivery result can beat the ack that names its `command_id`. Buffering it
/// is what keeps a rejection from reading as a success once the 20s
/// unconfirmed window lapses.
fn apply_delivery(
    app: &mut App,
    command_id: Uuid,
    ok: bool,
    error: Option<String>,
    now: i64,
) -> bool {
    let Some(send) = app.outbox.by_command_id(command_id) else { return false };
    if ok {
        delivered(send, now);
    } else {
        let reason = error.unwrap_or_else(|| "the agent did not accept the message".to_owned());
        attempt_failed(send, reason, now);
    }
    true
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
    app.set_input_text(&send.content);
    app.input_active = true;
    super::drafts::on_input(app)
}

fn fall_through(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    super::reduce(app, Action::ActivateInputWith(key))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use uuid::Uuid;

    use super::{
        ACK_TIMEOUT_MS, DELIVERED_LINGER_MS, DELIVERY_TIMEOUT_MS, DISPATCH_TIMEOUT_MS,
        MAX_ATTEMPTS, Phase, SendAction, TrackedSend, backoff_ms, pending_lines, redispatch_parked,
        tick,
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

    /// The dispatch a send produced. A send also clears the composer's draft
    /// and records the prompt, so it is not the only effect of one.
    fn sent(effects: &[crate::app::action::Effect]) -> (u64, &str) {
        effects
            .iter()
            .find_map(|e| match e {
                crate::app::action::Effect::SendMessage { send_id, content, .. } => {
                    Some((*send_id, content.as_str()))
                }
                _ => None,
            })
            .expect("expected a send effect")
    }

    fn send_one(app: &mut App) -> u64 {
        app.message_input.insert_str("ship it");
        let effects = reduce(app, Action::SubmitInput);
        let (send_id, content) = sent(&effects);
        assert_eq!(content, "ship it");
        send_id
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
        assert_eq!(
            status(&app),
            Some(LineStatus::Retrying { attempt: 2, max: MAX_ATTEMPTS }),
            "the superseded id must not deliver the attempt now in flight"
        );
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
                line: Some(Box::new(crate::app::state::ConversationLine::new(
                    crate::app::LineKind::User,
                    "ship it",
                    0,
                ))),
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

    /// F20: a retry must reuse the correlation id, so the server can recognise
    /// the resend and a late ack for the first attempt still lands.
    #[test]
    fn a_retry_replays_the_same_client_msg_id() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");

        app.clock_ms += ACK_TIMEOUT_MS;
        tick(&mut app);
        app.clock_ms += backoff_ms(1);
        let effects = tick(&mut app);

        let replayed = effects
            .iter()
            .find_map(|e| match e {
                crate::app::action::Effect::SendMessage { client_msg_id, .. } => {
                    Some(client_msg_id.clone())
                }
                _ => None,
            })
            .expect("a redispatch");
        assert_eq!(replayed, Some("cid-1".to_owned()), "the retry must not mint a new id");

        // The ack the first attempt never got still resolves the send.
        ack(&mut app, "cid-1", true, None);
        assert_eq!(status(&app), Some(LineStatus::Delivered));
    }

    /// F26: the effect worker is serial, so the ack clock must not run while
    /// the frame is still queued behind slow HTTP effects.
    #[test]
    fn the_ack_clock_starts_when_the_frame_reaches_the_socket() {
        let mut app = app();
        let id = send_one(&mut app);

        // Far past the ack timeout, but nothing has been dispatched yet.
        app.clock_ms += ACK_TIMEOUT_MS * 4;
        let effects = tick(&mut app);
        assert!(effects.is_empty(), "a queued send must not time out before it is sent");
        assert_eq!(status(&app), Some(LineStatus::Sending));

        dispatched(&mut app, id, "cid-1");
        app.clock_ms += ACK_TIMEOUT_MS - 1;
        assert!(tick(&mut app).is_empty(), "the clock runs from the dispatch report");
        app.clock_ms += 1;
        tick(&mut app);
        assert!(matches!(status(&app), Some(LineStatus::Retrying { .. })));
    }

    /// F28: a `command_result` can beat the ack that names its `command_id`.
    #[test]
    fn a_delivery_result_that_beats_its_ack_is_still_applied() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        let command = Uuid::from_u128(7);

        reduce(
            &mut app,
            Action::Send(SendAction::DeliveryResult {
                command_id: command,
                ok: false,
                error: Some("adapter refused it".to_owned()),
            }),
        );
        ack(&mut app, "cid-1", true, Some(command));

        assert_eq!(
            status(&app),
            Some(LineStatus::Retrying { attempt: 2, max: MAX_ATTEMPTS }),
            "an early rejection must not read as delivered"
        );
    }

    /// F28: without this the send spends four more attempts on a dead agent.
    #[test]
    fn a_send_fails_when_its_session_ends_and_leaves_when_it_is_deregistered() {
        use cctui_proto::models::SessionEndReason;

        use crate::app::attention::AttentionAction;

        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");

        reduce(
            &mut app,
            Action::Attention(AttentionAction::SessionEnded {
                session_id: "s-a".to_owned(),
                reason: SessionEndReason::Completed,
                detail: None,
            }),
        );
        assert!(
            matches!(status(&app), Some(LineStatus::Failed(_))),
            "an ended session cannot deliver what is in flight"
        );

        reduce(&mut app, Action::SessionDeregistered("s-a".to_owned()));
        assert!(pending_lines(&app, "s-a").is_empty(), "a deregistered session takes its sends");
    }

    /// R1: the server refuses a message whose daemon is offline. That ack must
    /// never read as delivered — the line stays undelivered so the retry runs
    /// and the quit flush keeps the text.
    #[test]
    fn a_refused_send_is_not_delivered_and_survives_a_quit() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");

        reduce(
            &mut app,
            Action::Send(SendAction::Acked {
                client_msg_id: "cid-1".to_owned(),
                ok: false,
                error: Some("no daemon connected".to_owned()),
                command_id: None,
            }),
        );

        assert_ne!(status(&app), Some(LineStatus::Delivered), "a refusal is not a delivery");
        assert!(
            app.outbox.tracked().any(TrackedSend::never_reached_the_server),
            "a refused send must still count as never having reached the server"
        );

        // The retry goes out rather than the message being dropped.
        app.clock_ms += backoff_ms(1);
        let effects = tick(&mut app);
        assert!(
            effects.iter().any(|e| matches!(e, crate::app::action::Effect::SendMessage { .. })),
            "a refused attempt must be retried"
        );

        let effects = reduce(&mut app, Action::Quit);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                crate::app::action::Effect::SaveDraftNow { text, .. } if text.contains("ship it")
            )),
            "quitting must put the undelivered text back into the draft"
        );
    }

    /// R1: a resend the server deduped carries the original command, so the
    /// client resolves a real delivery state instead of assuming success.
    #[test]
    fn a_deduped_resend_waits_on_the_original_command() {
        let mut app = app();
        let id = send_one(&mut app);
        dispatched(&mut app, id, "cid-1");
        let original = Uuid::from_u128(42);

        ack(&mut app, "cid-1", true, Some(original));
        assert_eq!(status(&app), Some(LineStatus::Sending), "still awaiting the adapter");

        reduce(
            &mut app,
            Action::Send(SendAction::DeliveryResult {
                command_id: original,
                ok: true,
                error: None,
            }),
        );
        assert_eq!(status(&app), Some(LineStatus::Delivered));
    }

    /// N1: a send whose effect never ran had `deadline_ms = i64::MAX`, so tick
    /// skipped it forever and it showed "Sending" with nothing to move it on.
    #[test]
    fn a_send_whose_effect_never_ran_does_not_wait_for_ever() {
        let mut app = app();
        let effects = super::submit(&mut app, "s-a".to_owned(), "hello".to_owned(), None);
        let (send_id, _) = sent(&effects);
        let send = app.outbox.find_mut(send_id).expect("the tracked send");
        assert_eq!(send.phase, Phase::Pending);
        assert!(send.deadline_ms < i64::MAX, "a send with no deadline can never be retried");

        // Nothing acks and no Dispatched arrives: the effect was dropped.
        app.clock_ms += DISPATCH_TIMEOUT_MS + 1;
        let retries = tick(&mut app);
        let send = app.outbox.find_mut(send_id).expect("the tracked send");
        assert_ne!(send.phase, Phase::Pending, "it must leave Pending once the deadline passes");
        assert!(
            retries.iter().any(|e| matches!(e, crate::app::action::Effect::SendMessage { .. }))
                || send.phase == Phase::Backoff,
            "it must be retried or parked, not stuck"
        );
    }

    /// The queue-full path hands the send straight to the failure route, which
    /// parks it for a retry rather than leaving it to the fallback deadline.
    #[test]
    fn a_dropped_dispatch_parks_the_send_for_a_retry() {
        let mut app = app();
        let effects = super::submit(&mut app, "s-a".to_owned(), "hello".to_owned(), None);
        let (send_id, _) = sent(&effects);

        reduce(
            &mut app,
            Action::Send(SendAction::DispatchFailed {
                send_id,
                reason: "the effect queue is full".to_owned(),
            }),
        );
        let send = app.outbox.find_mut(send_id).expect("the tracked send");
        assert_eq!(send.phase, Phase::Backoff, "a dropped send waits to be retried");
        assert!(send.deadline_ms > 0 && send.deadline_ms < i64::MAX);
        assert!(send.reason.is_some(), "the line says why it is waiting");
    }

    /// The other direction of the quit flush: a frame that did reach the socket
    /// must not be handed back as draft text, or the user sees it twice.
    #[test]
    fn a_dispatched_send_is_not_put_back_into_the_draft() {
        let mut app = app();
        let id = send_one(&mut app);
        assert!(
            app.outbox.tracked().any(TrackedSend::never_reached_the_server),
            "nothing has been written yet"
        );

        dispatched(&mut app, id, "cid-1");
        assert!(
            !app.outbox.tracked().any(TrackedSend::never_reached_the_server),
            "the frame is on the socket, so quitting must not duplicate it"
        );
    }
}
