//! Client for the OpenAI-compatible speech service configured in the
//! `speech` instance setting.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, Stream, StreamExt, TryStreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, Message};

pub use cctui_proto::api::settings::SpeechConfig;

pub const MAX_TTS_INPUT_CHARS: usize = 4096;
/// Takes over from an English-first instance model when another language is
/// picked: parakeet drifts into English words on French speech.
pub const MULTILINGUAL_STT_MODEL: &str = "whisper";
const ENGLISH_FIRST_STT_MODELS: &[&str] = &["parakeet"];
/// Models the router transcribes live over its websocket.
const STREAMING_STT_MODELS: &[&str] = &["nemotron"];
/// Audio per websocket frame: 16 kHz mono PCM16, a third of a second.
const STREAM_FRAME_BYTES: usize = 10_240;
const STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUDIO_TIMEOUT: Duration = Duration::from_mins(1);
const META_TIMEOUT: Duration = Duration::from_secs(10);
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error("speech service is not configured")]
    NotConfigured,
    #[error("speech service URL {0}")]
    Url(crate::outbound::OutboundUrlError),
    #[error("text is longer than {MAX_TTS_INPUT_CHARS} characters")]
    InputTooLong,
    #[error("speech service unreachable: {0}")]
    Transport(String),
    #[error("speech service answered {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("unexpected speech service response: {0}")]
    Decode(String),
    #[error("speech stream closed ({code}): {detail}")]
    StreamClosed { code: u16, detail: String },
}

impl SpeechError {
    /// The service cannot stream this model or language at all, as opposed
    /// to being busy or unreachable for now.
    pub const fn refuses_streaming(&self) -> bool {
        match self {
            Self::StreamClosed { code, .. } => *code == 1008,
            Self::Upstream { status, .. } => *status >= 400 && *status < 500 && *status != 429,
            _ => false,
        }
    }
}

