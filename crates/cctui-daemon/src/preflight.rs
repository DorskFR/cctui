//! The launch preflight every harness runs before its first turn.
//!
//! Two things must happen between "the session exists" and "turn 1 is sent",
//! and neither is harness-specific:
//!
//! 1. **The usage-limit hold.** Launching into a soft-limit-blocked model
//!    spends the first turn on a 429. The server already knows the answer
//!    (`GET /sessions/{id}/limits`), so the daemon asks and waits out
//!    `retry_after` instead, showing the wait on the session card. The
//!    decision logic itself lives in [`crate::launchgate`].
//! 2. **The MCP-readiness wait.** A session that may call `CctuiAgent` races
//!    its own relay if turn 1 goes out before the relay answers `initialize`.
//!    [`crate::mcpready`] tracks that; this is where the turn waits for it.
//!
//! Claude-code leaves [`Preflight::with_relay`] unset: its `SessionStart` hook
//! already holds turn 1 in-band.
//!
//! Every failure mode fails OPEN. An unreachable server, a malformed reply, a
//! hold past [`crate::launchgate::MAX_HOLD`] or a relay that never connects all
//! release the launch: these gates exist to save a turn, never to cost one.

use std::time::{Duration, Instant};

use cctui_proto::adapter::AdapterEvent;
use tokio::sync::mpsc;

/// Seconds the launch may wait for the session's MCP relay when the operator
/// names no other number.
const DEFAULT_RELAY_WAIT_SECS: u64 = 8;

/// Longest the gate may ever hold a first turn, whatever the operator asks
/// for. A relay is either up in seconds or not coming.
const MAX_RELAY_WAIT_SECS: u64 = 60;

/// Seconds the launch may wait for the session's MCP relay, as
/// `CCTUI_MCP_READY_WAIT_SECS` sets it. `0` disables the gate for every
/// harness. Read only here and by claude-code's `SessionStart` hook, so the
/// knob has exactly one production reader per delivery path.
#[must_use]
pub fn mcp_ready_wait_secs() -> u64 {
    relay_wait_secs(std::env::var("CCTUI_MCP_READY_WAIT_SECS").ok().as_deref())
}

/// The wait `raw` asks for, defaulted and clamped. Split from the environment
/// read so it is testable without mutating process-global state.
#[must_use]
fn relay_wait_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RELAY_WAIT_SECS)
        .min(MAX_RELAY_WAIT_SECS)
}

/// Where the limits question is asked.
struct Limits {
    server: crate::client::ServerClient,
    machine_key: String,
}

/// A launch preflight. Built at spawn time, bound to a card once the harness
/// has minted its local id, awaited immediately before the first turn.
pub struct Preflight {
    events: mpsc::Sender<AdapterEvent>,
    model: Option<String>,
    limits: Option<Limits>,
    /// The id the limits endpoint is keyed on. Defaults to the card id, which
    /// is what codex and opencode want; claude-code sets it explicitly because
    /// its card id (the worker `short`) is not its session id.
    limits_session_id: Option<String>,
    /// `local_id` the hold is reported on. Absent until the session exists, at
    /// which point the wait is silent rather than mis-addressed.
    card_id: Option<String>,
    /// The `mcpready` key of this session's relay, when it has one.
    relay_key: Option<String>,
    /// How long the first turn waits for that relay. Resolved from the
    /// environment when the relay is declared, so [`Self::run`] reads no
    /// global state.
    relay_wait: Duration,
}

impl Preflight {
    #[must_use]
    pub fn new(events: mpsc::Sender<AdapterEvent>, model: Option<String>) -> Self {
        Self {
            events,
            model,
            limits: None,
            limits_session_id: None,
            card_id: None,
            relay_key: None,
            relay_wait: Duration::ZERO,
        }
    }

    /// Ask this server about the job's model. An unattached daemon (no server
    /// or no machine key) launches unconditionally.
    #[must_use]
    pub fn with_limits(
        mut self,
        server: Option<&crate::client::ServerClient>,
        machine_key: Option<&str>,
    ) -> Self {
        if let (Some(server), Some(machine_key)) = (server, machine_key) {
            self.limits =
                Some(Limits { server: server.clone(), machine_key: machine_key.to_owned() });
        }
        self
    }

    /// Key the limits question on an id other than the card's.
    #[must_use]
    pub fn with_session(mut self, session_id: &str) -> Self {
        self.limits_session_id = Some(session_id.to_owned());
        self
    }

    #[must_use]
    pub fn with_card(mut self, local_id: &str) -> Self {
        self.card_id = Some(local_id.to_owned());
        self
    }

