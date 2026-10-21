//! `/api/v1/ws/voice?session=<id>`: a hands-free voice conversation with one
//! session.
//!
//! Binary frames carry audio both ways (16 kHz mono PCM up, synthesized
//! speech down); text frames carry [`VoiceClientMsg`] / [`VoiceServerMsg`]
//! JSON.

mod engine;
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

use crate::auth::{AuthContext, Scope};
use crate::speech::SpeechClient;
use crate::state::AppState;
pub use engine::{AudioStream, Inbound, Outbound, Prefs, Signal, VoiceBackend, run_voice_loop};
use text::{UTTERANCE_SAMPLE_RATE, pcm16_to_wav};

/// Longest utterance accepted (one minute of 16 kHz PCM16).
const MAX_UTTERANCE_BYTES: usize = 2 * 16_000 * 60;
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(20);
/// How the session stream renders a prompt the agent received.
const USER_PREFIX: &str = "▷ User:";

#[derive(Debug, Deserialize)]
pub struct VoiceQuery {
    pub session: String,
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
    Ok(ws.on_upgrade(move |socket| handle_voice_ws(socket, state, ctx, session_id)))
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
    async fn transcribe(&self, audio: Bytes) -> Result<String, String> {
        let wav = Bytes::from(pcm16_to_wav(&audio, UTTERANCE_SAMPLE_RATE));
        self.client.transcribe(wav, "audio/wav", None).await.map_err(|e| e.to_string())
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
    while let Some(Ok(msg)) = stream.next().await {
        let item = match msg {
            Message::Binary(bytes) if bytes.len() <= MAX_UTTERANCE_BYTES && bytes.len() >= 2 => {
                Inbound::Audio(bytes)
            }
            Message::Binary(bytes) if bytes.len() > MAX_UTTERANCE_BYTES => Inbound::TooLong,
            Message::Text(text) => match serde_json::from_str::<VoiceClientMsg>(&text) {
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
        AgentEvent::Text { content, kind: None, .. } if content.starts_with(USER_PREFIX) => {
            Some(Signal::TurnStart)
        }
        AgentEvent::Text { content, meta: false, kind: None, .. } => {
            Some(Signal::Text(content.clone()))
        }
        _ => None,
    }
}

/// Turns the session's bus traffic into [`Signal`]s for the loop.
///
/// Holds the text watch for the socket's lifetime so the daemon streams
/// deltas, and releases it when `cancel` fires.
async fn relay_signals(
    state: AppState,
    session_id: String,
    signals: mpsc::Sender<Signal>,
    cancel: CancellationToken,
) {
    let mut server_rx = state.bus.subscribe_server();
    if state.bus.text_watch_inc(&session_id) {
        crate::ws::set_daemon_text_watch(&state, &session_id, true).await;
    }
    loop {
        let signal = tokio::select! {
            () = cancel.cancelled() => break,
            frame = server_rx.recv() => match frame {
                Ok(frame) => match &*frame.event {
                    ServerEvent::TextDelta { session_id: sid, delta, .. } if *sid == session_id => {
                        Some(Signal::Delta(delta.clone()))
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
    if state.bus.text_watch_dec(&session_id) {
        crate::ws::set_daemon_text_watch(&state, &session_id, false).await;
    }
}

async fn handle_voice_ws(socket: WebSocket, state: AppState, ctx: AuthContext, session_id: String) {
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

    run_voice_loop(backend, Prefs { barge_in: true, ..Prefs::default() }, in_rx, sig_rx, out_tx)
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
        let prompt = "▷ User: <cctui-session-context voice=\"spoken\">\n</cctui-session-context>";
        assert_eq!(signal_of(&event(prompt, false, None)), Some(Signal::TurnStart));
        let harness = event("▷ User: <task-notification>", true, None);
        assert_eq!(signal_of(&harness), Some(Signal::TurnStart));
        assert_eq!(signal_of(&event("▷ User: queued", true, Some("queue_op"))), None);
    }
}
