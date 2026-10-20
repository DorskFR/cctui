pub mod state;
pub mod transport;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cctui_proto::ws::{ServerEvent, TuiCommand};
use tokio::sync::mpsc;

use crate::error::ClientError;
use state::{Ack, AckHandle, AckRegistry, Health, SubscriptionState, Watchdog, backoff};
use transport::{Connector, Frame, Transport};

const QUEUE: usize = 256;

/// How long [`WsClient::send_message_and_wait`] waits for a `message_ack`.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(15);

/// Anything that reaches the consumer off the socket, including its lifecycle.
///
/// A frame that fails to deserialize arrives as [`Incoming::Undecodable`]
/// rather than vanishing.
#[derive(Debug)]
pub enum Incoming {
    Connected,
    Disconnected(String),
    Event(Box<ServerEvent>),
    Undecodable(String),
}

/// The single place a websocket frame becomes an [`Incoming`].
#[must_use]
pub fn decode_frame(text: &str) -> Incoming {
    match serde_json::from_str::<ServerEvent>(text) {
        Ok(event) => Incoming::Event(Box::new(event)),
        Err(e) => Incoming::Undecodable(e.to_string()),
    }
}

/// A reconnecting client event stream.
///
/// Commands sent while the socket is down queue until it is back, and the
/// subscription set is re-sent on every fresh socket.
pub struct WsClient {
    outgoing: mpsc::Sender<TuiCommand>,
    subscriptions: Arc<Mutex<SubscriptionState>>,
    acks: Arc<AckRegistry>,
    /// Whether a socket is carrying frames right now.
    ///
    /// The outgoing queue accepts commands with no socket behind it, which is
    /// what lets a subscription survive a reconnect. A user message must not
    /// take that path: a queued frame is indistinguishable from a sent one, so
    /// its ack clock would start on a frame nobody has written.
    connected: Arc<AtomicBool>,
}

impl WsClient {
    /// Spawns the reconnect loop and returns the stream of everything it reads.
    pub fn start(connector: Arc<dyn Connector>) -> (Self, mpsc::Receiver<Incoming>) {
        Self::start_with(connector, Watchdog::default())
    }

    pub fn start_with(
        connector: Arc<dyn Connector>,
        watchdog: Watchdog,
    ) -> (Self, mpsc::Receiver<Incoming>) {
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<TuiCommand>(QUEUE);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Incoming>(QUEUE);
        let subscriptions = Arc::new(Mutex::new(SubscriptionState::new()));
        let acks = Arc::new(AckRegistry::new());
        let connected = Arc::new(AtomicBool::new(false));

        tokio::spawn(run(Loop {
            connector,
            watchdog,
            outgoing: outgoing_rx,
            incoming: incoming_tx,
            subscriptions: Arc::clone(&subscriptions),
            acks: Arc::clone(&acks),
            connected: Arc::clone(&connected),
        }));

        (Self { outgoing: outgoing_tx, subscriptions, acks, connected }, incoming_rx)
    }

    /// Queues `command`, recording anything the server would forget on a drop.
    pub async fn send(&self, command: TuiCommand) -> Result<(), ClientError> {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.record(&command);
        }
        self.outgoing.send(command).await.map_err(|_| ClientError::Disconnected)
    }

    pub async fn subscribe(&self, session_id: String) -> Result<(), ClientError> {
        self.send(TuiCommand::Subscribe { session_id }).await
    }

    pub async fn unsubscribe(&self, session_id: String) -> Result<(), ClientError> {
        self.send(TuiCommand::Unsubscribe { session_id }).await
    }

    pub async fn watch_terminal(&self, session_id: String, watch: bool) -> Result<(), ClientError> {
        self.send(TuiCommand::WatchTerminal { session_id, watch }).await
    }

    pub async fn respond_permission(
        &self,
        session_id: String,
        request_id: String,
        behavior: String,
    ) -> Result<(), ClientError> {
        self.send(TuiCommand::PermissionResponse {
            session_id,
            request_id,
            behavior,
            option_id: None,
        })
        .await
    }

    /// Sends a user message under a `client_msg_id` the caller owns.
    ///
    /// No ack is registered: the `message_ack` reaches the consumer on the
    /// event stream, for a caller that correlates delivery itself.
    pub async fn send_message_as(
        &self,
        session_id: String,
        content: String,
        client_msg_id: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        turn_id: Option<uuid::Uuid>,
    ) -> Result<(), ClientError> {
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }
        self.send(TuiCommand::Message {
            session_id,
            content,
            client_msg_id: Some(client_msg_id),
            ask_picks,
            turn_id,
        })
        .await
    }

    /// Sends a user message under a freshly minted `client_msg_id` and returns
    /// the handle its `message_ack` resolves.
    pub async fn send_message(
        &self,
        session_id: String,
        content: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        turn_id: Option<uuid::Uuid>,
    ) -> Result<AckHandle, ClientError> {
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }
        let client_msg_id = uuid::Uuid::new_v4().to_string();
        let rx = self.acks.register(client_msg_id.clone());
        let handle = AckHandle::new(client_msg_id.clone(), rx);
        let sent = self
            .send(TuiCommand::Message {
                session_id,
                content,
                client_msg_id: Some(client_msg_id.clone()),
                ask_picks,
                turn_id,
            })
            .await;
        if let Err(e) = sent {
            self.acks.forget(&client_msg_id);
            return Err(e);
        }
        Ok(handle)
    }

    /// Whether a socket is up right now. A send refused on this is parked for
    /// the next one rather than queued behind a dead transport.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// [`Self::send_message`] then [`AckHandle::wait`] under [`ACK_TIMEOUT`].
    pub async fn send_message_and_wait(
        &self,
        session_id: String,
        content: String,
    ) -> Result<Ack, ClientError> {
        self.send_message(session_id, content, None, None).await?.wait(ACK_TIMEOUT).await
    }

    #[must_use]
    pub fn is_subscribed(&self, session_id: &str) -> bool {
        self.subscriptions.lock().is_ok_and(|s| s.is_subscribed(session_id))
    }

    #[must_use]
    pub fn pending_acks(&self) -> usize {
        self.acks.pending_count()
    }
}