    /// Wait for the relay registered under `key` before the first turn, and
    /// start its connect clock now. `None` leaves the relay gate off.
    #[must_use]
    pub fn with_relay(mut self, key: Option<String>) -> Self {
        if let Some(key) = key {
            crate::mcpready::note_launch(&key);
            self.relay_key = Some(key);
            self.relay_wait = Duration::from_secs(mcp_ready_wait_secs());
        }
        self
    }

    /// Override the relay wait a [`Self::with_relay`] resolved.
    #[must_use]
    pub fn with_relay_wait(mut self, wait: Duration) -> Self {
        self.relay_wait = wait;
        self
    }

    /// Hold until the model is allowed and the relay is up, then return.
    pub async fn run(&self) {
        self.run_on(None).await;
    }

    /// [`Self::run`] against a card the harness only minted after the
    /// preflight was built — codex and opencode learn their local id from the
    /// harness, not from the launch.
    pub async fn run_bound(&self, local_id: &str) {
        self.run_on(Some(local_id)).await;
    }

    async fn run_on(&self, local_id: Option<&str>) {
        let card = local_id.or(self.card_id.as_deref());
        self.hold(card).await;
        self.await_relay().await;
    }

    fn limits_id<'a>(&'a self, card: Option<&'a str>) -> Option<&'a str> {
        self.limits_session_id.as_deref().or(card)
    }

    /// Block until the job's model is allowed, the hold outlives
    /// [`crate::launchgate::MAX_HOLD`], or the limits call fails.
    async fn hold(&self, card: Option<&str>) {
        let (Some(limits), Some(session_id)) = (self.limits.as_ref(), self.limits_id(card)) else {
            return;
        };
        let began = Instant::now();
        let mut waiting = false;
        loop {
            let payload = match limits
                .server
                .session_limits(&limits.machine_key, session_id, self.model.as_deref())
                .await
            {
                Ok(payload) => payload,
                Err(err) => {
                    tracing::warn!(session = %session_id, %err, "launch limits check failed; launching anyway");
                    return;
                }
            };
            let Some(hold) = crate::launchgate::hold_from_limits(&payload, self.model.as_deref())
            else {
                if waiting {
                    tracing::info!(
                        session = %session_id,
                        waited_secs = %began.elapsed().as_secs(),
                        "launch limit cleared; dispatching"
                    );
                    self.report(card, None).await;
                }
                return;
            };
            if crate::launchgate::expired(began) {
                tracing::warn!(
                    session = %session_id,
                    reason = %hold.reason,
                    "launch held too long; dispatching anyway"
                );
                self.report(card, None).await;
                return;
            }
            if !waiting {
                tracing::info!(
                    session = %session_id,
                    model = ?self.model,
                    reason = %hold.reason,
                    retry_after_secs = %hold.retry_after.as_secs(),
                    "holding launch: the model is limit blocked"
                );
            }
            waiting = true;
            self.report(card, Some(&hold)).await;
            tokio::time::sleep(crate::launchgate::backoff(&hold)).await;
        }
    }

    /// Hold the first turn until this session's relay has answered
    /// `initialize`. A relay that never connects releases the turn anyway.
    async fn await_relay(&self) {
        let Some(key) = self.relay_key.as_deref() else { return };
        if self.relay_wait.is_zero() {
            return;
        }
        let _ = crate::mcpready::wait_until_ready(key, self.relay_wait).await;
    }

