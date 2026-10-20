//! Admin-editable server settings seeded from env. Each value resolves as:
//! saved in `instance_settings` > env > built-in default, and is read at use
//! time so an edit applies without a restart.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};

pub use cctui_proto::api::settings::{
    SettingSource, SpawnDefaults, SpawnDefaultsInfo, SpawnDefaultsSources, SpeechCatalog,
    SpeechConfig, SpeechSettingsInfo, SpeechSettingsRequest, UploadCapsInfo, UploadCapsRequest,
    UpstreamHostsInfo, UpstreamHostsRequest,
};

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;
use crate::uploads::UploadCaps;

const SPAWN_KEY: &str = "spawn_defaults";
const UPSTREAM_KEY: &str = "upstream_allowed_hosts";
const UPLOAD_CAPS_KEY: &str = "upload_caps";
const SPEECH_KEY: &str = "speech";

/// Built-in layer: only the depth is capped. Child count and tree budget are
/// unlimited unless a saved setting or env sets a ceiling explicitly.
const fn builtin_spawn_defaults() -> SpawnDefaults {
    SpawnDefaults {
        max_children: None,
        max_depth: Some(cctui_proto::api::DEFAULT_MAX_DEPTH),
        max_tree_budget_usd: None,
    }
}

const fn env_spawn_defaults(config: &crate::config::Config) -> SpawnDefaults {
    SpawnDefaults {
        max_children: config.spawn_max_children,
        max_depth: config.spawn_max_depth,
        max_tree_budget_usd: config.spawn_max_tree_budget_usd,
    }
}

fn pick<T: Copy>(
    settings: Option<T>,
    env: Option<T>,
    default: Option<T>,
) -> (Option<T>, SettingSource) {
    settings
        .map(|v| (Some(v), SettingSource::Settings))
        .or_else(|| env.map(|v| (Some(v), SettingSource::Env)))
        .unwrap_or((default, SettingSource::Default))
}

pub fn resolve_spawn_defaults(settings: SpawnDefaults, env: SpawnDefaults) -> SpawnDefaultsInfo {
    let defaults = builtin_spawn_defaults();
    let (c, cs) = pick(settings.max_children, env.max_children, defaults.max_children);
    let (d, ds) = pick(settings.max_depth, env.max_depth, defaults.max_depth);
    let (b, bs) =
        pick(settings.max_tree_budget_usd, env.max_tree_budget_usd, defaults.max_tree_budget_usd);
    SpawnDefaultsInfo {
        effective: SpawnDefaults { max_children: c, max_depth: d, max_tree_budget_usd: b },
        sources: SpawnDefaultsSources { max_children: cs, max_depth: ds, max_tree_budget_usd: bs },
        settings,
        env,
        defaults,
    }
}

