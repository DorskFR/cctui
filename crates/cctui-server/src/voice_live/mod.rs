//! `/api/v1/ws/voice?session=<id>`: a hands-free voice conversation with one
//! session.
//!
//! Binary frames carry audio both ways (16 kHz mono PCM up, synthesized
//! speech down); text frames carry [`VoiceClientMsg`] / [`VoiceServerMsg`]
//! JSON.

mod audio;
mod engine;
mod mainline;
pub mod text;

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use bytes::Bytes;
use cctui_proto::voice::{VoiceClientMsg, VoiceServerMsg};
use cctui_proto::ws::{AgentEvent, ServerEvent};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::auth::{AuthContext, Scope};
use crate::bus::{VOICE_WATCH_REFRESH, VoiceEvent};
use crate::speech::SpeechClient;
use crate::state::AppState;
pub use engine::{
    AudioStream, Inbound, Outbound, Prefs, Signal, VoiceBackend, picked_language, run_voice_loop,
};
use text::{UTTERANCE_SAMPLE_RATE, pcm16_to_wav};

/// Well over the longest utterance an old browser sends whole; new ones
/// stream parts of at most [`audio::MAX_CHUNK_BYTES`].
const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(20);
/// Half a second of silence: enough for the speech service to load its model.
const WARM_UP_SILENCE: [u8; 16_000] = [0; 16_000];
/// How the session stream renders a prompt the agent received.
const USER_PREFIX: &str = "▷ User:";
/// The annotation claude-code writes when a turn is over.
const TURN_DURATION: &str = "turn_duration";

#[derive(Debug, Deserialize)]
pub struct VoiceQuery {
    pub session: String,
    /// Spoken language, as a `mode` frame's `language`.
    #[serde(default)]
    pub language: Option<String>,
}

