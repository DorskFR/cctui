//! `/voice/*`: speech for signed-in users through the instance speech service.

use axum::body::Body;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use bytes::Bytes;

pub use cctui_proto::api::settings::{VoiceConfigInfo, VoiceSpeakRequest, VoiceTranscript};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::routes::server_settings::{read_speech, speech_client, speech_error};
use crate::speech::SpeechConfig;
use crate::state::AppState;

pub const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;

pub fn audio_mime(format: &str) -> String {
    match format {
        "opus" => "audio/ogg".to_owned(),
        "mp3" => "audio/mpeg".to_owned(),
        "pcm" => "audio/L16".to_owned(),
        f => format!("audio/{f}"),
    }
}

fn config_info(c: SpeechConfig) -> VoiceConfigInfo {
    VoiceConfigInfo {
        enabled: c.enabled && !c.base_url.is_empty(),
        stt_model: c.stt_model,
        stt_language: c.stt_language,
        tts_model: c.tts_model,
        tts_voice: c.tts_voice,
        tts_format: c.tts_format,
    }
}

pub async fn config(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<VoiceConfigInfo>, AppError> {
    Ok(Json(config_info(read_speech(&state.pool).await?.0)))
}

pub async fn transcribe(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
    mut multipart: Multipart,
) -> Result<Json<VoiceTranscript>, AppError> {
    let bad = |msg: &str| AppError::new(StatusCode::BAD_REQUEST, msg);
    let (mut audio, mut mime, mut language) = (None, None, None);
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
            _ => {}
        }
    }
    let audio = audio
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad("a non-empty `file` part is required"))?;
    let mime = mime.unwrap_or_else(|| "audio/webm".to_owned());
    let language = language.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty());
    let client = speech_client(&state).await?;
    let text =
        client.transcribe(audio, &mime, language.as_deref()).await.map_err(|e| speech_error(&e))?;
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
    let stream = client
        .synthesize_stream(&req.text, req.voice.as_deref(), None, req.speed)
        .await
        .map_err(|e| speech_error(&e))?;
    let mime = audio_mime(&client.config().tts_format);
    Ok(([(http::header::CONTENT_TYPE, mime)], Body::from_stream(stream)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn audio_mime_maps_formats() {
        assert_eq!(audio_mime("opus"), "audio/ogg");
        assert_eq!(audio_mime("mp3"), "audio/mpeg");
        assert_eq!(audio_mime("wav"), "audio/wav");
    }
}
