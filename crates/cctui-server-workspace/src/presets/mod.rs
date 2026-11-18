pub mod cron;
pub mod store;
#[cfg(test)]
pub mod store_tests;
pub mod tz;

use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::presets::{LaunchPresetRequest, OnStale, Overlap};
use uuid::Uuid;

pub const FRESH_SECS_DEFAULT: i32 = 900;
pub const FRESH_SECS_MIN: i32 = 60;
pub const FRESH_SECS_MAX: i32 = 2700;
pub const MAX_BATCH_DEFAULT: i32 = 20;
pub const MAX_BATCH_MAX: i32 = 100;
const MAX_NAME_CHARS: usize = 120;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_NOTIFY_URL_CHARS: usize = 2048;
const MAX_TIMEZONE_CHARS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Machine {
        machine_id: Uuid,
        working_dir: String,
        adapter_id: String,
        model: Option<String>,
        permission_mode: Option<PermissionMode>,
    },
    Container {
        dispatcher_id: Uuid,
        profile: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPreset {
    pub name: String,
    pub prompt: String,
    pub target: Target,
    pub external_sources: bool,
    pub fresh_secs: i32,
    pub on_stale: OnStale,
    pub max_batch: i32,
    pub cron: Option<String>,
    pub timezone: String,
    pub enabled: bool,
    pub overlap: Overlap,
    pub then_preset_id: Option<Uuid>,
    pub notify_url: Option<String>,
}

fn trimmed(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned)
}

#[must_use]
pub fn clamp_fresh_secs(raw: Option<i32>) -> i32 {
    raw.unwrap_or(FRESH_SECS_DEFAULT).clamp(FRESH_SECS_MIN, FRESH_SECS_MAX)
}

#[must_use]
pub fn clamp_max_batch(raw: Option<i32>) -> i32 {
    raw.unwrap_or(MAX_BATCH_DEFAULT).clamp(1, MAX_BATCH_MAX)
}

fn machine_target(req: &LaunchPresetRequest, machine_id: Uuid) -> Result<Target, String> {
    if req.dispatcher_id.is_some() || trimmed(req.profile.as_deref()).is_some() {
        return Err("a preset targets a machine or a dispatcher, not both".into());
    }
    let working_dir = trimmed(req.working_dir.as_deref())
        .ok_or("working_dir is required for a machine target")?;
    if !working_dir.starts_with('/') && !working_dir.starts_with('~') {
        return Err("working_dir must be an absolute path".into());
    }
    let adapter_id = trimmed(req.adapter_id.as_deref()).unwrap_or_else(|| "claude-code".to_owned());
    Ok(Target::Machine {
        machine_id,
        working_dir,
        adapter_id,
        model: trimmed(req.model.as_deref()),
        permission_mode: req.permission_mode,
    })
}

fn container_target(req: &LaunchPresetRequest) -> Result<Target, String> {
    let Some(dispatcher_id) = req.dispatcher_id else {
        return Err("a preset needs a target: machine_id, or dispatcher_id and profile".into());
    };
    let profile =
        trimmed(req.profile.as_deref()).ok_or("profile is required for a dispatcher target")?;
    let machine_only = [&req.working_dir, &req.adapter_id, &req.model];
    if machine_only.iter().any(|f| trimmed(f.as_deref()).is_some()) || req.permission_mode.is_some()
    {
        return Err(
            "working_dir, adapter_id, model and permission_mode apply to machine targets only"
                .into(),
        );
    }
    Ok(Target::Container { dispatcher_id, profile })
}

fn target(req: &LaunchPresetRequest) -> Result<Target, String> {
    req.machine_id
        .map_or_else(|| container_target(req), |machine_id| machine_target(req, machine_id))
}

/// External text reaching a machine session is prompt injection with a warm
/// context: only a session that asks before acting, or runs sandboxed, takes it.
fn guard_external(target: &Target, external_sources: bool) -> Result<(), String> {
    let guarded = match target {
        Target::Machine { permission_mode, .. } => {
            matches!(permission_mode, Some(PermissionMode::Ask | PermissionMode::Auto))
        }
        Target::Container { .. } => true,
    };
    if external_sources && !guarded {
        return Err("a machine preset that accepts external sources must run guarded: set \
                    permission_mode to ask or auto"
            .into());
    }
    Ok(())
}