/// The principal gate shared with the main socket: a human key with `read`.
/// Session ownership is checked separately, with the write gate the
/// interrupt route uses.
pub fn principal_permitted(ctx: &AuthContext) -> Result<(), StatusCode> {
    if ctx.machine_id.is_some() || !ctx.has(Scope::Read) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub async fn voice_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<VoiceQuery>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    if !crate::ws::origin_permitted(&state.config, &headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    let token = crate::auth::bearer_or_cookie(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    principal_permitted(&ctx)?;
    let session_id = query.session.trim().to_owned();
    if session_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    crate::authz::authorize_session_write(&ctx, &session_id, &state.pool).await?;
    crate::routes::sessions::authorize_reply(&state, &ctx, &session_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let language = query.language.as_deref().and_then(picked_language);
    let ws = ws.max_message_size(MAX_WS_MESSAGE_BYTES).max_frame_size(MAX_WS_MESSAGE_BYTES);
    Ok(ws.on_upgrade(move |socket| handle_voice_ws(socket, state, ctx, session_id, language)))
}

/// Speech through the instance speech service, the session through the same
/// reply and interrupt paths as the typed UI.
struct ServerBackend {
    state: AppState,
    session_id: String,
    user_id: uuid::Uuid,
    client: SpeechClient,
}

impl VoiceBackend for ServerBackend {
    async fn transcribe(&self, audio: Bytes, language: Option<&str>) -> Result<String, String> {
        let wav = Bytes::from(pcm16_to_wav(&audio, UTTERANCE_SAMPLE_RATE));
        self.client.transcribe(wav, "audio/wav", language, None).await.map_err(|e| e.to_string())
    }

    async fn warm_up(&self, language: Option<&str>) -> Result<(), String> {
        let wav = Bytes::from(pcm16_to_wav(&WARM_UP_SILENCE, UTTERANCE_SAMPLE_RATE));
        self.client
            .transcribe(wav, "audio/wav", language, None)
            .await
            .map(drop)
            .map_err(|e| e.to_string())
    }

    async fn synthesize(
        &self,
        text: &str,
        voice: Option<&str>,
        speed: Option<f32>,
    ) -> Result<AudioStream, String> {
        let stream = self
            .client
            .synthesize_stream(text, voice, None, speed)
            .await
            .map_err(|e| e.to_string())?;
        Ok(stream.map_err(|e| e.to_string()).boxed())
    }

    async fn send_message(&self, text: String) -> Result<(), String> {
        let (_, dispatch) =
            crate::ws::dispatch_reply(&self.state, &self.session_id, text, None, None).await;
        dispatch.map_err(|e| e.to_string())
    }

    async fn interrupt(&self) -> Result<(), String> {
        crate::routes::sessions::dispatch_interrupt(&self.state, &self.session_id, self.user_id)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn spawn_writer(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Outbound>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(KEEPALIVE);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;
        loop {
            let msg = tokio::select! {
                frame = rx.recv() => match frame {
                    Some(Outbound::Audio(bytes)) => Message::Binary(bytes),
                    Some(Outbound::Control(msg)) => match serde_json::to_string(&msg) {
                        Ok(json) => Message::Text(json.into()),
                        Err(_) => continue,
                    },
                    None => break,
                },
                _ = keepalive.tick() => Message::Ping(Vec::new().into()),
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    })
}

async fn read_socket(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    inbound: mpsc::Sender<Inbound>,
) {
    let mut part_next = false;
    while let Some(Ok(msg)) = stream.next().await {
        let item = match msg {
            Message::Binary(bytes) if bytes.len() >= 2 => {
                if std::mem::take(&mut part_next) {
                    Inbound::Part(bytes)
                } else {
                    Inbound::Audio(bytes)
                }
            }
            Message::Text(text) => match serde_json::from_str::<VoiceClientMsg>(&text) {
                Ok(VoiceClientMsg::Part {}) => {
                    part_next = true;
                    continue;
                }
                Ok(VoiceClientMsg::UtteranceEnd {}) => Inbound::UtteranceEnd,
                Ok(c) => Inbound::Control(c),
                Err(err) => {
                    tracing::debug!(%err, "voice_ws: unreadable control frame");
                    continue;
                }
            },
            Message::Close(_) => break,
            _ => continue,
        };
        if inbound.send(item).await.is_err() {
            break;
        }
    }
}

/// What a session event means to the loop: a prompt echo starts a turn,
/// visible assistant prose is read out.
fn signal_of(event: &AgentEvent) -> Option<Signal> {
    match event {
        AgentEvent::TurnEnd { .. } => Some(Signal::TurnEnd),
        AgentEvent::Text { content, kind: Some(kind), .. }
            if kind == "turn_annotation" && content.starts_with(TURN_DURATION) =>
        {
            Some(Signal::TurnEnd)
        }
        AgentEvent::Text { content, kind: None, .. } if content.starts_with(USER_PREFIX) => {
            let prompt = content.strip_prefix(USER_PREFIX).unwrap_or_default();
            Some(Signal::TurnStart(prompt.trim().to_owned()))
        }
        AgentEvent::Text { content, meta: false, kind: None, .. } => {
            Some(Signal::Text(content.clone()))
        }
        _ => None,
    }
}

fn voice_signal(event: VoiceEvent, session_id: &str) -> Option<Signal> {
    match event {
        VoiceEvent::Request { session_id: sid, request_id, user_text } if sid == session_id => {
            Some(Signal::Request { id: request_id, user_text })
        }
        VoiceEvent::Delta { session_id: sid, item_id, delta } if sid == session_id => {
            Some(Signal::Delta { item: item_id, text: delta })
        }
        _ => None,
    }
}

/// Turns the session's bus traffic into [`Signal`]s for the loop.
///
/// Holds the text watch for the socket's lifetime so the daemon streams
/// deltas, and the cluster-wide voice watch so the gateway does, and
/// releases both when `cancel` fires.
async fn relay_signals(
    state: AppState,
    session_id: String,
    signals: mpsc::Sender<Signal>,
    cancel: CancellationToken,
) {
    let mut server_rx = state.bus.subscribe_server();
    let mut voice_rx = state.bus.subscribe_voice();
    if state.bus.text_watch_inc(&session_id) {
        crate::ws::set_daemon_text_watch(&state, &session_id, true).await;
    }
    let watcher = uuid::Uuid::new_v4();
    let beacon = |on| VoiceEvent::Watch { session_id: session_id.clone(), watcher, on };
    let mut refresh = tokio::time::interval(VOICE_WATCH_REFRESH);
    loop {
        let signal = tokio::select! {
            () = cancel.cancelled() => break,
            _ = refresh.tick() => {
                state.bus.publish_voice(beacon(true));
                None
            }
            frame = voice_rx.recv() => match frame {
                Ok(event) => voice_signal(event, &session_id),
                Err(RecvError::Lagged(_)) => None,
                Err(RecvError::Closed) => break,
            },
            frame = server_rx.recv() => match frame {
                Ok(frame) => match &*frame.event {
                    ServerEvent::TextDelta { session_id: sid, item_id, delta }
                        if *sid == session_id =>
                    {
                        Some(Signal::Delta { item: item_id.clone(), text: delta.clone() })
                    }
                    ServerEvent::Stream { session_id: sid, data } if *sid == session_id => {
                        signal_of(data)
                    }
                    _ => None,
                },
                Err(RecvError::Lagged(_)) => None,
                Err(RecvError::Closed) => break,
            },
        };
        if let Some(signal) = signal
            && signals.send(signal).await.is_err()
        {
            break;
        }
    }
    state.bus.publish_voice(beacon(false));
    if state.bus.text_watch_dec(&session_id) {
        crate::ws::set_daemon_text_watch(&state, &session_id, false).await;
    }
}

async fn handle_voice_ws(
    socket: WebSocket,
    state: AppState,
    ctx: AuthContext,
    session_id: String,
    language: Option<String>,
) {
    let (sink, stream) = socket.split();
    let (out_tx, out_rx) = mpsc::channel::<Outbound>(256);
    let writer = spawn_writer(sink, out_rx);

    let client = match crate::routes::server_settings::speech_client(&state).await {
        Ok(c) => c,
        Err(err) => {
            let message = err.to_string();
            tracing::debug!(%session_id, %message, "voice_ws: speech unavailable");
            let _ = out_tx.send(Outbound::Control(VoiceServerMsg::Error { message })).await;
            drop(out_tx);
            let _ = writer.await;
            return;
        }
    };

    let (in_tx, in_rx) = mpsc::channel(16);
    let (sig_tx, sig_rx) = mpsc::channel(256);
    let reader = tokio::spawn(read_socket(stream, in_tx));
    let cancel = CancellationToken::new();
    let relay =
        tokio::spawn(relay_signals(state.clone(), session_id.clone(), sig_tx, cancel.clone()));
    let backend = Arc::new(ServerBackend {
        state,
        session_id: session_id.clone(),
        user_id: ctx.user_id,
        client,
    });

    let prefs = Prefs { barge_in: true, language, ..Prefs::default() };
    run_voice_loop(backend, prefs, in_rx, sig_rx, out_tx)
        .instrument(tracing::info_span!("voice", session = %session_id))
        .await;

    reader.abort();
    cancel.cancel();
    let _ = relay.await;
    writer.abort();
    tracing::debug!(%session_id, "voice WebSocket disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ctx(scopes: &[Scope], machine: bool) -> AuthContext {
        AuthContext {
            user_id: uuid::Uuid::new_v4(),
            key_id: uuid::Uuid::new_v4(),
            machine_id: machine.then(uuid::Uuid::new_v4),
            scopes: scopes.iter().copied().collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn only_human_keys_with_read_may_open_the_socket() {
        assert_eq!(principal_permitted(&ctx(&[Scope::Read], false)), Ok(()));
        assert_eq!(principal_permitted(&ctx(&[Scope::Admin, Scope::Read], false)), Ok(()));
        assert_eq!(
            principal_permitted(&ctx(&[Scope::Dispatch], false)),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(principal_permitted(&ctx(&[Scope::Read], true)), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn only_visible_assistant_prose_is_spoken_and_a_prompt_starts_a_turn() {
        let event = |content: &str, meta: bool, kind: Option<&str>| AgentEvent::Text {
            content: content.into(),
            meta,
            kind: kind.map(str::to_owned),
            operation: None,
            ts: 0,
            message_id: None,
            usage: None,
            seq: None,
            turn_id: None,
        };
        let text = |meta, kind| event("hi", meta, kind);
        assert_eq!(signal_of(&text(false, None)), Some(Signal::Text("hi".into())));
        assert_eq!(signal_of(&text(true, None)), None);
        assert_eq!(signal_of(&text(false, Some("thinking"))), None);
        assert_eq!(signal_of(&AgentEvent::TurnEnd { ts: 0, seq: None }), Some(Signal::TurnEnd));
        let annotation = |content: &str| event(content, true, Some("turn_annotation"));
        assert_eq!(signal_of(&annotation("turn_duration:5406")), Some(Signal::TurnEnd));
        assert_eq!(signal_of(&annotation("file_history:src/a.rs")), None);
        let prompt = "▷ User: <cctui-session-context voice=\"spoken\">\n</cctui-session-context>";
        let spoken = "<cctui-session-context voice=\"spoken\">\n</cctui-session-context>";
        assert_eq!(signal_of(&event(prompt, false, None)), Some(Signal::TurnStart(spoken.into())));
        let harness = event("▷ User: <task-notification>", true, None);
        assert_eq!(signal_of(&harness), Some(Signal::TurnStart("<task-notification>".into())));
        assert_eq!(signal_of(&event("▷ User: queued", true, Some("queue_op"))), None);
    }

    #[test]
    fn only_this_sessions_gateway_traffic_reaches_the_loop() {
        let delta = |sid: &str| VoiceEvent::Delta {
            session_id: sid.into(),
            item_id: "gw:r:0".into(),
            delta: "Hi".into(),
        };
        assert_eq!(
            voice_signal(delta("s"), "s"),
            Some(Signal::Delta { item: "gw:r:0".into(), text: "Hi".into() })
        );
        assert_eq!(voice_signal(delta("other"), "s"), None);
        let request = VoiceEvent::Request {
            session_id: "s".into(),
            request_id: "r".into(),
            user_text: "go".into(),
        };
        assert_eq!(
            voice_signal(request, "s"),
            Some(Signal::Request { id: "r".into(), user_text: "go".into() })
        );
        let watcher = uuid::Uuid::nil();
        let watch = VoiceEvent::Watch { session_id: "s".into(), watcher, on: true };
        assert_eq!(voice_signal(watch, "s"), None);
    }
}
