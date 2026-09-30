use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

use cctui_proto::ws::TuiCommand;
use tokio::sync::oneshot;

use crate::error::ClientError;

/// What the server forgets when a socket drops, and the client must re-send.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SubscriptionState {
    sessions: BTreeSet<String>,
    terminals: BTreeSet<String>,
}

impl SubscriptionState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, command: &TuiCommand) {
        match command {
            TuiCommand::Subscribe { session_id } => {
                self.sessions.insert(session_id.clone());
            }
            TuiCommand::Unsubscribe { session_id } => {
                self.sessions.remove(session_id);
                self.terminals.remove(session_id);
            }
            TuiCommand::WatchTerminal { session_id, watch } => {
                if *watch {
                    self.terminals.insert(session_id.clone());
                } else {
                    self.terminals.remove(session_id);
                }
            }
            TuiCommand::Message { .. } | TuiCommand::PermissionResponse { .. } => {}
        }
    }

    /// The commands that rebuild this state on a fresh socket.
    #[must_use]
    pub fn replay(&self) -> Vec<TuiCommand> {
        let mut out: Vec<TuiCommand> = self
            .sessions
            .iter()
            .map(|session_id| TuiCommand::Subscribe { session_id: session_id.clone() })
            .collect();
        out.extend(self.terminals.iter().map(|session_id| TuiCommand::WatchTerminal {
            session_id: session_id.clone(),
            watch: true,
        }));
        out
    }

    #[must_use]
    pub fn is_subscribed(&self, session_id: &str) -> bool {
        self.sessions.contains(session_id)
    }

    #[must_use]
    pub fn watches_terminal(&self, session_id: &str) -> bool {
        self.terminals.contains(session_id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.terminals.is_empty()
    }
}

/// The server's verdict on one sent message. `ok` means queued to a daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    pub session_id: String,
    pub client_msg_id: String,
    pub ok: bool,
    pub error: Option<String>,
    pub command_id: Option<uuid::Uuid>,
}

/// Outstanding `client_msg_id` → waiter map.
#[derive(Debug, Default)]
pub struct AckRegistry {
    pending: Mutex<Vec<(String, oneshot::Sender<Ack>)>>,
}

impl AckRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `client_msg_id` and returns the handle that resolves on its ack.
    pub fn register(&self, client_msg_id: String) -> oneshot::Receiver<Ack> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.push((client_msg_id, tx));
        }
        rx
    }

    /// Whether the ack matched a waiter.
    pub fn resolve(&self, ack: Ack) -> bool {
        let Ok(mut pending) = self.pending.lock() else { return false };
        let Some(index) = pending.iter().position(|(id, _)| *id == ack.client_msg_id) else {
            return false;
        };
        let (_, tx) = pending.swap_remove(index);
        tx.send(ack).is_ok()
    }

    pub fn forget(&self, client_msg_id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|(id, _)| id != client_msg_id);
        }
    }

    /// Drops every waiter: a lost socket means those acks will never arrive.
    pub fn fail_all(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.lock().map_or(0, |pending| pending.len())
    }
}

/// Reconnect delay for attempt `n`, capped so a long outage still retries.
#[must_use]
pub const fn backoff(attempt: u32) -> Duration {
    let secs = match attempt {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => 30,
    };
    Duration::from_secs(secs)
}

/// What to do about a socket that has been quiet for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Live,
    /// Quiet: prove the socket is still there.
    Probe,
    /// Quiet past a completed probe: the socket is half-open.
    Dead,
}

/// Turns "time since the last frame" into a verdict.
///
/// A TCP connection dropped by a NAT or a load balancer stays writable, so
/// silence — not a write error — is the only signal a client gets.
#[derive(Debug, Clone, Copy)]
pub struct Watchdog {
    pub probe_after: Duration,
    pub dead_after: Duration,
}

impl Default for Watchdog {
    fn default() -> Self {
        Self { probe_after: Duration::from_secs(30), dead_after: Duration::from_secs(75) }
    }
}