/// The address is vetted again, with DNS, when a dispatch registers it.
fn notify_url(target: &Target, raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(url) = trimmed(raw) else { return Ok(None) };
    if matches!(target, Target::Machine { .. }) {
        return Err("notify_url applies to dispatcher targets only".into());
    }
    if !url.starts_with("https://") {
        return Err("notify_url must be an https URL".into());
    }
    if url.chars().count() > MAX_NOTIFY_URL_CHARS {
        return Err(format!("notify_url must be at most {MAX_NOTIFY_URL_CHARS} characters"));
    }
    Ok(Some(url))
}

pub fn validate(req: &LaunchPresetRequest) -> Result<ValidPreset, String> {
    let name = req.name.trim().to_owned();
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("name must be at most {MAX_NAME_CHARS} characters"));
    }
    if req.prompt.trim().is_empty() {
        return Err("prompt must not be empty".into());
    }
    if req.prompt.len() > MAX_PROMPT_BYTES {
        return Err(format!("prompt must be at most {MAX_PROMPT_BYTES} bytes"));
    }
    let target = target(req)?;
    guard_external(&target, req.external_sources)?;
    let notify_url = notify_url(&target, req.notify_url.as_deref())?;
    let cron = trimmed(req.cron.as_deref());
    if let Some(expr) = &cron {
        cron::validate(expr)?;
    }
    let timezone = trimmed(req.timezone.as_deref()).unwrap_or_else(|| tz::UTC.to_owned());
    if timezone.chars().count() > MAX_TIMEZONE_CHARS {
        return Err(format!("timezone must be at most {MAX_TIMEZONE_CHARS} characters"));
    }
    Ok(ValidPreset {
        name,
        prompt: req.prompt.clone(),
        target,
        external_sources: req.external_sources,
        fresh_secs: clamp_fresh_secs(req.fresh_secs),
        on_stale: req.on_stale.unwrap_or_default(),
        max_batch: clamp_max_batch(req.max_batch),
        cron,
        timezone,
        enabled: req.enabled.unwrap_or(true),
        overlap: req.overlap.unwrap_or_default(),
        then_preset_id: req.then_preset_id,
        notify_url,
    })
}

#[cfg(test)]
mod tests {
    use cctui_proto::adapter::PermissionMode;
    use cctui_proto::api::presets::{LaunchPresetRequest, OnStale, Overlap};
    use uuid::Uuid;

    use super::{Target, validate};

    fn machine() -> LaunchPresetRequest {
        LaunchPresetRequest {
            name: " nightly ".into(),
            prompt: "triage the queue".into(),
            machine_id: Some(Uuid::nil()),
            working_dir: Some("/w".into()),
            ..LaunchPresetRequest::default()
        }
    }

    fn container() -> LaunchPresetRequest {
        LaunchPresetRequest {
            name: "worker".into(),
            prompt: "p".into(),
            dispatcher_id: Some(Uuid::nil()),
            profile: Some("default".into()),
            ..LaunchPresetRequest::default()
        }
    }

    #[test]
    fn a_machine_preset_gets_the_defaults() {
        let valid = validate(&machine()).unwrap();
        assert_eq!(valid.name, "nightly");
        assert_eq!((valid.fresh_secs, valid.max_batch), (900, 20));
        assert_eq!(valid.on_stale, OnStale::Followup);
        let Target::Machine { adapter_id, .. } = valid.target else { panic!("machine target") };
        assert_eq!(adapter_id, "claude-code");
    }

    #[test]
    fn fresh_secs_and_max_batch_are_clamped() {
        for (raw, want) in [(Some(1), 60), (Some(59), 60), (Some(600), 600), (Some(99_999), 2700)] {
            let req = LaunchPresetRequest { fresh_secs: raw, ..machine() };
            assert_eq!(validate(&req).unwrap().fresh_secs, want, "{raw:?}");
        }
        for (raw, want) in [(Some(0), 1), (Some(-3), 1), (Some(500), 100), (None, 20)] {
            let req = LaunchPresetRequest { max_batch: raw, ..machine() };
            assert_eq!(validate(&req).unwrap().max_batch, want, "{raw:?}");
        }
    }