async fn stored<T: serde::de::DeserializeOwned>(pool: &sqlx::PgPool, key: &str) -> Option<T> {
    sqlx::query_scalar::<_, serde_json::Value>("SELECT value FROM instance_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
}

async fn store(
    pool: &sqlx::PgPool,
    key: &str,
    value: Option<serde_json::Value>,
) -> Result<(), AppError> {
    match value {
        Some(v) => {
            sqlx::query(
                "INSERT INTO instance_settings (key, value, updated_at) VALUES ($1, $2, now()) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(key)
            .bind(v)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM instance_settings WHERE key = $1")
                .bind(key)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn read_spawn_defaults(state: &AppState) -> SpawnDefaultsInfo {
    let settings = stored(&state.pool, SPAWN_KEY).await.unwrap_or_default();
    resolve_spawn_defaults(settings, env_spawn_defaults(&state.config))
}

/// Capability for a session that declares none.
pub async fn spawn_default_capability(state: &AppState) -> cctui_proto::api::SpawnCapability {
    let e = read_spawn_defaults(state).await.effective;
    cctui_proto::api::SpawnCapability {
        max_children: e.max_children,
        max_depth: e.max_depth,
        max_tree_budget_usd: e.max_tree_budget_usd,
        ..cctui_proto::api::SpawnCapability::machine_default()
    }
}

fn validate_spawn_defaults(v: SpawnDefaults) -> Result<SpawnDefaults, AppError> {
    let bad = |msg: &str| AppError::new(StatusCode::BAD_REQUEST, msg);
    if v.max_children == Some(0) {
        return Err(bad("max_children must be a positive integer"));
    }
    if v.max_depth == Some(0) {
        return Err(bad("max_depth must be a positive integer"));
    }
    if v.max_tree_budget_usd.is_some_and(|b| !b.is_finite() || b < 0.0) {
        return Err(bad("max_tree_budget_usd must be a non-negative number"));
    }
    Ok(v)
}

fn admin(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

pub async fn get_spawn_defaults(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SpawnDefaultsInfo>, AppError> {
    admin(&ctx)?;
    Ok(Json(read_spawn_defaults(&state).await))
}

/// Body is the saved layer; a `null` field falls back to env/default.
pub async fn update_spawn_defaults(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<SpawnDefaults>,
) -> Result<Json<SpawnDefaultsInfo>, AppError> {
    admin(&ctx)?;
    let v = validate_spawn_defaults(req)?;
    let value =
        (v != SpawnDefaults::default()).then(|| serde_json::to_value(v).expect("serializable"));
    store(&state.pool, SPAWN_KEY, value).await?;
    Ok(Json(read_spawn_defaults(&state).await))
}

pub fn resolve_upstream_hosts(
    settings: Option<Vec<String>>,
    env: Vec<String>,
) -> UpstreamHostsInfo {
    let managed = crate::outbound::managed_upstream_entries();
    let source = if settings.is_some() { SettingSource::Settings } else { SettingSource::Default };
    UpstreamHostsInfo { hosts: settings.unwrap_or_default(), source, env, managed }
}

async fn read_upstream_hosts(pool: &sqlx::PgPool) -> Result<UpstreamHostsInfo, sqlx::Error> {
    let raw = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM instance_settings WHERE key = $1",
    )
    .bind(UPSTREAM_KEY)
    .fetch_optional(pool)
    .await?;
    let settings = raw.and_then(|v| serde_json::from_value(v).ok());
    Ok(resolve_upstream_hosts(settings, crate::outbound::env_upstream_entries()))
}

/// Reloads the in-memory upstream allowlist from the table; a failed read
/// keeps the current list.
pub async fn refresh_upstream_allowlist(pool: &sqlx::PgPool) {
    if let Ok(speech) = stored_speech(pool).await {
        let url = speech.as_ref().map(|s| s.config.base_url.as_str()).filter(|u| !u.is_empty());
        crate::outbound::set_speech_upstream(url);
    }
    match read_upstream_hosts(pool).await {
        Ok(info) => crate::outbound::set_upstream_allowlist(&info.hosts),
        Err(e) => tracing::warn!(error = %e, "upstream allowlist refresh failed"),
    }
}

/// Other replicas pick an edit up on this tick.
pub async fn upstream_allowlist_task(pool: sqlx::PgPool) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tick.tick().await;
        refresh_upstream_allowlist(&pool).await;
    }
}

fn validate_upstream_hosts(raw: &[String]) -> Result<Vec<String>, AppError> {
    let mut out: Vec<String> = Vec::new();
    for entry in raw.iter().filter(|e| !e.trim().is_empty()) {
        let host = crate::outbound::normalize_allowlist_entry(entry)
            .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
        if !out.contains(&host) {
            out.push(host);
        }
    }
    Ok(out)
}

pub async fn get_upstream_hosts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<UpstreamHostsInfo>, AppError> {
    admin(&ctx)?;
    Ok(Json(read_upstream_hosts(&state.pool).await?))
}

pub async fn update_upstream_hosts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<UpstreamHostsRequest>,
) -> Result<Json<UpstreamHostsInfo>, AppError> {
    admin(&ctx)?;
    let value = req
        .hosts
        .as_deref()
        .map(validate_upstream_hosts)
        .transpose()?
        .map(|h| serde_json::to_value(h).expect("serializable"));
    store(&state.pool, UPSTREAM_KEY, value).await?;
    let info = read_upstream_hosts(&state.pool).await?;
    crate::outbound::set_upstream_allowlist(&info.hosts);
    Ok(Json(info))
}

fn upload_caps_info(stored: Option<UploadCaps>) -> UploadCapsInfo {
    let source = if stored.is_some() { SettingSource::Settings } else { SettingSource::Default };
    UploadCapsInfo {
        effective: stored.unwrap_or_default(),
        defaults: UploadCaps::default(),
        source,
        body_limit_bytes: crate::config::upload_body_limit(),
        body_limit_env: crate::config::UPLOAD_BODY_LIMIT_ENV,
    }
}

/// Reject caps the enforcement path could not honour. A total cap at or above
/// the router's body limit would surface as a generic body-limit error instead
/// of the 413 `parse_upload_multipart` raises, so it needs the env var and a
/// restart first.
pub fn validate_upload_caps(caps: UploadCaps, body_limit: u64) -> Result<UploadCaps, AppError> {
    let bad = |msg: String| AppError::new(StatusCode::BAD_REQUEST, msg);
    if caps.max_files == 0 {
        return Err(bad("max_files must be a positive integer".into()));
    }
    if caps.max_file_bytes == 0 || caps.max_total_bytes == 0 {
        return Err(bad("size caps must be positive byte counts".into()));
    }
    if caps.max_file_bytes > caps.max_total_bytes {
        return Err(bad("max_file_bytes cannot exceed max_total_bytes".into()));
    }
    if caps.max_total_bytes >= body_limit {
        return Err(bad(format!(
            "max_total_bytes must stay below the {body_limit}-byte request ceiling; raise {} and restart the server to go higher",
            crate::config::UPLOAD_BODY_LIMIT_ENV
        )));
    }
    Ok(caps)
}

async fn read_upload_caps(pool: &sqlx::PgPool) -> UploadCapsInfo {
    upload_caps_info(stored(pool, UPLOAD_CAPS_KEY).await)
}

/// The cached caps every upload is checked against, with no database round
/// trip on the upload path.
pub fn cached_upload_caps(state: &AppState) -> UploadCaps {
    *state.upload_caps.read().expect("upload caps lock")
}

/// Reloads the cache from the table; a failed read keeps the current value.
pub async fn refresh_upload_caps(state: &AppState) {
    let caps = read_upload_caps(&state.pool).await.effective;
    *state.upload_caps.write().expect("upload caps lock") = caps;
}

/// Other replicas pick an edit up on this tick.
pub async fn upload_caps_task(state: AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tick.tick().await;
        refresh_upload_caps(&state).await;
    }
}

pub async fn get_upload_caps(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<UploadCapsInfo>, AppError> {
    admin(&ctx)?;
    Ok(Json(read_upload_caps(&state.pool).await))
}

pub async fn update_upload_caps(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<UploadCapsRequest>,
) -> Result<Json<UploadCapsInfo>, AppError> {
    admin(&ctx)?;
    let value = req
        .caps
        .map(|c| validate_upload_caps(c, crate::config::upload_body_limit()))
        .transpose()?
        .map(|c| serde_json::to_value(c).expect("serializable"));
    store(&state.pool, UPLOAD_CAPS_KEY, value).await?;
    refresh_upload_caps(&state).await;
    Ok(Json(read_upload_caps(&state.pool).await))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredSpeech {
    #[serde(flatten)]
    config: SpeechConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encrypted_key: Option<String>,
}

async fn stored_speech(pool: &sqlx::PgPool) -> Result<Option<StoredSpeech>, sqlx::Error> {
    let raw = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM instance_settings WHERE key = $1",
    )
    .bind(SPEECH_KEY)
    .fetch_optional(pool)
    .await?;
    Ok(raw.and_then(|v| serde_json::from_value(v).ok()))
}

fn speech_info(stored: Option<&StoredSpeech>) -> SpeechSettingsInfo {
    SpeechSettingsInfo {
        config: stored.map(|s| s.config.clone()).unwrap_or_default(),
        has_key: stored.is_some_and(|s| s.encrypted_key.is_some()),
        source: if stored.is_some() { SettingSource::Settings } else { SettingSource::Default },
    }
}

/// The saved speech setting with its decrypted key, read at use time.
pub async fn read_speech(pool: &sqlx::PgPool) -> Result<(SpeechConfig, Option<String>), AppError> {
    let stored = stored_speech(pool).await?.unwrap_or_default();
    let key = match stored.encrypted_key {
        Some(enc) => {
            Some(crate::crypto::decrypt(&enc, &crate::crypto::vault_key()).ok_or_else(|| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "speech API key cannot be decrypted",
                )
            })?)
        }
        None => None,
    };
    Ok((stored.config, key))
}

const SPEECH_FORMATS: &[&str] = &["opus", "mp3", "aac", "flac", "wav", "pcm"];

fn validate_speech(c: SpeechConfig) -> Result<SpeechConfig, AppError> {
    let bad = |msg: String| AppError::new(StatusCode::BAD_REQUEST, msg);
    let trim = |s: &str| s.trim().to_owned();
    let c = SpeechConfig {
        enabled: c.enabled,
        base_url: trim(&c.base_url).trim_end_matches('/').to_owned(),
        stt_model: trim(&c.stt_model),
        stt_language: c.stt_language.map(|l| trim(&l)).filter(|l| !l.is_empty()),
        tts_model: trim(&c.tts_model),
        tts_voice: trim(&c.tts_voice),
        tts_format: trim(&c.tts_format).to_ascii_lowercase(),
    };
    if c.base_url.is_empty() {
        if c.enabled {
            return Err(bad("base_url is required to enable speech".into()));
        }
    } else {
        let url = reqwest::Url::parse(&c.base_url)
            .map_err(|_| bad(format!("`{}` is not a URL", c.base_url)))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(bad("base_url must be an http(s) URL with a host".into()));
        }
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
            return Err(bad("base_url cannot carry credentials or a query".into()));
        }
    }
    for (name, v) in
        [("stt_model", &c.stt_model), ("tts_model", &c.tts_model), ("tts_voice", &c.tts_voice)]
    {
        if v.is_empty() || v.len() > 128 {
            return Err(bad(format!("{name} must be 1 to 128 characters")));
        }
    }
    if c.stt_language.as_deref().is_some_and(|l| {
        l.len() > 16 || !l.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    }) {
        return Err(bad("stt_language must be a language code such as `en`".into()));
    }
    if !SPEECH_FORMATS.contains(&c.tts_format.as_str()) {
        return Err(bad(format!("tts_format must be one of {}", SPEECH_FORMATS.join(", "))));
    }
    Ok(c)
}