    /// Put the wait (or its end) on the session card.
    async fn report(&self, card: Option<&str>, hold: Option<&crate::launchgate::Hold>) {
        let Some(local_id) = card else { return };
        let state = if hold.is_some() { "held" } else { "starting" };
        let _ = self
            .events
            .send(AdapterEvent::Status {
                local_id: local_id.to_owned(),
                tempo: None,
                state: Some(state.to_owned()),
                detail: hold.map(crate::launchgate::Hold::card_detail),
                activity: None,
                name: None,
                intent: None,
                model: self.model.clone(),
                effort: None,
                permission_mode: None,
                children: Vec::new(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Preflight};
    use cctui_proto::adapter::AdapterEvent;
    use tokio::sync::mpsc;

    /// With nothing configured the preflight is a no-op: an unattached daemon
    /// must launch unconditionally rather than stall.
    #[tokio::test]
    async fn a_preflight_without_a_server_or_a_relay_returns_at_once() {
        let (tx, mut rx) = mpsc::channel(4);
        Preflight::new(tx, Some("opus".to_owned())).with_card("s1").run().await;
        assert!(rx.try_recv().is_err(), "nothing to report means no card update");
    }

    /// An unbound preflight must not address a card that does not exist yet.
    #[tokio::test]
    async fn an_unbound_preflight_reports_nothing() {
        let (tx, mut rx) = mpsc::channel(4);
        let pf = Preflight::new(tx, None);
        pf.report(
            None,
            Some(&crate::launchgate::Hold {
                retry_after: Duration::from_secs(30),
                reason: "weekly cap".to_owned(),
            }),
        )
        .await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_bound_preflight_puts_the_hold_on_its_card() {
        let (tx, mut rx) = mpsc::channel(4);
        let pf = Preflight::new(tx, Some("gpt-5.6".to_owned()));
        pf.report(
            Some("thread-1"),
            Some(&crate::launchgate::Hold {
                retry_after: Duration::from_mins(3),
                reason: "weekly cap".to_owned(),
            }),
        )
        .await;
        match rx.try_recv().expect("a status") {
            AdapterEvent::Status { local_id, state, detail, model, .. } => {
                assert_eq!(local_id, "thread-1");
                assert_eq!(state.as_deref(), Some("held"));
                let detail = detail.expect("the wait is spelled out");
                assert!(detail.contains("3m00s"), "{detail}");
                assert!(detail.contains("weekly cap"), "{detail}");
                assert_eq!(model.as_deref(), Some("gpt-5.6"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        pf.report(Some("thread-1"), None).await;
        match rx.try_recv().expect("the release") {
            AdapterEvent::Status { state, detail, .. } => {
                assert_eq!(state.as_deref(), Some("starting"));
                assert!(detail.is_none(), "a cleared hold leaves no stale detail");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// The limits question follows the card unless the adapter keys it
    /// elsewhere — claude-code's card id is its worker short, not its session.
    #[test]
    fn the_limits_id_defaults_to_the_card_and_yields_to_an_explicit_one() {
        let (tx, _rx) = mpsc::channel(1);
        let pf = Preflight::new(tx.clone(), None);
        assert_eq!(pf.limits_id(None), None);
        assert_eq!(pf.limits_id(Some("ses_abc")), Some("ses_abc"));

        let pf = Preflight::new(tx, None).with_session("uuid-1").with_card("c0ffee00");
        assert_eq!(pf.limits_id(pf.card_id.as_deref()), Some("uuid-1"));
        assert_eq!(
            pf.limits_id(Some("thread-9")),
            Some("uuid-1"),
            "an explicit session id wins a late binding"
        );
    }

    /// The relay key arms `mcpready` at build time, so the connect latency is
    /// measured from the launch and not from the wait.
    #[tokio::test]
    async fn declaring_a_relay_arms_the_readiness_clock_and_the_wait_releases_on_announce() {
        let (tx, _rx) = mpsc::channel(4);
        let key = "preflight-relay-1";
        crate::mcpready::forget(key);
        let pf = Preflight::new(tx, None).with_relay(Some(key.to_owned()));
        assert!(!crate::mcpready::is_ready(key));
        let waiter = tokio::spawn(async move { pf.run().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        crate::mcpready::announce(key);
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the announce releases the turn")
            .expect("the wait task ran");
        crate::mcpready::forget(key);
    }

    /// A relay that never connects must not cost the session its launch.
    #[tokio::test]
    async fn a_relay_that_never_connects_still_releases_the_turn() {
        let (tx, _rx) = mpsc::channel(4);
        let key = "preflight-relay-2";
        crate::mcpready::forget(key);
        let pf = Preflight::new(tx, None)
            .with_relay(Some(key.to_owned()))
            .with_relay_wait(Duration::from_millis(80));
        tokio::time::timeout(Duration::from_secs(10), pf.run()).await.expect("the wait is bounded");
        crate::mcpready::forget(key);
    }

    /// A zero wait is the operator switching the gate off, not a zero-length
    /// poll: the turn goes out without consulting the relay at all.
    #[tokio::test]
    async fn a_zero_wait_skips_the_relay_gate() {
        let (tx, _rx) = mpsc::channel(4);
        let key = "preflight-relay-3";
        crate::mcpready::forget(key);
        let pf = Preflight::new(tx, None)
            .with_relay(Some(key.to_owned()))
            .with_relay_wait(Duration::ZERO);
        tokio::time::timeout(Duration::from_secs(5), pf.run()).await.expect("no wait at all");
        assert!(!crate::mcpready::is_ready(key), "the gate was skipped, not satisfied");
        crate::mcpready::forget(key);
    }

    #[test]
    fn the_readiness_wait_is_defaulted_clamped_and_switchable() {
        use super::relay_wait_secs;
        assert_eq!(relay_wait_secs(None), 8, "the default when the knob is unset");
        assert_eq!(relay_wait_secs(Some("nonsense")), 8, "an unreadable value is no value");
        assert_eq!(relay_wait_secs(Some("0")), 0, "0 disables the gate");
        assert_eq!(relay_wait_secs(Some(" 12 ")), 12);
        assert_eq!(relay_wait_secs(Some("600")), 60, "clamped to a minute");
    }
}