    #[test]
    fn a_target_is_a_machine_or_a_dispatcher() {
        assert!(validate(&container()).is_ok());
        let both = LaunchPresetRequest { dispatcher_id: Some(Uuid::nil()), ..machine() };
        assert!(validate(&both).unwrap_err().contains("not both"));
        let neither = LaunchPresetRequest { machine_id: None, working_dir: None, ..machine() };
        assert!(validate(&neither).unwrap_err().contains("needs a target"));
        let no_cwd = LaunchPresetRequest { working_dir: Some(" ".into()), ..machine() };
        assert!(validate(&no_cwd).unwrap_err().contains("working_dir"));
        let relative = LaunchPresetRequest { working_dir: Some("w".into()), ..machine() };
        assert!(validate(&relative).unwrap_err().contains("absolute"));
        let no_profile = LaunchPresetRequest { profile: None, ..container() };
        assert!(validate(&no_profile).unwrap_err().contains("profile"));
        let model = LaunchPresetRequest { model: Some("opus".into()), ..container() };
        assert!(validate(&model).unwrap_err().contains("machine targets only"));
    }

    #[test]
    fn a_machine_preset_taking_external_sources_must_be_guarded() {
        for mode in [None, Some(PermissionMode::Yolo), Some(PermissionMode::Whip)] {
            let req =
                LaunchPresetRequest { external_sources: true, permission_mode: mode, ..machine() };
            assert!(validate(&req).unwrap_err().contains("guarded"), "{mode:?}");
        }
        for mode in [PermissionMode::Ask, PermissionMode::Auto] {
            let req = LaunchPresetRequest {
                external_sources: true,
                permission_mode: Some(mode),
                ..machine()
            };
            assert!(validate(&req).is_ok(), "{mode:?}");
        }
        let unguarded_internal =
            LaunchPresetRequest { permission_mode: Some(PermissionMode::Yolo), ..machine() };
        assert!(validate(&unguarded_internal).is_ok());
        let external_container = LaunchPresetRequest { external_sources: true, ..container() };
        assert!(validate(&external_container).is_ok());
    }

    #[test]
    fn notify_url_is_an_https_url_on_a_dispatcher_target() {
        let url = Some(" https://n8n.example/hook ".to_owned());
        let container_url = LaunchPresetRequest { notify_url: url.clone(), ..container() };
        assert_eq!(
            validate(&container_url).unwrap().notify_url.as_deref(),
            Some("https://n8n.example/hook")
        );
        let machine_url = LaunchPresetRequest { notify_url: url, ..machine() };
        assert!(validate(&machine_url).unwrap_err().contains("dispatcher targets only"));
        let plain = LaunchPresetRequest { notify_url: Some("http://x/h".into()), ..container() };
        assert!(validate(&plain).unwrap_err().contains("https"));
        let blank = LaunchPresetRequest { notify_url: Some(" ".into()), ..container() };
        assert_eq!(validate(&blank).unwrap().notify_url, None);
    }

    #[test]
    fn name_prompt_and_cron_are_checked() {
        assert!(validate(&LaunchPresetRequest { name: " ".into(), ..machine() }).is_err());
        assert!(validate(&LaunchPresetRequest { prompt: "\n".into(), ..machine() }).is_err());
        let bad_cron = LaunchPresetRequest { cron: Some("every day".into()), ..machine() };
        assert!(validate(&bad_cron).unwrap_err().starts_with("cron"));
        let cron = LaunchPresetRequest { cron: Some(" 0 9 * * 1-5 ".into()), ..machine() };
        assert_eq!(validate(&cron).unwrap().cron.as_deref(), Some("0 9 * * 1-5"));
        let blank = LaunchPresetRequest { cron: Some(String::new()), ..machine() };
        assert_eq!(validate(&blank).unwrap().cron, None);
    }

    #[test]
    fn a_schedule_defaults_to_utc_enabled_and_overlapping() {
        let valid = validate(&machine()).unwrap();
        assert_eq!(
            (valid.timezone.as_str(), valid.enabled, valid.overlap),
            ("UTC", true, Overlap::Allow)
        );
        let set = LaunchPresetRequest {
            timezone: Some(" Asia/Tokyo ".into()),
            enabled: Some(false),
            overlap: Some(Overlap::Skip),
            ..machine()
        };
        let valid = validate(&set).unwrap();
        assert_eq!(
            (valid.timezone.as_str(), valid.enabled, valid.overlap),
            ("Asia/Tokyo", false, Overlap::Skip)
        );
        let long = LaunchPresetRequest { timezone: Some("x".repeat(65)), ..machine() };
        assert!(validate(&long).unwrap_err().contains("timezone"));
    }
}