fn validate_speech_key(raw: &str) -> Result<String, AppError> {
    let key = raw.trim();
    if key.is_empty() || key.len() > 4096 || key.chars().any(char::is_whitespace) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "api_key must be a single token"));
    }
    Ok(key.to_owned())
}

pub async fn get_speech(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SpeechSettingsInfo>, AppError> {
    admin(&ctx)?;
    Ok(Json(speech_info(stored_speech(&state.pool).await?.as_ref())))
}

pub async fn update_speech(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<SpeechSettingsRequest>,
) -> Result<Json<SpeechSettingsInfo>, AppError> {
    admin(&ctx)?;
    let config = validate_speech(req.config)?;
    let new_key = req.api_key.as_deref().map(validate_speech_key).transpose()?;
    let current = stored_speech(&state.pool).await?;
    let encrypted_key = match (new_key, req.clear_key.unwrap_or(false)) {
        (Some(k), _) => Some(crate::crypto::encrypt(&k, &crate::crypto::vault_key())),
        (None, true) => None,
        (None, false) => current.and_then(|c| c.encrypted_key),
    };
    let next = StoredSpeech { config, encrypted_key };
    let value = (next != StoredSpeech::default())
        .then(|| serde_json::to_value(&next).expect("serializable"));
    store(&state.pool, SPEECH_KEY, value).await?;
    refresh_upstream_allowlist(&state.pool).await;
    Ok(Json(speech_info(stored_speech(&state.pool).await?.as_ref())))
}

pub fn speech_error(e: &crate::speech::SpeechError) -> AppError {
    use crate::speech::SpeechError as E;
    let status = match e {
        E::NotConfigured => StatusCode::CONFLICT,
        E::InputTooLong | E::Url(_) => StatusCode::BAD_REQUEST,
        E::Transport(_) | E::Upstream { .. } | E::Decode(_) => StatusCode::BAD_GATEWAY,
    };
    AppError::new(status, e.to_string())
}

pub async fn speech_client(state: &AppState) -> Result<crate::speech::SpeechClient, AppError> {
    let (config, key) = read_speech(&state.pool).await?;
    crate::speech::SpeechClient::for_upstream(config, key).map_err(|e| speech_error(&e))
}

/// Models and voices the configured service offers, for the admin pickers.
pub async fn speech_catalog(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SpeechCatalog>, AppError> {
    admin(&ctx)?;
    let client = speech_client(&state).await?;
    let models = client.health().await.map_err(|e| speech_error(&e))?;
    let voices = client.voices().await.unwrap_or_default();
    Ok(Json(SpeechCatalog { models, voices }))
}

/// Health check plus a short synthesis the admin UI plays back.
pub async fn test_speech(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<axum::response::Response, AppError> {
    admin(&ctx)?;
    let client = speech_client(&state).await?;
    client.health().await.map_err(|e| speech_error(&e))?;
    let audio = client
        .synthesize("Speech is working.", None, None, None)
        .await
        .map_err(|e| speech_error(&e))?;
    let mime = crate::routes::voice::audio_mime(&client.config().tts_format);
    Ok(([(http::header::CONTENT_TYPE, mime)], audio).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn ctx(scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: scopes.iter().copied().collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn upload_caps_fall_back_to_the_built_ins() {
        let info = upload_caps_info(None);
        assert_eq!(info.source, SettingSource::Default);
        assert_eq!(info.effective, UploadCaps::default());
        assert_eq!(info.body_limit_env, crate::config::UPLOAD_BODY_LIMIT_ENV);

        let saved = UploadCaps { max_files: 3, max_file_bytes: 1024, max_total_bytes: 4096 };
        let info = upload_caps_info(Some(saved));
        assert_eq!(info.source, SettingSource::Settings);
        assert_eq!(info.effective, saved);
        assert_eq!(info.defaults, UploadCaps::default());
    }

    #[test]
    fn upload_caps_at_or_above_the_body_ceiling_are_refused_by_name() {
        let limit = 1000;
        let at = UploadCaps { max_files: 1, max_file_bytes: 10, max_total_bytes: limit };
        let err = validate_upload_caps(at, limit).unwrap_err();
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        assert!(err.message().contains(crate::config::UPLOAD_BODY_LIMIT_ENV), "{}", err.message());
        assert!(err.message().contains("1000"), "{}", err.message());
        assert!(
            validate_upload_caps(UploadCaps { max_total_bytes: limit + 1, ..at }, limit).is_err()
        );
        assert!(
            validate_upload_caps(UploadCaps { max_total_bytes: limit - 1, ..at }, limit).is_ok()
        );
    }

    #[test]
    fn upload_caps_reject_zeroes_and_an_inverted_pair() {
        let limit = 1000;
        let ok = UploadCaps { max_files: 2, max_file_bytes: 100, max_total_bytes: 200 };
        assert!(validate_upload_caps(ok, limit).is_ok());
        for bad in [
            UploadCaps { max_files: 0, ..ok },
            UploadCaps { max_file_bytes: 0, ..ok },
            UploadCaps { max_total_bytes: 0, ..ok },
            UploadCaps { max_file_bytes: 300, ..ok },
        ] {
            assert_eq!(
                validate_upload_caps(bad, limit).unwrap_err().status(),
                StatusCode::BAD_REQUEST,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn spawn_defaults_resolve_settings_then_env_then_default() {
        let settings = SpawnDefaults { max_children: Some(2), ..Default::default() };
        let env =
            SpawnDefaults { max_children: Some(5), max_depth: Some(1), max_tree_budget_usd: None };
        let info = resolve_spawn_defaults(settings, env);
        assert_eq!(info.effective.max_children, Some(2));
        assert_eq!(info.sources.max_children, SettingSource::Settings);
        assert_eq!(info.effective.max_depth, Some(1));
        assert_eq!(info.sources.max_depth, SettingSource::Env);
        assert_eq!(info.effective.max_tree_budget_usd, None, "unset budget is unlimited");
        assert_eq!(info.sources.max_tree_budget_usd, SettingSource::Default);
    }

    #[test]
    fn spawn_defaults_are_unlimited_unless_set_explicitly() {
        let info = resolve_spawn_defaults(SpawnDefaults::default(), SpawnDefaults::default());
        assert_eq!(info.effective.max_children, None);
        assert_eq!(info.effective.max_tree_budget_usd, None);
        assert_eq!(info.effective.max_depth, Some(cctui_proto::api::DEFAULT_MAX_DEPTH));
        assert_eq!(info.defaults.max_children, None);
        assert_eq!(info.defaults.max_tree_budget_usd, None);
    }

    #[test]
    fn spawn_defaults_validation() {
        let ok = SpawnDefaults {
            max_children: Some(1),
            max_depth: Some(1),
            max_tree_budget_usd: Some(0.0),
        };
        assert!(validate_spawn_defaults(ok).is_ok());
        for bad in [
            SpawnDefaults { max_children: Some(0), ..Default::default() },
            SpawnDefaults { max_depth: Some(0), ..Default::default() },
            SpawnDefaults { max_tree_budget_usd: Some(-1.0), ..Default::default() },
            SpawnDefaults { max_tree_budget_usd: Some(f64::INFINITY), ..Default::default() },
            SpawnDefaults { max_tree_budget_usd: Some(f64::NAN), ..Default::default() },
        ] {
            assert_eq!(validate_spawn_defaults(bad).unwrap_err().status(), StatusCode::BAD_REQUEST);
        }
        let parsed: Result<SpawnDefaults, _> = serde_json::from_str(r#"{"max_children": -1}"#);
        assert!(parsed.is_err());
    }

    #[test]
    fn upstream_hosts_keep_env_separate_from_saved() {
        let env = vec!["a.example".to_owned()];
        let info = resolve_upstream_hosts(Some(vec!["b.example".into()]), env.clone());
        assert_eq!(info.hosts, vec!["b.example".to_owned()]);
        assert_eq!((info.source, info.env), (SettingSource::Settings, env.clone()));
        let info = resolve_upstream_hosts(None, env.clone());
        assert_eq!((info.hosts.len(), info.source), (0, SettingSource::Default));
        assert_eq!(info.env, env);
        let info = resolve_upstream_hosts(None, vec![]);
        assert_eq!((info.hosts.len(), info.source), (0, SettingSource::Default));
    }

    #[test]
    fn upstream_hosts_validation_normalizes_and_dedupes() {
        let ok =
            validate_upstream_hosts(&[" A.example ".into(), "a.example".into(), String::new()]);
        assert_eq!(ok.unwrap(), vec!["a.example".to_owned()]);
        let bad = validate_upstream_hosts(&["https://a.example".into()]);
        assert_eq!(bad.unwrap_err().status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn upstream_hosts_round_trip_and_apply_without_restart() {
        let _serial = UPSTREAM_TESTS.lock().await;
        let Some(state) = state("upstream_hosts_round_trip_and_apply_without_restart").await else {
            return;
        };
        let url = "http://settings-allowlist-probe.internal:8123/v1";
        let denied = update_upstream_hosts(
            State(state.clone()),
            Extension(ctx(&[])),
            Json(UpstreamHostsRequest { hosts: Some(vec!["x.example".into()]) }),
        )
        .await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        let denied = get_upstream_hosts(State(state.clone()), Extension(ctx(&[]))).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        assert!(crate::outbound::upstream_url_permitted(url).is_err());

        let Json(info) = update_upstream_hosts(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(UpstreamHostsRequest {
                hosts: Some(vec!["Settings-Allowlist-Probe.internal:8123".into()]),
            }),
        )
        .await
        .unwrap();
        assert_eq!(info.source, SettingSource::Settings);
        assert_eq!(info.hosts, vec!["settings-allowlist-probe.internal:8123".to_owned()]);
        crate::outbound::upstream_url_permitted(url).unwrap();
        crate::outbound::validate_upstream_url(url).await.unwrap();

        let bad = update_upstream_hosts(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(UpstreamHostsRequest { hosts: Some(vec!["*.example".into()]) }),
        )
        .await;
        assert_eq!(bad.unwrap_err().status(), StatusCode::BAD_REQUEST);

        let Json(info) = update_upstream_hosts(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(UpstreamHostsRequest { hosts: None }),
        )
        .await
        .unwrap();
        assert_ne!(info.source, SettingSource::Settings);
        assert!(crate::outbound::upstream_url_permitted(url).is_err());
    }

    #[test]
    fn speech_validation_trims_and_rejects_bad_values() {
        let ok = SpeechConfig {
            enabled: true,
            base_url: " https://speech.example/v1/ ".into(),
            stt_language: Some(" ".into()),
            tts_format: "MP3".into(),
            ..SpeechConfig::default()
        };
        let v = validate_speech(ok.clone()).unwrap();
        assert_eq!(v.base_url, "https://speech.example/v1");
        assert_eq!((v.stt_language, v.tts_format.as_str()), (None, "mp3"));
        assert!(validate_speech(SpeechConfig::default()).is_ok());
        for bad in [
            SpeechConfig { base_url: String::new(), ..ok.clone() },
            SpeechConfig { base_url: "ftp://speech.example".into(), ..ok.clone() },
            SpeechConfig { base_url: "https://u:p@speech.example/v1".into(), ..ok.clone() },
            SpeechConfig { base_url: "not a url".into(), ..ok.clone() },
            SpeechConfig { tts_voice: " ".into(), ..ok.clone() },
            SpeechConfig { stt_model: String::new(), ..ok.clone() },
            SpeechConfig { stt_language: Some("en; drop".into()), ..ok.clone() },
            SpeechConfig { tts_format: "ogg".into(), ..ok },
        ] {
            let status = validate_speech(bad.clone()).unwrap_err().status();
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
        }
        assert!(validate_speech_key("sk abc").is_err());
        assert!(validate_speech_key("  ").is_err());
        assert_eq!(validate_speech_key(" sk-1 ").unwrap(), "sk-1");
    }

    #[tokio::test]
    async fn speech_key_is_encrypted_redacted_and_host_allowlisted() {
        let _serial = UPSTREAM_TESTS.lock().await;
        let Some(state) = state("speech_key_is_encrypted_redacted_and_host_allowlisted").await
        else {
            return;
        };
        crate::crypto::install_vault_key(vec![7u8; 32]);
        let url = "http://speech-router.speech-probe.svc:8000/v1";
        let put = |config: SpeechConfig, api_key: Option<&str>, clear_key: Option<bool>| {
            update_speech(
                State(state.clone()),
                Extension(ctx(&[Scope::Admin])),
                Json(SpeechSettingsRequest { config, api_key: api_key.map(Into::into), clear_key }),
            )
        };
        let denied = get_speech(State(state.clone()), Extension(ctx(&[]))).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        assert!(crate::outbound::upstream_url_permitted(url).is_err());

        let config =
            SpeechConfig { enabled: true, base_url: url.into(), ..SpeechConfig::default() };
        let Json(info) = put(config.clone(), Some("sk-speech-secret"), None).await.unwrap();
        assert!(info.has_key);
        assert_eq!(info.source, SettingSource::Settings);
        assert!(!serde_json::to_string(&info).unwrap().contains("sk-speech-secret"));
        let raw: serde_json::Value =
            sqlx::query_scalar("SELECT value FROM instance_settings WHERE key = 'speech'")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(!raw.to_string().contains("sk-speech-secret"), "{raw}");
        assert_eq!(read_speech(&state.pool).await.unwrap().1.as_deref(), Some("sk-speech-secret"));
        crate::outbound::upstream_url_permitted(url).unwrap();
        let hosts = read_upstream_hosts(&state.pool).await.unwrap();
        assert!(hosts.managed.contains(&"speech-router.speech-probe.svc:8000".to_owned()));

        let Json(info) =
            put(SpeechConfig { tts_voice: "bf_emma".into(), ..config.clone() }, None, None)
                .await
                .unwrap();
        assert!(info.has_key);
        assert_eq!(info.config.tts_voice, "bf_emma");

        let Json(info) = put(config.clone(), None, Some(true)).await.unwrap();
        assert!(!info.has_key);
        assert_eq!(read_speech(&state.pool).await.unwrap().1, None);

        let bad = put(SpeechConfig { base_url: String::new(), ..config }, None, None).await;
        assert_eq!(bad.unwrap_err().status(), StatusCode::BAD_REQUEST);

        let Json(info) = put(SpeechConfig::default(), None, Some(true)).await.unwrap();
        assert_eq!(info.source, SettingSource::Default);
        assert!(crate::outbound::upstream_url_permitted(url).is_err());
    }

    static UPSTREAM_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn upgrade_seeds_existing_upstreams_into_the_allowlist() {
        let _serial = UPSTREAM_TESTS.lock().await;
        let Some(state) = state("upgrade_seeds_existing_upstreams_into_the_allowlist").await else {
            return;
        };
        let pool = &state.pool;
        sqlx::query("DELETE FROM instance_settings WHERE key = $1")
            .bind(UPSTREAM_KEY)
            .execute(pool)
            .await
            .unwrap();
        let (user, account) = (Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(user)
            .bind(format!("seed-{user}"))
            .bind(format!("seed-{user}-hash"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, 'seed')")
            .bind(account)
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account_providers (user_id, account_id, provider, base_url, auth_scheme) \
             VALUES ($1, $2, 'anthropic', 'http://10.9.8.7:4000/v1', 'api_key')",
        )
        .bind(user)
        .bind(account)
        .execute(pool)
        .await
        .unwrap();
        let old = "http://10.9.8.7:4000/v1/messages";
        let new = "http://10.9.8.6:4000/v1";
        refresh_upstream_allowlist(pool).await;
        assert!(crate::outbound::upstream_url_permitted(old).is_err());

        sqlx::raw_sql(include_str!("../../../../migrations/140_seed_upstream_allowlist.up.sql"))
            .execute(pool)
            .await
            .unwrap();
        refresh_upstream_allowlist(pool).await;
        let info = read_upstream_hosts(pool).await.unwrap();
        assert_eq!(info.source, SettingSource::Settings);
        assert!(info.hosts.contains(&"10.9.8.7:4000".to_owned()), "{:?}", info.hosts);
        crate::outbound::upstream_url_permitted(old).unwrap();
        crate::routes::accounts::check_base_url("http://10.9.8.7:4000/v1").await.unwrap();
        assert!(crate::outbound::upstream_url_permitted(new).is_err());
        assert!(crate::routes::accounts::check_base_url(new).await.is_err());

        sqlx::query("DELETE FROM users WHERE id = $1").bind(user).execute(pool).await.unwrap();
        sqlx::query("DELETE FROM instance_settings WHERE key = $1")
            .bind(UPSTREAM_KEY)
            .execute(pool)
            .await
            .unwrap();
        refresh_upstream_allowlist(pool).await;
    }

    async fn state(tag: &str) -> Option<AppState> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        Some(AppState::for_test(pool))
    }

    #[tokio::test]
    async fn spawn_defaults_round_trip_and_apply_without_restart() {
        let Some(mut state) = state("spawn_defaults_round_trip_and_apply_without_restart").await
        else {
            return;
        };
        state.config.spawn_max_depth = Some(2);
        sqlx::query("DELETE FROM instance_settings WHERE key = $1")
            .bind(SPAWN_KEY)
            .execute(&state.pool)
            .await
            .unwrap();

        let denied = get_spawn_defaults(State(state.clone()), Extension(ctx(&[]))).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        let denied = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[])),
            Json(SpawnDefaults { max_children: Some(3), ..Default::default() }),
        )
        .await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);

        let cap = spawn_default_capability(&state).await;
        assert_eq!(cap.max_children, None, "no child-count cap unless one is set");
        assert_eq!(cap.max_tree_budget_usd, None, "no tree budget unless one is set");
        assert_eq!(cap.max_budget_usd, None, "no per-child budget unless one is set");
        assert_eq!(cap.max_depth, Some(2));

        let Json(info) = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(SpawnDefaults {
                max_children: Some(3),
                max_depth: Some(4),
                max_tree_budget_usd: Some(9.5),
            }),
        )
        .await
        .unwrap();
        assert_eq!(info.sources.max_depth, SettingSource::Settings);
        let cap = spawn_default_capability(&state).await;
        assert_eq!(
            (cap.max_children, cap.max_depth, cap.max_tree_budget_usd),
            (Some(3), Some(4), Some(9.5))
        );

        let bad = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(SpawnDefaults { max_depth: Some(0), ..Default::default() }),
        )
        .await;
        assert_eq!(bad.unwrap_err().status(), StatusCode::BAD_REQUEST);

        let Json(info) = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(SpawnDefaults::default()),
        )
        .await
        .unwrap();
        assert_eq!(info.sources.max_depth, SettingSource::Env);
        assert_eq!(info.effective.max_depth, Some(2));
        assert_eq!(info.sources.max_children, SettingSource::Default);
    }
}
