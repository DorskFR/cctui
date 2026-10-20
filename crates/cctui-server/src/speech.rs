//! Client for the OpenAI-compatible speech service configured in the
//! `speech` instance setting.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use serde::Deserialize;

pub use cctui_proto::api::settings::SpeechConfig;

pub const MAX_TTS_INPUT_CHARS: usize = 4096;
const AUDIO_TIMEOUT: Duration = Duration::from_mins(1);
const META_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

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
        Self { http, config, api_key }
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
        Ok(Self::new(crate::outbound::upstream_client().clone(), config, api_key))
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

    pub async fn transcribe(
        &self,
        audio: Bytes,
        mime: &str,
        language: Option<&str>,
    ) -> Result<String, SpeechError> {
        let part = reqwest::multipart::Part::stream(audio)
            .file_name(format!("audio.{}", extension(mime)))
            .mime_str(mime)
            .map_err(|_| SpeechError::Decode(format!("invalid audio type `{mime}`")))?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.config.stt_model.clone())
            .text("response_format", "json");
        if let Some(lang) = language.or(self.config.stt_language.as_deref()) {
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

    /// The model ids the service serves; doubles as the health check.
    pub async fn health(&self) -> Result<Vec<String>, SpeechError> {
        let req = self.request(reqwest::Method::GET, "models").timeout(META_TIMEOUT);
        let body: ModelsBody = Self::send(req)
            .await?
            .json()
            .await
            .map_err(|e| SpeechError::Decode(e.without_url().to_string()))?;
        Ok(body.data.into_iter().map(|m| m.id).collect())
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
                "/v1/models",
                get(|| async {
                    Json(serde_json::json!({ "data": [{ "id": "kokoro" }, { "id": "parakeet" }] }))
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

    fn client(base: &str, key: Option<&str>) -> SpeechClient {
        let config = SpeechConfig {
            enabled: true,
            base_url: format!("{base}/v1/"),
            stt_language: Some("en".into()),
            ..SpeechConfig::default()
        };
        SpeechClient::new(reqwest::Client::new(), config, key.map(str::to_owned))
    }

    #[tokio::test]
    async fn transcribe_posts_multipart_with_model_language_and_key() {
        let (base, seen) = mock(serde_json::json!([])).await;
        let c = client(&base, Some("sk-secret"));
        let text = c.transcribe(Bytes::from_static(b"OggS"), "audio/ogg", None).await.unwrap();
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
        c.transcribe(Bytes::from_static(b"x"), "audio/webm", Some("fr")).await.unwrap();
        assert!(seen.lock().unwrap().fields.contains(&("language".into(), "fr".into())));
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
            reqwest::Client::new(),
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
