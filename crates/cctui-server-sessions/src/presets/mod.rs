pub mod cron;
pub mod from_draft;
pub mod store;
#[cfg(test)]
pub mod store_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod tz;

use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::presets::{LaunchPresetRequest, OnStale, Overlap, SpawnTemplate};
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
const MAX_TEMPLATE_LIST: usize = 64;

/// `Auto` binds the matching account with the most allocation left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountPick {
    Auto,
    Account(Uuid),
    Pool(Uuid),
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Machine {
        machine_id: Uuid,
        working_dir: String,
        adapter_id: String,
        model: Option<String>,
        permission_mode: Option<PermissionMode>,
        account: AccountPick,
    },
    Container {
        dispatcher_id: Uuid,
        profile: String,
    },
}

/// What a write does to the preset's sealed env.
#[derive(Clone, PartialEq, Eq)]
pub enum EnvEdit {
    Unchanged,
    Edit { keep: Vec<String>, set: crate::sealed_env::EnvMap },
}

impl std::fmt::Debug for EnvEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unchanged => f.write_str("Unchanged"),
            Self::Edit { keep, set } => f
                .debug_struct("Edit")
                .field("keep", keep)
                .field("set", &set.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
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
    pub env: EnvEdit,
    /// `None` keeps the stored template.
    pub template: Option<SpawnTemplate>,
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

fn account_pick(req: &LaunchPresetRequest) -> Result<AccountPick, String> {
    match (req.account_id, req.pool_id, req.no_account) {
        (None, None, false) => Ok(AccountPick::Auto),
        (Some(id), None, false) => Ok(AccountPick::Account(id)),
        (None, Some(id), false) => Ok(AccountPick::Pool(id)),
        (None, None, true) => Ok(AccountPick::Unbound),
        _ => Err("pick at most one of account_id, pool_id and no_account".into()),
    }
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
        account: account_pick(req)?,
    })
}

fn container_target(req: &LaunchPresetRequest) -> Result<Target, String> {
    let Some(dispatcher_id) = req.dispatcher_id else {
        return Err("a preset needs a target: machine_id, or dispatcher_id and profile".into());
    };
    let profile =
        trimmed(req.profile.as_deref()).ok_or("profile is required for a dispatcher target")?;
    let machine_only = [&req.working_dir, &req.adapter_id, &req.model];
    let account = req.account_id.is_some() || req.pool_id.is_some() || req.no_account;
    if machine_only.iter().any(|f| trimmed(f.as_deref()).is_some())
        || req.permission_mode.is_some()
        || account
    {
        return Err("working_dir, adapter_id, model, permission_mode and the account pick apply \
                    to machine targets only"
            .into());
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

/// A dispatcher refuses caller env that redirects egress; say so at save time.
fn env_edit(target: &Target, req: &LaunchPresetRequest) -> Result<EnvEdit, String> {
    if req.env_keys.is_none() && req.env.is_none() {
        return Ok(EnvEdit::Unchanged);
    }
    let keep = req.env_keys.clone().unwrap_or_default();
    let set = req.env.as_ref().map_or_else(crate::sealed_env::EnvMap::new, |e| e.0.clone());
    crate::sealed_env::check(keep.iter().map(String::as_str), &set)?;
    if matches!(target, Target::Container { .. })
        && let Some(name) = keep
            .iter()
            .chain(set.keys())
            .find(|n| n.ends_with("_BASE_URL") || n.starts_with("WORKER_NET_"))
    {
        return Err(format!("env {name} is reserved on a dispatcher target"));
    }
    Ok(EnvEdit::Edit { keep, set })
}

/// Blank strings mean the harness default; stored files are never set here.
fn template(target: &Target, raw: Option<&SpawnTemplate>) -> Result<Option<SpawnTemplate>, String> {
    let Some(raw) = raw else { return Ok(None) };
    let mut label_ids: Vec<String> = Vec::with_capacity(raw.label_ids.len());
    for id in raw.label_ids.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if !label_ids.iter().any(|kept| kept == id) {
            label_ids.push(id.to_owned());
        }
    }
    let context = raw.context.clone().filter(|c| !c.items.is_empty() || c.auto);
    if label_ids.len() > MAX_TEMPLATE_LIST
        || context.as_ref().is_some_and(|c| c.items.len() > MAX_TEMPLATE_LIST)
    {
        return Err(format!("at most {MAX_TEMPLATE_LIST} labels and context items"));
    }
    let template = SpawnTemplate {
        effort: trimmed(raw.effort.as_deref()),
        service_tier: trimmed(raw.service_tier.as_deref()),
        label_ids,
        context,
        profile_id: raw.profile_id,
        auto_archive: raw.auto_archive,
        attachment_names: Vec::new(),
    };
    if matches!(target, Target::Container { .. }) && !template.is_blank() {
        return Err("effort, service tier, labels, context, profile and auto-archive apply to \
                    machine targets only"
            .into());
    }
    Ok(Some(template))
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
    let env = env_edit(&target, req)?;
    let template = template(&target, req.template.as_ref())?;
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
        env,
        template,
    })
}

#[cfg(test)]
mod tests {
    use cctui_proto::adapter::PermissionMode;
    use cctui_proto::api::presets::{LaunchPresetRequest, OnStale, Overlap, SpawnTemplate};
    use uuid::Uuid;

