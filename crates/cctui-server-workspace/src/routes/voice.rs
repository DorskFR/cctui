//! `/voice/*`: speech for signed-in users through the instance speech service.

use axum::body::Body;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use bytes::Bytes;

pub use cctui_proto::api::settings::{
    VoiceConfigInfo, VoiceSpeakRequest, VoiceTranscript, VoiceVoices,
};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::routes::server_settings::{read_speech, speech_client, speech_error};
use crate::speech::{MAX_TTS_INPUT_CHARS, SpeechConfig};
use crate::state::AppState;

pub const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
/// Body ceiling for `/voice/transcribe`: the audio cap plus multipart framing
/// headroom, so an over-cap recording reaches the handler's 413 instead of
/// axum's 2 MB default tripping mid-stream as a malformed part.
pub const TRANSCRIBE_BODY_LIMIT: usize = MAX_AUDIO_BYTES + 64 * 1024;
/// Text up to this length is read as written even when a summary is asked for.
pub const SUMMARY_MIN_CHARS: usize = 600;

pub fn audio_mime(format: &str) -> String {
    match format {
        "opus" => "audio/ogg".to_owned(),
        "mp3" => "audio/mpeg".to_owned(),
        "pcm" => "audio/L16".to_owned(),
        f => format!("audio/{f}"),
    }
}

fn config_info(c: SpeechConfig) -> VoiceConfigInfo {
    let enabled = c.enabled && !c.base_url.is_empty();
    VoiceConfigInfo {
        enabled,
        stt: enabled && !c.stt_model.is_empty(),
        tts: enabled && !c.tts_model.is_empty(),
        stt_model: c.stt_model,
        stt_language: c.stt_language,
        tts_model: c.tts_model,
        tts_voice: c.tts_voice,
        tts_format: c.tts_format,
        can_summarize: !c.summary_model.is_empty(),
    }
}

pub async fn config(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<VoiceConfigInfo>, AppError> {
    Ok(Json(config_info(read_speech(&state.pool).await?.0)))
}

struct AudioForm {
    audio: Bytes,
    mime: String,
    language: Option<String>,
    model: Option<String>,
}

/// The `file`, `language` and `model` parts of a transcribe request, the
/// audio capped at [`MAX_AUDIO_BYTES`].
async fn read_audio_form(mut multipart: Multipart) -> Result<AudioForm, AppError> {
    let bad = |msg: &str| AppError::new(StatusCode::BAD_REQUEST, msg);
    let (mut audio, mut mime, mut language, mut model) = (None, None, None, None);
    while let Some(mut field) =
        multipart.next_field().await.map_err(|_| bad("malformed multipart body"))?
    {
        match field.name() {
            Some("file") => {
                mime = field.content_type().map(str::to_owned);
                let mut buf = Vec::new();
                while let Some(chunk) =
                    field.chunk().await.map_err(|_| bad("malformed audio part"))?
                {
                    if buf.len() + chunk.len() > MAX_AUDIO_BYTES {
                        return Err(AppError::new(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!("audio is larger than {MAX_AUDIO_BYTES} bytes"),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }
                audio = Some(Bytes::from(buf));
            }
            Some("language") => {
                language = Some(field.text().await.map_err(|_| bad("malformed language"))?);
            }
            Some("model") => {
                model = Some(field.text().await.map_err(|_| bad("malformed model"))?);
            }
            _ => {}
        }
    }
    let audio = audio
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad("a non-empty `file` part is required"))?;
    let mime = mime.unwrap_or_else(|| "audio/webm".to_owned());
    let trimmed = |v: Option<String>| v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    Ok(AudioForm { audio, mime, language: trimmed(language), model: trimmed(model) })
}

pub async fn transcribe(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
    multipart: Multipart,
) -> Result<Json<VoiceTranscript>, AppError> {
    let form = read_audio_form(multipart).await?;
    let client = speech_client(&state).await?;
    let text = client
        .transcribe(form.audio, &form.mime, form.language.as_deref(), form.model.as_deref())
        .await
        .map_err(|e| speech_error(&e))?;
    Ok(Json(VoiceTranscript { text }))
}

pub async fn speak(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
    Json(req): Json<VoiceSpeakRequest>,
) -> Result<Response, AppError> {
    if req.text.trim().is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "text is required"));
    }
    if req.speed.is_some_and(|s| !(0.25..=4.0).contains(&s)) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "speed must be between 0.25 and 4"));
    }
    let client = speech_client(&state).await?;
    let plain = crate::voice_live::text::strip_markdown(&req.text);
    let text = if plain.is_empty() { req.text.as_str() } else { plain.as_str() };
    let summary = if req.summarize == Some(true)
        && client.can_summarize()
        && text.chars().count() > SUMMARY_MIN_CHARS
    {
        client
            .summarize(text)
            .await
            .inspect_err(|e| tracing::warn!(%e, "voice: summary failed, reading the full text"))
            .ok()
    } else {
        None
    };
    let text = clip_for_speech(summary.as_deref().unwrap_or(text));
    let stream = client
        .synthesize_stream(text, req.voice.as_deref(), None, req.speed)
        .await
        .map_err(|e| speech_error(&e))?;
    let mime = audio_mime(&client.config().tts_format);
    Ok(([(http::header::CONTENT_TYPE, mime)], Body::from_stream(stream)).into_response())
}