impl Watchdog {
    #[must_use]
    pub const fn check(&self, quiet_for: Duration) -> Health {
        if quiet_for >= self.dead_after {
            Health::Dead
        } else if quiet_for >= self.probe_after {
            Health::Probe
        } else {
            Health::Live
        }
    }

    /// How often the loop must ask [`Self::check`] to catch both thresholds.
    #[must_use]
    pub fn tick(&self) -> Duration {
        (self.probe_after / 2).max(Duration::from_millis(20))
    }
}

/// Waits for the ack of one sent message.
#[derive(Debug)]
pub struct AckHandle {
    client_msg_id: String,
    rx: oneshot::Receiver<Ack>,
}

impl AckHandle {
    #[must_use]
    pub const fn new(client_msg_id: String, rx: oneshot::Receiver<Ack>) -> Self {
        Self { client_msg_id, rx }
    }

    #[must_use]
    pub fn client_msg_id(&self) -> &str {
        &self.client_msg_id
    }

    /// `Disconnected` if the socket dropped before the ack, `AckTimeout` if it
    /// simply never came.
    pub async fn wait(self, timeout: Duration) -> Result<Ack, ClientError> {
        let Self { client_msg_id, rx } = self;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(ack)) => Ok(ack),
            Ok(Err(_)) => Err(ClientError::Disconnected),
            Err(_) => Err(ClientError::AckTimeout(client_msg_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: &str) -> TuiCommand {
        TuiCommand::Subscribe { session_id: id.to_owned() }
    }

    /// `TuiCommand` has no `PartialEq`, so replays are compared as
    /// `(kind, session_id)` pairs.
    fn shape(state: &SubscriptionState) -> Vec<(&'static str, String)> {
        state
            .replay()
            .into_iter()
            .map(|command| match command {
                TuiCommand::Subscribe { session_id } => ("subscribe", session_id),
                TuiCommand::WatchTerminal { session_id, watch: true } => ("watch", session_id),
                other => panic!("replay must not contain {other:?}"),
            })
            .collect()
    }

    #[test]
    fn replay_rebuilds_subscriptions_in_a_stable_order() {
        let mut state = SubscriptionState::new();
        state.record(&sub("b"));
        state.record(&sub("a"));
        assert_eq!(
            shape(&state),
            vec![("subscribe", "a".to_owned()), ("subscribe", "b".to_owned())]
        );
    }

    #[test]
    fn unsubscribe_is_not_replayed() {
        let mut state = SubscriptionState::new();
        state.record(&sub("a"));
        state.record(&sub("b"));
        state.record(&TuiCommand::Unsubscribe { session_id: "a".to_owned() });
        assert!(!state.is_subscribed("a"));
        assert!(state.is_subscribed("b"));
        assert_eq!(state.replay().len(), 1);
    }

    #[test]
    fn terminal_watch_is_replayed_after_its_subscription() {
        let mut state = SubscriptionState::new();
        state.record(&sub("a"));
        state.record(&TuiCommand::WatchTerminal { session_id: "a".to_owned(), watch: true });
        let replay = state.replay();
        assert!(matches!(replay.first(), Some(TuiCommand::Subscribe { .. })));
        assert!(matches!(replay.get(1), Some(TuiCommand::WatchTerminal { watch: true, .. })));
        assert!(state.watches_terminal("a"));
    }

    #[test]
    fn unwatching_a_terminal_drops_it_from_the_replay() {
        let mut state = SubscriptionState::new();
        state.record(&sub("a"));
        state.record(&TuiCommand::WatchTerminal { session_id: "a".to_owned(), watch: true });
        state.record(&TuiCommand::WatchTerminal { session_id: "a".to_owned(), watch: false });
        assert!(!state.watches_terminal("a"));
        assert_eq!(shape(&state), vec![("subscribe", "a".to_owned())]);
    }

    #[test]
    fn unsubscribing_also_stops_watching_the_terminal() {
        let mut state = SubscriptionState::new();
        state.record(&sub("a"));
        state.record(&TuiCommand::WatchTerminal { session_id: "a".to_owned(), watch: true });
        state.record(&TuiCommand::Unsubscribe { session_id: "a".to_owned() });
        assert!(state.is_empty());
        assert!(state.replay().is_empty());
    }

    #[test]
    fn messages_do_not_become_subscription_state() {
        let mut state = SubscriptionState::new();
        state.record(&TuiCommand::Message {
            session_id: "a".to_owned(),
            content: "hi".to_owned(),
            client_msg_id: Some("m1".to_owned()),
            ask_picks: None,
            turn_id: None,
        });
        assert!(state.is_empty());
    }

    fn ack(client_msg_id: &str) -> Ack {
        Ack {
            session_id: "s1".to_owned(),
            client_msg_id: client_msg_id.to_owned(),
            ok: true,
            error: None,
            command_id: None,
        }
    }

    #[tokio::test]
    async fn the_matching_ack_resolves_its_own_handle() {
        let registry = AckRegistry::new();
        let first = AckHandle::new("m1".to_owned(), registry.register("m1".to_owned()));
        let second = AckHandle::new("m2".to_owned(), registry.register("m2".to_owned()));
        assert_eq!(registry.pending_count(), 2);

        assert!(registry.resolve(ack("m2")));
        assert_eq!(registry.pending_count(), 1);

        let resolved = second.wait(Duration::from_millis(50)).await.unwrap();
        assert_eq!(resolved.client_msg_id, "m2");
        assert!(matches!(
            first.wait(Duration::from_millis(10)).await,
            Err(ClientError::AckTimeout(id)) if id == "m1"
        ));
    }

    #[tokio::test]
    async fn an_unknown_ack_resolves_nothing() {
        let registry = AckRegistry::new();
        let _handle = AckHandle::new("m1".to_owned(), registry.register("m1".to_owned()));
        assert!(!registry.resolve(ack("other")));
        assert_eq!(registry.pending_count(), 1);
    }

    #[tokio::test]
    async fn an_error_ack_still_resolves_the_handle() {
        let registry = AckRegistry::new();
        let handle = AckHandle::new("m1".to_owned(), registry.register("m1".to_owned()));
        registry.resolve(Ack { ok: false, error: Some("no daemon".to_owned()), ..ack("m1") });
        let resolved = handle.wait(Duration::from_millis(50)).await.unwrap();
        assert!(!resolved.ok);
        assert_eq!(resolved.error.as_deref(), Some("no daemon"));
    }

    #[tokio::test]
    async fn a_dropped_socket_fails_every_waiter() {
        let registry = AckRegistry::new();
        let handle = AckHandle::new("m1".to_owned(), registry.register("m1".to_owned()));
        registry.fail_all();
        assert_eq!(registry.pending_count(), 0);
        assert!(matches!(
            handle.wait(Duration::from_secs(5)).await,
            Err(ClientError::Disconnected)
        ));
    }

    #[tokio::test]
    async fn forget_drops_one_waiter() {
        let registry = AckRegistry::new();
        let _rx = registry.register("m1".to_owned());
        registry.forget("m1");
        assert_eq!(registry.pending_count(), 0);
    }

    #[test]
    fn backoff_grows_then_caps() {
        let secs: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn the_watchdog_probes_before_declaring_death() {
        let dog = Watchdog::default();
        assert_eq!(dog.check(Duration::from_secs(1)), Health::Live);
        assert_eq!(dog.check(Duration::from_secs(29)), Health::Live);
        assert_eq!(dog.check(Duration::from_secs(30)), Health::Probe);
        assert_eq!(dog.check(Duration::from_secs(74)), Health::Probe);
        assert_eq!(dog.check(Duration::from_secs(75)), Health::Dead);
        assert!(dog.tick() < dog.probe_after);
    }
}