    use super::{AccountPick, EnvEdit, Target, validate};

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
    fn a_machine_preset_picks_at_most_one_account_and_defaults_to_auto() {
        let pick = |req: LaunchPresetRequest| match validate(&req).map(|v| v.target) {
            Ok(Target::Machine { account, .. }) => Ok(account),
            Ok(Target::Container { .. }) => panic!("machine target"),
            Err(e) => Err(e),
        };
        let id = Uuid::from_u128(7);
        assert_eq!(pick(machine()), Ok(AccountPick::Auto));
        assert_eq!(
            pick(LaunchPresetRequest { account_id: Some(id), ..machine() }),
            Ok(AccountPick::Account(id))
        );
        assert_eq!(
            pick(LaunchPresetRequest { pool_id: Some(id), ..machine() }),
            Ok(AccountPick::Pool(id))
        );
        assert_eq!(
            pick(LaunchPresetRequest { no_account: true, ..machine() }),
            Ok(AccountPick::Unbound)
        );
        let two = LaunchPresetRequest { account_id: Some(id), no_account: true, ..machine() };
        assert!(pick(two).unwrap_err().contains("at most one"));
        let on_container = LaunchPresetRequest { pool_id: Some(id), ..container() };
        assert!(validate(&on_container).unwrap_err().contains("machine targets only"));
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

    fn env(pairs: &[(&str, &str)]) -> cctui_proto::api::presets::EnvValues {
        cctui_proto::api::presets::EnvValues(
            pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        )
    }

    #[test]
    fn env_is_left_alone_unless_the_request_names_it() {
        assert_eq!(validate(&machine()).unwrap().env, EnvEdit::Unchanged);
        let keys_only = LaunchPresetRequest { env_keys: Some(vec!["A".into()]), ..machine() };
        assert_eq!(
            validate(&keys_only).unwrap().env,
            EnvEdit::Edit { keep: vec!["A".into()], set: crate::sealed_env::EnvMap::new() }
        );
        let values_only = LaunchPresetRequest { env: Some(env(&[("B", "v")])), ..machine() };
        let EnvEdit::Edit { keep, set } = validate(&values_only).unwrap().env else {
            panic!("an edit")
        };
        assert!(keep.is_empty());
        assert_eq!(set.get("B").map(String::as_str), Some("v"));
        let cleared = LaunchPresetRequest { env_keys: Some(Vec::new()), ..machine() };
        assert!(matches!(validate(&cleared).unwrap().env, EnvEdit::Edit { .. }));
    }

    #[test]
    fn env_names_are_checked_and_values_stay_out_of_debug() {
        let bad = LaunchPresetRequest { env_keys: Some(vec!["lower".into()]), ..machine() };
        assert!(validate(&bad).unwrap_err().contains("invalid env key"));
        let bad_value_key = LaunchPresetRequest { env: Some(env(&[("1X", "v")])), ..machine() };
        assert!(validate(&bad_value_key).is_err());
        let ok = LaunchPresetRequest { env: Some(env(&[("TOKEN", "s3cr3t-v4lue")])), ..machine() };
        let valid = validate(&ok).unwrap();
        assert!(!format!("{valid:?}").contains("s3cr3t-v4lue"), "{valid:?}");
        assert!(format!("{valid:?}").contains("TOKEN"));
    }

    #[test]
    fn a_dispatcher_target_carries_env_but_not_a_reserved_name() {
        let carried = LaunchPresetRequest { env: Some(env(&[("FEED_TOKEN", "t")])), ..container() };
        assert!(matches!(validate(&carried).unwrap().env, EnvEdit::Edit { .. }));
        for name in ["ANTHROPIC_BASE_URL", "WORKER_NET_PROXY"] {
            let reserved = LaunchPresetRequest { env: Some(env(&[(name, "x")])), ..container() };
            assert!(validate(&reserved).unwrap_err().contains("reserved"), "{name}");
        }
    }

    #[test]
    fn a_template_is_kept_unless_sent_and_normalised_when_it_is() {
        assert_eq!(validate(&machine()).unwrap().template, None);
        let sent = LaunchPresetRequest {
            template: Some(SpawnTemplate {
                effort: Some(" high ".into()),
                service_tier: Some(" ".into()),
                label_ids: vec!["l1".into(), " l1 ".into(), String::new(), "l2".into()],
                context: Some(cctui_proto::api::SpawnContext { items: Vec::new(), auto: false }),
                auto_archive: true,
                attachment_names: vec!["forged.md".into()],
                ..SpawnTemplate::default()
            }),
            ..machine()
        };
        let t = validate(&sent).unwrap().template.expect("sent");
        assert_eq!(t.effort.as_deref(), Some("high"));
        assert_eq!(t.service_tier, None);
        assert_eq!(t.label_ids, ["l1", "l2"]);
        assert_eq!(t.context, None, "an empty pick is no context");
        assert!(t.auto_archive);
        assert!(t.attachment_names.is_empty(), "files are set by their own route");
    }

    #[test]
    fn a_dispatcher_target_takes_a_blank_template_only() {
        let blank = LaunchPresetRequest { template: Some(SpawnTemplate::default()), ..container() };
        assert!(validate(&blank).is_ok(), "a GET of a dispatcher preset round-trips");
        let effort = LaunchPresetRequest {
            template: Some(SpawnTemplate {
                effort: Some("high".into()),
                ..SpawnTemplate::default()
            }),
            ..container()
        };
        assert!(validate(&effort).unwrap_err().contains("machine targets only"));
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