/// `text` cut to what one synthesis takes, at the last sentence end that fits.
fn clip_for_speech(text: &str) -> &str {
    let Some((cut, _)) = text.char_indices().nth(MAX_TTS_INPUT_CHARS) else {
        return text;
    };
    let head = &text[..cut];
    head.rfind(['.', '!', '?', '\n']).map_or(head, |i| &head[..=i])
}

pub async fn voices(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<VoiceVoices>, AppError> {
    let client = speech_client(&state).await?;
    let voices = client.voices().await.map_err(|e| speech_error(&e))?;
    Ok(Json(VoiceVoices { voices }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDARY: &str = "----cctui-voice-test";

    fn audio_request(size: usize) -> axum::http::Request<Body> {
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"a.webm\"\r\nContent-Type: audio/webm\r\n\r\n"
        )
        .into_bytes();
        body.resize(body.len() + size, b'x');
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        axum::http::Request::builder()
            .method("POST")
            .uri("/t")
            .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
            .body(Body::from(body))
            .expect("request")
    }

    /// `(status, body)` of the audio-reading half of `transcribe` behind
    /// `limit`, as the real route layers it.
    async fn read_behind(limit: Option<usize>, size: usize) -> (StatusCode, String) {
        use tower::ServiceExt;

        let route = axum::routing::post(|mp: Multipart| async move {
            match read_audio_form(mp).await {
                Ok(form) => (StatusCode::OK, form.audio.len().to_string()).into_response(),
                Err(e) => e.into_response(),
            }
        });
        let route = match limit {
            Some(max) => route.layer(axum::extract::DefaultBodyLimit::max(max)),
            None => route,
        };
        let app = axum::Router::new().route("/t", route);
        let res = app.oneshot(audio_request(size)).await.expect("infallible");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.expect("response body");
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// axum's 2 MB default used to trip mid-stream on any longer recording,
    /// reported as a malformed part; behind the route's limit the whole
    /// 10 MB cap is reachable and an over-cap recording gets the 413.
    #[tokio::test]
    async fn transcribe_accepts_recordings_up_to_the_audio_cap() {
        let three_mb = 3 * 1024 * 1024;
        let (status, _) = read_behind(None, three_mb).await;
        assert_ne!(status, StatusCode::OK, "the default limit is the regression");

        let (status, body) = read_behind(Some(TRANSCRIBE_BODY_LIMIT), three_mb).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "3145728"));

        let (status, body) = read_behind(Some(TRANSCRIBE_BODY_LIMIT), MAX_AUDIO_BYTES).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (status, body) = read_behind(Some(TRANSCRIBE_BODY_LIMIT), MAX_AUDIO_BYTES + 1).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert!(body.contains("audio is larger than"), "{body}");
    }

    #[test]
    fn config_hides_the_url_and_needs_one_to_be_enabled() {
        let c = SpeechConfig { enabled: true, ..SpeechConfig::default() };
        assert!(!config_info(c.clone()).enabled);
        let c = SpeechConfig { base_url: "https://speech.example/v1".into(), ..c };
        let info = config_info(c);
        assert!(info.enabled);
        assert!(!serde_json::to_string(&info).unwrap().contains("speech.example"));
    }

    #[test]
    fn config_reports_stt_and_tts_separately() {
        let on = SpeechConfig {
            enabled: true,
            base_url: "https://s.example/v1".into(),
            ..SpeechConfig::default()
        };
        let both = config_info(on.clone());
        assert_eq!((both.stt, both.tts), (true, true));
        let no_stt = config_info(SpeechConfig { stt_model: String::new(), ..on.clone() });
        assert_eq!((no_stt.enabled, no_stt.stt, no_stt.tts), (true, false, true));
        let no_tts = config_info(SpeechConfig { tts_model: String::new(), ..on.clone() });
        assert_eq!((no_tts.enabled, no_tts.stt, no_tts.tts), (true, true, false));
        let off = config_info(SpeechConfig { enabled: false, ..on });
        assert_eq!((off.stt, off.tts), (false, false));
    }

    #[test]
    fn overlong_text_is_clipped_at_a_sentence_end() {
        assert_eq!(clip_for_speech("short. text"), "short. text");
        let long = format!("{}. {}", "é".repeat(MAX_TTS_INPUT_CHARS - 10), "x".repeat(50));
        let clipped = clip_for_speech(&long);
        assert!(clipped.ends_with('.'));
        assert_eq!(clipped.chars().count(), MAX_TTS_INPUT_CHARS - 9);
        let unbroken = "y".repeat(MAX_TTS_INPUT_CHARS + 5);
        assert_eq!(clip_for_speech(&unbroken).len(), MAX_TTS_INPUT_CHARS);
    }

    #[test]
    fn config_offers_summaries_once_a_summary_model_is_set() {
        let c = SpeechConfig { base_url: "https://s.example/v1".into(), ..SpeechConfig::default() };
        assert!(!config_info(c.clone()).can_summarize);
        let c = SpeechConfig { summary_model: "qwen3-1.7b".into(), ..c };
        assert!(config_info(c).can_summarize);
    }

    #[test]
    fn audio_mime_maps_formats() {
        assert_eq!(audio_mime("opus"), "audio/ogg");
        assert_eq!(audio_mime("mp3"), "audio/mpeg");
        assert_eq!(audio_mime("wav"), "audio/wav");
    }
}