struct Loop {
    connector: Arc<dyn Connector>,
    watchdog: Watchdog,
    outgoing: mpsc::Receiver<TuiCommand>,
    incoming: mpsc::Sender<Incoming>,
    subscriptions: Arc<Mutex<SubscriptionState>>,
    acks: Arc<AckRegistry>,
    connected: Arc<AtomicBool>,
}

async fn run(mut ctx: Loop) {
    let mut attempt = 0_u32;
    loop {
        match ctx.connector.connect().await {
            Ok(transport) => {
                attempt = 0;
                ctx.connected.store(true, Ordering::SeqCst);
                let reason = pump(
                    &mut ctx.outgoing,
                    &ctx.incoming,
                    &ctx.subscriptions,
                    &ctx.acks,
                    ctx.watchdog,
                    transport,
                )
                .await;
                ctx.connected.store(false, Ordering::SeqCst);
                ctx.acks.fail_all();
                if ctx.incoming.send(Incoming::Disconnected(reason)).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::debug!(%e, "websocket connect failed");
                if ctx.incoming.is_closed() {
                    return;
                }
            }
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

/// Runs one socket to its end and reports why it ended.
async fn pump(
    outgoing: &mut mpsc::Receiver<TuiCommand>,
    incoming: &mpsc::Sender<Incoming>,
    subscriptions: &Arc<Mutex<SubscriptionState>>,
    acks: &AckRegistry,
    watchdog: Watchdog,
    mut transport: Box<dyn Transport>,
) -> String {
    for command in replay(subscriptions) {
        let Ok(json) = serde_json::to_string(&command) else { continue };
        if let Err(e) = transport.send_text(json).await {
            return format!("resubscribe failed: {e}");
        }
    }
    if incoming.send(Incoming::Connected).await.is_err() {
        return "consumer gone".to_owned();
    }

    let mut last_frame = Instant::now();
    let mut probed = false;
    let mut ticker = tokio::time::interval(watchdog.tick());

    loop {
        tokio::select! {
            frame = transport.recv() => {
                last_frame = Instant::now();
                probed = false;
                match frame {
                    None => return "stream ended".to_owned(),
                    Some(Err(e)) => return e.to_string(),
                    Some(Ok(Frame::Closed)) => return "server closed the socket".to_owned(),
                    Some(Ok(Frame::Other)) => {}
                    Some(Ok(Frame::Text(text))) => {
                        let decoded = decode_frame(&text);
                        if let Incoming::Event(event) = &decoded {
                            resolve_ack(acks, event);
                        }
                        if incoming.send(decoded).await.is_err() {
                            return "consumer gone".to_owned();
                        }
                    }
                }
            }
            Some(command) = outgoing.recv() => {
                let Ok(json) = serde_json::to_string(&command) else { continue };
                if let Err(e) = transport.send_text(json).await {
                    return format!("send failed: {e}");
                }
            }
            _ = ticker.tick() => {
                match watchdog.check(last_frame.elapsed()) {
                    Health::Probe if !probed => {
                        probed = true;
                        if let Err(e) = transport.send_ping().await {
                            return format!("keepalive ping failed: {e}");
                        }
                    }
                    Health::Live | Health::Probe => {}
                    Health::Dead => return "keepalive timeout".to_owned(),
                }
            }
        }
    }
}

fn replay(subscriptions: &Arc<Mutex<SubscriptionState>>) -> Vec<TuiCommand> {
    subscriptions.lock().map_or_else(|_| Vec::new(), |s| s.replay())
}

fn resolve_ack(acks: &AckRegistry, event: &ServerEvent) {
    if let ServerEvent::MessageAck { session_id, client_msg_id, ok, error, command_id } = event {
        let _ = acks.resolve(Ack {
            session_id: session_id.clone(),
            client_msg_id: client_msg_id.clone(),
            ok: *ok,
            error: error.clone(),
            command_id: *command_id,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;

    /// One scripted socket: frames it hands out, then the texts it received.
    struct FakeTransport {
        inbound: mpsc::Receiver<Frame>,
        outbound: mpsc::Sender<String>,
    }

    #[async_trait]
    impl Transport for FakeTransport {
        async fn send_text(&mut self, text: String) -> Result<(), ClientError> {
            self.outbound.send(text).await.map_err(|_| ClientError::Disconnected)
        }

        async fn send_ping(&mut self) -> Result<(), ClientError> {
            self.outbound.send("<ping>".to_owned()).await.map_err(|_| ClientError::Disconnected)
        }

        async fn recv(&mut self) -> Option<Result<Frame, ClientError>> {
            self.inbound.recv().await.map(Ok)
        }
    }

    /// Hands out one scripted socket per connect attempt.
    struct FakeConnector {
        sockets: Mutex<Vec<(mpsc::Receiver<Frame>, mpsc::Sender<String>)>>,
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl Connector for FakeConnector {
        async fn connect(&self) -> Result<Box<dyn Transport>, ClientError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let next = self
                .sockets
                .lock()
                .ok()
                .and_then(|mut s| if s.is_empty() { None } else { Some(s.remove(0)) });
            match next {
                Some((inbound, outbound)) => Ok(Box::new(FakeTransport { inbound, outbound })),
                None => Err(ClientError::Websocket("no more scripted sockets".to_owned())),
            }
        }
    }

    struct Socket {
        frames: mpsc::Sender<Frame>,
        sent: mpsc::Receiver<String>,
    }

    fn scripted(count: usize) -> (Arc<FakeConnector>, Vec<Socket>) {
        let mut sockets = Vec::new();
        let mut handles = Vec::new();
        for _ in 0..count {
            let (frames, inbound) = mpsc::channel::<Frame>(16);
            let (outbound, sent) = mpsc::channel::<String>(16);
            sockets.push((inbound, outbound));
            handles.push(Socket { frames, sent });
        }
        (
            Arc::new(FakeConnector { sockets: Mutex::new(sockets), attempts: AtomicUsize::new(0) }),
            handles,
        )
    }

    async fn next_incoming(rx: &mut mpsc::Receiver<Incoming>) -> Incoming {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("no incoming within the timeout")
            .expect("stream closed")
    }

    async fn next_sent(socket: &mut Socket) -> String {
        tokio::time::timeout(Duration::from_secs(2), socket.sent.recv())
            .await
            .expect("nothing sent within the timeout")
            .expect("socket closed")
    }

    fn ack_frame(client_msg_id: &str) -> Frame {
        Frame::Text(
            serde_json::json!({
                "type": "message_ack",
                "session_id": "s1",
                "client_msg_id": client_msg_id,
                "ok": true,
            })
            .to_string(),
        )
    }

    #[tokio::test]
    async fn an_ack_resolves_the_handle_of_its_own_message() {
        let (connector, mut sockets) = scripted(1);
        let mut socket = sockets.remove(0);
        let (client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        let first =
            client.send_message("s1".to_owned(), "one".to_owned(), None, None).await.unwrap();
        let second =
            client.send_message("s1".to_owned(), "two".to_owned(), None, None).await.unwrap();
        let _ = next_sent(&mut socket).await;
        let _ = next_sent(&mut socket).await;
        assert_eq!(client.pending_acks(), 2);

        socket.frames.send(ack_frame(second.client_msg_id())).await.unwrap();
        let resolved = second.wait(Duration::from_secs(2)).await.unwrap();
        assert!(resolved.ok);
        assert_eq!(resolved.session_id, "s1");
        assert!(matches!(
            first.wait(Duration::from_millis(100)).await,
            Err(ClientError::AckTimeout(_))
        ));
    }

    #[tokio::test]
    async fn a_sent_message_carries_a_minted_client_msg_id() {
        let (connector, mut sockets) = scripted(1);
        let mut socket = sockets.remove(0);
        let (client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        let handle =
            client.send_message("s1".to_owned(), "hi".to_owned(), None, None).await.unwrap();
        let wire: serde_json::Value = serde_json::from_str(&next_sent(&mut socket).await).unwrap();
        assert_eq!(wire["type"], "message");
        assert_eq!(wire["client_msg_id"], handle.client_msg_id());
        assert!(uuid::Uuid::parse_str(handle.client_msg_id()).is_ok());
    }

    #[tokio::test]
    async fn a_caller_owned_id_reaches_the_wire_unregistered() {
        let (connector, mut sockets) = scripted(1);
        let mut socket = sockets.remove(0);
        let (client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        client
            .send_message_as("s1".to_owned(), "hi".to_owned(), "mine-1".to_owned(), None, None)
            .await
            .unwrap();
        let wire: serde_json::Value = serde_json::from_str(&next_sent(&mut socket).await).unwrap();
        assert_eq!(wire["client_msg_id"], "mine-1");
        assert_eq!(client.pending_acks(), 0);
    }

    #[tokio::test]
    async fn a_reconnect_replays_the_subscription_state() {
        let (connector, mut sockets) = scripted(2);
        let mut first = sockets.remove(0);
        let mut second = sockets.remove(0);
        let (client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        client.subscribe("a".to_owned()).await.unwrap();
        client.subscribe("b".to_owned()).await.unwrap();
        client.watch_terminal("a".to_owned(), true).await.unwrap();
        client.unsubscribe("b".to_owned()).await.unwrap();
        for _ in 0..4 {
            let _ = next_sent(&mut first).await;
        }

        drop(first);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Disconnected(_)));
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        let replayed: Vec<serde_json::Value> = vec![
            serde_json::from_str(&next_sent(&mut second).await).unwrap(),
            serde_json::from_str(&next_sent(&mut second).await).unwrap(),
        ];
        assert_eq!(replayed[0]["type"], "subscribe");
        assert_eq!(replayed[0]["session_id"], "a");
        assert_eq!(replayed[1]["type"], "watch_terminal");
        assert_eq!(replayed[1]["watch"], true);
        assert!(client.is_subscribed("a"));
        assert!(!client.is_subscribed("b"));
    }

    #[tokio::test]
    async fn a_dropped_socket_fails_the_acks_it_was_carrying() {
        let (connector, mut sockets) = scripted(2);
        let first = sockets.remove(0);
        let (client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        let handle =
            client.send_message("s1".to_owned(), "hi".to_owned(), None, None).await.unwrap();
        drop(first);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Disconnected(_)));
        assert!(matches!(
            handle.wait(Duration::from_secs(2)).await,
            Err(ClientError::Disconnected)
        ));
    }

    #[tokio::test]
    async fn an_undecodable_frame_is_surfaced_not_dropped() {
        let (connector, mut sockets) = scripted(1);
        let socket = sockets.remove(0);
        let (_client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        socket.frames.send(Frame::Text("{\"type\":\"nope\"}".to_owned())).await.unwrap();
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Undecodable(_)));
    }

    #[tokio::test]
    async fn a_quiet_socket_is_probed_then_declared_dead() {
        let (connector, mut sockets) = scripted(2);
        let mut first = sockets.remove(0);
        let watchdog = Watchdog {
            probe_after: Duration::from_millis(60),
            dead_after: Duration::from_millis(400),
        };
        let (_client, mut incoming) = WsClient::start_with(connector, watchdog);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));

        assert_eq!(next_sent(&mut first).await, "<ping>");
        let reason = match next_incoming(&mut incoming).await {
            Incoming::Disconnected(reason) => reason,
            other => panic!("expected a disconnect, got {other:?}"),
        };
        assert_eq!(reason, "keepalive timeout");
    }

    /// F20: the outgoing queue accepts frames with no socket behind it, which is
    /// right for a subscription and wrong for a user message: the caller would
    /// start an ack clock on a frame nobody wrote, time out, and resend.
    #[tokio::test]
    async fn a_message_is_refused_rather_than_queued_on_a_dead_socket() {
        let (connector, mut sockets) = scripted(2);
        let first = sockets.remove(0);
        let (client, mut incoming) = WsClient::start(connector);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Connected));
        assert!(client.is_connected());

        drop(first);
        assert!(matches!(next_incoming(&mut incoming).await, Incoming::Disconnected(_)));
        assert!(!client.is_connected());

        let refused = client
            .send_message_as("s1".to_owned(), "hi".to_owned(), "cid-1".to_owned(), None, None)
            .await;
        assert!(matches!(refused, Err(ClientError::Disconnected)));

        // A subscription still queues: it is replayed on the next socket.
        assert!(client.subscribe("a".to_owned()).await.is_ok());
    }
}