pub fn stt_streams(model: &str) -> bool {
    STREAMING_STT_MODELS.contains(&model)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEvent {
    /// Everything heard so far in the utterance.
    Partial(String),
    Final(String),
}

#[derive(Debug)]
pub enum SttInput {
    Audio(Bytes),
    End,
}

/// One live transcription: 16 kHz mono PCM16 in, partial and final text
/// out. After a final the same stream takes the next utterance. Dropping it
/// discards the utterance in flight.
#[derive(Debug)]
pub struct SttStream {
    input: mpsc::UnboundedSender<SttInput>,
    events: mpsc::UnboundedReceiver<Result<SttEvent, SpeechError>>,
    task: Option<tokio::task::AbortHandle>,
}

impl Drop for SttStream {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

type SttEvents = mpsc::UnboundedSender<Result<SttEvent, SpeechError>>;

impl SttStream {
    /// A stream fed by whatever holds the other ends.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<SttInput>, SttEvents) {
        let (input, input_rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::unbounded_channel();
        (Self { input, events, task: None }, input_rx, events_tx)
    }

    /// False once the stream is gone.
    pub fn send_audio(&self, pcm: &Bytes) -> bool {
        let mut at = 0;
        while at < pcm.len() {
            let end = (at + STREAM_FRAME_BYTES).min(pcm.len());
            if self.input.send(SttInput::Audio(pcm.slice(at..end))).is_err() {
                return false;
            }
            at = end;
        }
        true
    }

    pub fn end(&self) -> bool {
        self.input.send(SttInput::End).is_ok()
    }

    /// `None` once the stream is gone; a failure is reported once before.
    pub async fn event(&mut self) -> Option<Result<SttEvent, SpeechError>> {
        self.events.recv().await
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamFrame {
    Partial {
        text: String,
    },
    Final {
        text: String,
    },
    Error {
        #[serde(default)]
        detail: String,
    },
    #[serde(other)]
    Other,
}

type UpstreamWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn pump_stream(
    ws: UpstreamWs,
    mut input: mpsc::UnboundedReceiver<SttInput>,
    events: SttEvents,
) {
    let (mut sink, mut stream) = ws.split();
    let mut detail = String::new();
    loop {
        tokio::select! {
            cmd = input.recv() => {
                let msg = match cmd {
                    Some(SttInput::Audio(pcm)) => Message::Binary(pcm),
                    Some(SttInput::End) => Message::Text(r#"{"type":"end"}"#.into()),
                    None => {
                        let _ = sink.send(Message::Close(None)).await;
                        return;
                    }
                };
                if let Err(e) = sink.send(msg).await {
                    let _ = events.send(Err(SpeechError::Transport(e.to_string())));
                    return;
                }
            }
            msg = stream.next() => {
                let event = match msg {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                        Ok(StreamFrame::Partial { text }) => Ok(SttEvent::Partial(text)),
                        Ok(StreamFrame::Final { text }) => Ok(SttEvent::Final(text)),
                        Ok(StreamFrame::Error { detail: d }) => {
                            detail = d;
                            continue;
                        }
                        Ok(StreamFrame::Other) | Err(_) => continue,
                    },
                    Some(Ok(Message::Close(frame))) => {
                        let (code, reason) =
                            frame.map_or((1005, String::new()), |f| (u16::from(f.code), f.reason.to_string()));
                        let detail = if reason.is_empty() { std::mem::take(&mut detail) } else { reason };
                        let _ = events.send(Err(SpeechError::StreamClosed { code, detail }));
                        return;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => Err(SpeechError::Transport(e.to_string())),
                    None => Err(SpeechError::StreamClosed { code: 1006, detail: std::mem::take(&mut detail) }),
                };
                let failed = event.is_err();
                if events.send(event).is_err() || failed {
                    return;
                }
            }
        }
    }
}

impl From<reqwest::Error> for SpeechError {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e.without_url().to_string())
    }
}

pub struct SpeechClient {
    http: reqwest::Client,
    config: SpeechConfig,
    api_key: Option<String>,
    /// Websocket connections resolve through the SSRF guard, as
    /// [`crate::outbound::upstream_client`] does.
    guarded: bool,
}

impl std::fmt::Debug for SpeechClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechClient")
            .field("base_url", &self.config.base_url)
            .field("has_key", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum VoiceEntry {
    Name(String),
    Object {
        #[serde(alias = "voice_id", alias = "name")]
        id: String,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum VoicesBody {
    List(Vec<VoiceEntry>),
    Wrapped {
        #[serde(alias = "data")]
        voices: Vec<VoiceEntry>,
    },
}

#[derive(Deserialize)]
struct ModelsBody {
    data: Vec<ModelEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    /// The router tags its models `stt`, `tts` or `text` here.
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Deserialize)]
struct ChatBody {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

const SUMMARY_PROMPT: &str = "Summarize the message for text-to-speech in at most three short \
    spoken sentences, in the message's language. Plain prose: no markdown, no lists, no code, \
    no URLs. Reply with the summary only.";

#[derive(Deserialize)]
struct TranscriptBody {
    text: String,
}

fn extension(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or_default().trim() {
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/flac" => "flac",
        "audio/mp4" | "audio/m4a" | "audio/aac" => "m4a",
        _ => "webm",
    }
}

impl SpeechClient {
    pub const fn new(http: reqwest::Client, config: SpeechConfig, api_key: Option<String>) -> Self {
        Self { http, config, api_key, guarded: false }
    }

    /// The client for the saved setting, through the SSRF-guarded upstream client.
    pub fn for_upstream(
        config: SpeechConfig,
        api_key: Option<String>,
    ) -> Result<Self, SpeechError> {
        if !config.enabled || config.base_url.trim().is_empty() {
            return Err(SpeechError::NotConfigured);
        }
        crate::outbound::upstream_url_permitted(&config.base_url).map_err(SpeechError::Url)?;
        let mut client = Self::new(crate::outbound::upstream_client().clone(), config, api_key);
        client.guarded = true;
        Ok(client)
    }

    pub const fn config(&self) -> &SpeechConfig {
        &self.config
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}/{path}", self.config.base_url.trim_end_matches('/'));
        let req = self.http.request(method, url);
        match &self.api_key {
            Some(k) => req.bearer_auth(k),
            None => req,
        }
    }

    async fn send(req: reqwest::RequestBuilder) -> Result<reqwest::Response, SpeechError> {
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(SpeechError::Upstream {
            status: status.as_u16(),
            body: body.chars().take(300).collect(),
        })
    }

    /// The model and language a transcription uses. An explicit `model` wins;
    /// a picked non-English `language` moves an English-first instance model
    /// to [`MULTILINGUAL_STT_MODEL`]; with neither, the instance setting.
    pub fn stt_route<'a>(
        &'a self,
        language: Option<&'a str>,
        model: Option<&'a str>,
    ) -> (&'a str, Option<&'a str>) {
        let instance = self.config.stt_model.as_str();
        let english = |l: &str| {
            l.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("en"))
                && matches!(l.as_bytes().get(2), None | Some(b'-' | b'_'))
        };
        let reroute =
            language.is_some_and(|l| !english(l)) && ENGLISH_FIRST_STT_MODELS.contains(&instance);
        let model = model.unwrap_or(if reroute { MULTILINGUAL_STT_MODEL } else { instance });
        (model, language.or(self.config.stt_language.as_deref()))
    }

    pub async fn transcribe(
        &self,
        audio: Bytes,
        mime: &str,
        language: Option<&str>,
        model: Option<&str>,
    ) -> Result<String, SpeechError> {
        let (model, language) = self.stt_route(language, model);
        let part = reqwest::multipart::Part::stream(audio)
            .file_name(format!("audio.{}", extension(mime)))
            .mime_str(mime)
            .map_err(|_| SpeechError::Decode(format!("invalid audio type `{mime}`")))?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", model.to_owned())
            .text("response_format", "json");
        if let Some(lang) = language {
            form = form.text("language", lang.to_owned());
        }
        let req = self
            .request(reqwest::Method::POST, "audio/transcriptions")
            .timeout(AUDIO_TIMEOUT)
            .multipart(form);
        let body: TranscriptBody = Self::send(req)
            .await?
            .json()
            .await
            .map_err(|e| SpeechError::Decode(e.without_url().to_string()))?;
        Ok(body.text)
    }

    /// Whether `language` routes to a model that streams.
    pub fn stt_streams(&self, language: Option<&str>) -> bool {
        stt_streams(self.stt_route(language, None).0)
    }

    /// A live transcription session with the model `language` routes to.
    pub async fn stream_transcription(
        &self,
        language: Option<&str>,
    ) -> Result<SttStream, SpeechError> {
        use tungstenite::client::IntoClientRequest;

        let (model, language) = self.stt_route(language, None);
        let base = self.config.base_url.trim_end_matches('/');
        let mut url = reqwest::Url::parse(&format!("{base}/audio/transcriptions/stream"))
            .map_err(|_| SpeechError::Url(crate::outbound::OutboundUrlError::Malformed))?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|()| SpeechError::Url(crate::outbound::OutboundUrlError::Malformed))?;
        url.query_pairs_mut().append_pair("model", model);
        if let Some(lang) = language {
            url.query_pairs_mut().append_pair("language", lang);
        }
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|_| SpeechError::Url(crate::outbound::OutboundUrlError::Malformed))?;
        if let Some(key) = &self.api_key {
            let value = format!("Bearer {key}")
                .parse()
                .map_err(|_| SpeechError::Decode("invalid API key".into()))?;
            req.headers_mut().insert(reqwest::header::AUTHORIZATION, value);
        }
        let connect = async {
            if !self.guarded {
                return tokio_tungstenite::connect_async(req).await;
            }
            let host = url.host_str().unwrap_or_default();
            let port = url.port_or_known_default().unwrap_or(443);
            let allow = crate::outbound::upstream_allowlist();
            let addrs = crate::outbound::permitted_addrs(host, port, &allow).await?;
            let tcp = tokio::net::TcpStream::connect(&*addrs).await?;
            tokio_tungstenite::client_async_tls(req, tcp).await
        };
        let (ws, _) = tokio::time::timeout(STREAM_CONNECT_TIMEOUT, connect)
            .await
            .map_err(|_| SpeechError::Transport("speech stream connect timed out".into()))?
            .map_err(|e| match e {
                tungstenite::Error::Http(resp) => SpeechError::Upstream {
                    status: resp.status().as_u16(),
                    body: resp
                        .body()
                        .as_deref()
                        .map(|b| String::from_utf8_lossy(b).chars().take(300).collect())
                        .unwrap_or_default(),
                },
                other => SpeechError::Transport(other.to_string()),
            })?;
        let (mut stream, input, events) = SttStream::channel();
        stream.task = Some(tokio::spawn(pump_stream(ws, input, events)).abort_handle());
        Ok(stream)
    }

    fn speech_request(
        &self,
        text: &str,
        voice: Option<&str>,
        format: Option<&str>,
        speed: Option<f32>,
    ) -> Result<reqwest::RequestBuilder, SpeechError> {
        if text.chars().count() > MAX_TTS_INPUT_CHARS {
            return Err(SpeechError::InputTooLong);
        }
        let mut body = serde_json::json!({
            "model": self.config.tts_model,
            "input": text,
            "voice": voice.unwrap_or(&self.config.tts_voice),
            "response_format": format.unwrap_or(&self.config.tts_format),
        });
        if let Some(s) = speed {
            body["speed"] = s.into();
        }
        Ok(self.request(reqwest::Method::POST, "audio/speech").timeout(AUDIO_TIMEOUT).json(&body))
    }

    pub async fn synthesize(
        &self,
        text: &str,
        voice: Option<&str>,
        format: Option<&str>,
        speed: Option<f32>,
    ) -> Result<Bytes, SpeechError> {
        let req = self.speech_request(text, voice, format, speed)?;
        Ok(Self::send(req).await?.bytes().await?)
    }

    pub async fn synthesize_stream(
        &self,
        text: &str,
        voice: Option<&str>,
        format: Option<&str>,
        speed: Option<f32>,
    ) -> Result<impl Stream<Item = Result<Bytes, SpeechError>> + use<>, SpeechError> {
        let req = self.speech_request(text, voice, format, speed)?;
        Ok(Self::send(req).await?.bytes_stream().map_err(SpeechError::from).boxed())
    }

    pub async fn voices(&self) -> Result<Vec<String>, SpeechError> {
        let req = self.request(reqwest::Method::GET, "audio/voices").timeout(META_TIMEOUT);
        let body: VoicesBody = Self::send(req)
            .await?
            .json()
            .await
            .map_err(|e| SpeechError::Decode(e.without_url().to_string()))?;
        let (VoicesBody::List(v) | VoicesBody::Wrapped { voices: v }) = body;
        Ok(v.into_iter()
            .map(|e| match e {
                VoiceEntry::Name(n) | VoiceEntry::Object { id: n } => n,
            })
            .collect())
    }

    /// The models the service serves; doubles as the health check.
    pub async fn models(&self) -> Result<Vec<ModelEntry>, SpeechError> {
        let req = self.request(reqwest::Method::GET, "models").timeout(META_TIMEOUT);
        let body: ModelsBody = Self::send(req)
            .await?
            .json()
            .await
            .map_err(|e| SpeechError::Decode(e.without_url().to_string()))?;
        Ok(body.data)
    }

    pub async fn health(&self) -> Result<Vec<String>, SpeechError> {
        Ok(self.models().await?.into_iter().map(|m| m.id).collect())
    }

    pub const fn can_summarize(&self) -> bool {
        !self.config.summary_model.is_empty()
    }

    /// A few spoken sentences standing in for `text`, from the summary model.
    pub async fn summarize(&self, text: &str) -> Result<String, SpeechError> {
        if !self.can_summarize() {
            return Err(SpeechError::NotConfigured);
        }
        let body = serde_json::json!({
            "model": self.config.summary_model,
            "temperature": 0.3,
            "max_tokens": 300,
            "chat_template_kwargs": { "enable_thinking": false },
            "messages": [
                { "role": "system", "content": SUMMARY_PROMPT },
                { "role": "user", "content": text },
            ],
        });
        let req = self
            .request(reqwest::Method::POST, "chat/completions")
            .timeout(SUMMARY_TIMEOUT)
            .json(&body);
        let body: ChatBody = Self::send(req)
            .await?
            .json()
            .await
            .map_err(|e| SpeechError::Decode(e.without_url().to_string()))?;
        let summary = body
            .choices
            .into_iter()
            .next()
            .map(|c| {
                let content = c.message.content;
                let answer = content.rsplit_once("</think>").map_or(content.as_str(), |(_, a)| a);
                answer.trim().to_owned()
            })
            .unwrap_or_default();
        if summary.is_empty() {
            return Err(SpeechError::Decode("empty summary".into()));
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Multipart, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Seen {
        auth: Vec<Option<String>>,
        fields: Vec<(String, String)>,
        speech: Option<serde_json::Value>,
        chat: Option<serde_json::Value>,
    }
    type Shared = Arc<Mutex<Seen>>;

    fn auth(seen: &Shared, h: &HeaderMap) {
        let v = h.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
        seen.lock().unwrap().auth.push(v);
    }

    async fn mock(voices: serde_json::Value) -> (String, Shared) {
        let seen: Shared = Arc::default();
        let app = Router::new()
            .route(
                "/v1/audio/transcriptions",
                post(|State(s): State<Shared>, h: HeaderMap, mut mp: Multipart| async move {
                    auth(&s, &h);
                    while let Some(f) = mp.next_field().await.unwrap() {
                        let name = f.name().unwrap().to_owned();
                        let value = match f.file_name() {
                            Some(file) => format!("{file}|{}", f.content_type().unwrap()),
                            None => f.text().await.unwrap(),
                        };
                        s.lock().unwrap().fields.push((name, value));
                    }
                    Json(serde_json::json!({ "text": "hello there" }))
                }),
            )
            .route(
                "/v1/audio/speech",
                post(|State(s): State<Shared>, h: HeaderMap, Json(b): Json<serde_json::Value>| async move {
                    auth(&s, &h);
                    s.lock().unwrap().speech = Some(b);
                    vec![1u8, 2, 3, 4]
                }),
            )
            .route("/v1/audio/voices", get(move || async move { Json(voices) }))
            .route(
                "/v1/chat/completions",
                post(|State(s): State<Shared>, Json(b): Json<serde_json::Value>| async move {
                    s.lock().unwrap().chat = Some(b);
                    Json(serde_json::json!({
                        "choices": [{ "message": { "content": "<think>\n</think>\n\n Short. " } }]
                    }))
                }),
            )
            .route(
                "/v1/models",
                get(|| async {
                    Json(serde_json::json!({ "data": [
                        { "id": "kokoro", "owned_by": "tts" },
                        { "id": "parakeet", "owned_by": "stt" }
                    ] }))
                }),
            )
            .route(
                "/broken/models",
                get(|| async { (StatusCode::UNAUTHORIZED, "bad key") }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    fn http() -> reqwest::Client {
        crate::install_crypto_provider();
        reqwest::Client::new()
    }

    fn client(base: &str, key: Option<&str>) -> SpeechClient {
        let config = SpeechConfig {
            enabled: true,
            base_url: format!("{base}/v1/"),
            stt_language: Some("en".into()),
            ..SpeechConfig::default()
        };
        SpeechClient::new(http(), config, key.map(str::to_owned))
    }

    #[tokio::test]
    async fn transcribe_posts_multipart_with_model_language_and_key() {
        let (base, seen) = mock(serde_json::json!([])).await;
        let c = client(&base, Some("sk-secret"));
        let text =
            c.transcribe(Bytes::from_static(b"OggS"), "audio/ogg", None, None).await.unwrap();
        assert_eq!(text, "hello there");
        let (auth, fields) = {
            let mut s = seen.lock().unwrap();
            (s.auth.clone(), std::mem::take(&mut s.fields))
        };
        assert_eq!(auth, vec![Some("Bearer sk-secret".to_owned())]);
        let field = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        assert_eq!(field("file"), Some("audio.ogg|audio/ogg"));
        assert_eq!(field("model"), Some("parakeet"));
        assert_eq!(field("language"), Some("en"));
        assert_eq!(field("response_format"), Some("json"));
        c.transcribe(Bytes::from_static(b"x"), "audio/webm", Some("fr"), Some("custom"))
            .await
            .unwrap();
        let fields = std::mem::take(&mut seen.lock().unwrap().fields);
        assert!(fields.contains(&("language".into(), "fr".into())));
        assert!(fields.contains(&("model".into(), "custom".into())));
    }

    #[tokio::test]
    async fn a_picked_language_routes_between_parakeet_and_whisper() {
        let (base, seen) = mock(serde_json::json!([])).await;
        let mut c = client(&base, None);
        c.config.stt_language = None;
        let c = &c;
        let model_of = move |language: Option<&'static str>| {
            let seen = seen.clone();
            async move {
                c.transcribe(Bytes::from_static(b"x"), "audio/wav", language, None).await.unwrap();
                let fields = std::mem::take(&mut seen.lock().unwrap().fields);
                let field = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
                (field("model").unwrap(), field("language"))
            }
        };
        assert_eq!(model_of(None).await, ("parakeet".into(), None));
        assert_eq!(model_of(Some("en")).await, ("parakeet".into(), Some("en".into())));
        assert_eq!(model_of(Some("en-GB")).await, ("parakeet".into(), Some("en-GB".into())));
        assert_eq!(model_of(Some("fr")).await, ("whisper".into(), Some("fr".into())));
        assert_eq!(model_of(Some("de")).await, ("whisper".into(), Some("de".into())));
    }

    #[test]
    fn only_an_english_first_instance_model_is_rerouted() {
        let c = |stt_model: &str, stt_language: Option<&str>| {
            let config = SpeechConfig {
                stt_model: stt_model.into(),
                stt_language: stt_language.map(str::to_owned),
                ..SpeechConfig::default()
            };
            SpeechClient::new(http(), config, None)
        };
        let gpt = c("gpt-4o-transcribe", None);
        assert_eq!(gpt.stt_route(Some("fr"), None), ("gpt-4o-transcribe", Some("fr")));
        let fr_default = c("parakeet", Some("fr"));
        assert_eq!(fr_default.stt_route(None, None), ("parakeet", Some("fr")));
        assert_eq!(fr_default.stt_route(Some("fr"), None), ("whisper", Some("fr")));
        assert_eq!(fr_default.stt_route(Some("fr"), Some("parakeet")), ("parakeet", Some("fr")));
    }

    #[tokio::test]
    async fn synthesize_sends_defaults_and_overrides() {
        let (base, seen) = mock(serde_json::json!([])).await;
        let c = client(&base, None);
        let audio = c.synthesize("hi", None, None, None).await.unwrap();
        assert_eq!(&audio[..], &[1, 2, 3, 4]);
        let body = seen.lock().unwrap().speech.take().unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "model": "kokoro", "input": "hi", "voice": "af_heart", "response_format": "opus"
            })
        );
        assert_eq!(seen.lock().unwrap().auth, vec![None]);

        let chunks: Vec<Bytes> = c
            .synthesize_stream("yo", Some("bf_emma"), Some("mp3"), Some(1.25))
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(chunks.concat(), vec![1, 2, 3, 4]);
        let body = seen.lock().unwrap().speech.take().unwrap();
        assert_eq!(body["voice"], "bf_emma");
        assert_eq!(body["response_format"], "mp3");
        assert_eq!(body["speed"], 1.25);
    }

    #[tokio::test]
    async fn synthesize_refuses_overlong_input_without_calling_out() {
        let (base, seen) = mock(serde_json::json!([])).await;
        let c = client(&base, None);
        let long = "a".repeat(MAX_TTS_INPUT_CHARS + 1);
        assert!(matches!(
            c.synthesize(&long, None, None, None).await,
            Err(SpeechError::InputTooLong)
        ));
        assert!(seen.lock().unwrap().speech.is_none());
        c.synthesize(&"é".repeat(MAX_TTS_INPUT_CHARS), None, None, None).await.unwrap();
    }

    #[tokio::test]
    async fn voices_accept_plain_and_wrapped_lists() {
        for body in [
            serde_json::json!(["af_heart", "bf_emma"]),
            serde_json::json!({ "voices": ["af_heart", "bf_emma"] }),
            serde_json::json!({ "voices": [{ "id": "af_heart" }, { "name": "bf_emma" }] }),
        ] {
            let (base, _) = mock(body.clone()).await;
            assert_eq!(
                client(&base, None).voices().await.unwrap(),
                vec!["af_heart", "bf_emma"],
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn health_lists_models_and_upstream_errors_never_echo_the_key() {
        let (base, _) = mock(serde_json::json!([])).await;
        assert_eq!(client(&base, None).health().await.unwrap(), vec!["kokoro", "parakeet"]);

        let broken = SpeechClient::new(
            http(),
            SpeechConfig { base_url: format!("{base}/broken"), ..SpeechConfig::default() },
            Some("sk-secret".into()),
        );
        let err = broken.health().await.unwrap_err();
        assert!(matches!(err, SpeechError::Upstream { status: 401, .. }), "{err}");
        assert!(!format!("{err} {broken:?}").contains("sk-secret"));

        let dead = client("http://127.0.0.1:1", Some("sk-secret"));
        let err = dead.health().await.unwrap_err();
        assert!(matches!(err, SpeechError::Transport(_)), "{err}");
        assert!(!err.to_string().contains("sk-secret"));
    }

    #[tokio::test]
    async fn models_keep_the_router_role_tags() {
        let (base, _) = mock(serde_json::json!([])).await;
        let models = client(&base, None).models().await.unwrap();
        assert_eq!(models[1], ModelEntry { id: "parakeet".into(), owned_by: Some("stt".into()) });
    }

    #[tokio::test]
    async fn summarize_asks_the_summary_model_without_thinking() {
        let (base, seen) = mock(serde_json::json!([])).await;
        let mut c = client(&base, None);
        assert!(matches!(c.summarize("long").await, Err(SpeechError::NotConfigured)));
        c.config.summary_model = "qwen3-1.7b".into();
        assert_eq!(c.summarize("a long message").await.unwrap(), "Short.");
        let body = seen.lock().unwrap().chat.take().unwrap();
        assert_eq!(body["model"], "qwen3-1.7b");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["messages"][1]["content"], "a long message");
    }

    #[derive(Default)]
    struct StreamSeen {
        auth: Option<String>,
        query: Option<String>,
        largest_frame: usize,
        closed: bool,
    }
    type StreamShared = Arc<Mutex<StreamSeen>>;

    /// The router's streaming endpoint: a partial after each frame with the
    /// bytes heard so far, a final at `end`; language `xx` is refused.
    async fn stream_mock() -> (String, StreamShared) {
        use axum::extract::RawQuery;
        use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocketUpgrade};

        let seen: StreamShared = Arc::default();
        let app = Router::new()
            .route(
                "/v1/audio/transcriptions/stream",
                get(|State(s): State<StreamShared>,
                     h: HeaderMap,
                     RawQuery(q): RawQuery,
                     ws: WebSocketUpgrade| async move {
                    {
                        let mut seen = s.lock().unwrap();
                        seen.auth =
                            h.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
                        seen.query.clone_from(&q);
                    }
                    ws.on_upgrade(move |mut socket| async move {
                        if q.unwrap_or_default().contains("language=xx") {
                            let error = r#"{"type":"error","detail":"unsupported language"}"#;
                            let _ = socket.send(WsMessage::Text(error.into())).await;
                            let frame = CloseFrame { code: 1008, reason: "".into() };
                            let _ = socket.send(WsMessage::Close(Some(frame))).await;
                            return;
                        }
                        let mut heard = 0;
                        while let Some(Ok(msg)) = socket.recv().await {
                            let reply = match msg {
                                WsMessage::Binary(b) => {
                                    heard += b.len();
                                    let mut seen = s.lock().unwrap();
                                    seen.largest_frame = seen.largest_frame.max(b.len());
                                    drop(seen);
                                    serde_json::json!({ "type": "partial", "text": format!("{heard} bytes") })
                                }
                                WsMessage::Text(t) if t.contains("end") => {
                                    let text = format!("heard {heard} bytes");
                                    heard = 0;
                                    serde_json::json!({ "type": "final", "text": text, "language": "en" })
                                }
                                WsMessage::Close(_) => break,
                                _ => continue,
                            };
                            if socket.send(WsMessage::Text(reply.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                        s.lock().unwrap().closed = true;
                    })
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    async fn next_event(s: &mut SttStream) -> Result<SttEvent, SpeechError> {
        tokio::time::timeout(Duration::from_secs(2), s.event()).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn a_stream_relays_partials_and_a_final_per_utterance() {
        let (base, seen) = stream_mock().await;
        let mut c = client(&base, Some("sk-secret"));
        c.config.stt_model = "nemotron".into();
        let mut s = c.stream_transcription(None).await.unwrap();
        let (auth, query) = {
            let seen = seen.lock().unwrap();
            (seen.auth.clone(), seen.query.clone())
        };
        assert_eq!(auth.as_deref(), Some("Bearer sk-secret"));
        assert_eq!(query.as_deref(), Some("model=nemotron&language=en"));
        assert!(s.send_audio(&Bytes::from(vec![0u8; 4000])));
        assert_eq!(next_event(&mut s).await.unwrap(), SttEvent::Partial("4000 bytes".into()));
        assert!(s.send_audio(&Bytes::from(vec![0u8; 3 * STREAM_FRAME_BYTES])));
        for n in 1..=3 {
            let partial = format!("{} bytes", 4000 + n * STREAM_FRAME_BYTES);
            assert_eq!(next_event(&mut s).await.unwrap(), SttEvent::Partial(partial));
        }
        assert!(s.end());
        let heard = format!("heard {} bytes", 4000 + 3 * STREAM_FRAME_BYTES);
        assert_eq!(next_event(&mut s).await.unwrap(), SttEvent::Final(heard));
        assert_eq!(seen.lock().unwrap().largest_frame, STREAM_FRAME_BYTES);

        assert!(s.send_audio(&Bytes::from_static(&[0, 0])));
        assert_eq!(next_event(&mut s).await.unwrap(), SttEvent::Partial("2 bytes".into()));
        assert!(s.end());
        assert_eq!(next_event(&mut s).await.unwrap(), SttEvent::Final("heard 2 bytes".into()));
        drop(s);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !seen.lock().unwrap().closed {
            assert!(tokio::time::Instant::now() < deadline, "stream left open");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_refused_stream_reports_its_close_code_and_detail() {
        let (base, _) = stream_mock().await;
        let mut c = client(&base, None);
        c.config.stt_model = "nemotron".into();
        let mut s = c.stream_transcription(Some("xx")).await.unwrap();
        let err = next_event(&mut s).await.unwrap_err();
        assert!(
            matches!(&err, SpeechError::StreamClosed { code: 1008, detail } if detail == "unsupported language"),
            "{err}"
        );
        assert!(err.refuses_streaming());
        assert!(s.event().await.is_none());
        let busy = SpeechError::StreamClosed { code: 1013, detail: "loading".into() };
        assert!(!busy.refuses_streaming());
    }

    #[tokio::test]
    async fn a_missing_stream_endpoint_is_an_upstream_refusal() {
        let (base, _) = mock(serde_json::json!([])).await;
        let mut c = client(&base, None);
        c.config.stt_model = "nemotron".into();
        let err = c.stream_transcription(None).await.unwrap_err();
        assert!(matches!(err, SpeechError::Upstream { status: 404, .. }), "{err}");
        assert!(err.refuses_streaming());
        let dead = client("http://127.0.0.1:1", None);
        let err = dead.stream_transcription(None).await.unwrap_err();
        assert!(matches!(err, SpeechError::Transport(_)), "{err}");
        assert!(!err.refuses_streaming());
    }

    #[test]
    fn only_a_streaming_model_after_routing_streams() {
        let c = |stt_model: &str| {
            let config = SpeechConfig { stt_model: stt_model.into(), ..SpeechConfig::default() };
            SpeechClient::new(http(), config, None)
        };
        assert!(c("nemotron").stt_streams(None));
        assert!(c("nemotron").stt_streams(Some("en")));
        assert!(!c("parakeet").stt_streams(None));
        assert!(!c("whisper").stt_streams(Some("fr")));
    }

    #[test]
    fn for_upstream_requires_an_enabled_permitted_url() {
        let off = SpeechConfig {
            base_url: "https://speech.example/v1".into(),
            ..SpeechConfig::default()
        };
        assert!(matches!(SpeechClient::for_upstream(off, None), Err(SpeechError::NotConfigured)));
        let internal = SpeechConfig {
            enabled: true,
            base_url: "http://speech-for-upstream-probe.internal:8000/v1".into(),
            ..SpeechConfig::default()
        };
        assert!(matches!(SpeechClient::for_upstream(internal, None), Err(SpeechError::Url(_))));
    }
}
