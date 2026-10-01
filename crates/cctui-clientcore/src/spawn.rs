//! Turning spawn-form choices into a [`SpawnRequest`].
//!
//! The web UI's `spawn/spawnBody.ts` is the other implementation of this rule.
//! Both are replayed against `fixtures/parity/spawnBody.json`, so the TUI and
//! the browser cannot send different requests for the same choices.

use std::collections::BTreeMap;

use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::{SpawnContext, SpawnRequest};

/// Picker value meaning "run on the machine's own login". Outside the
/// account-name space so a real account can never collide with it.
pub const NO_ACCOUNT: &str = "\x00no-account";

/// Picker prefix meaning "bind inside this pool".
pub const POOL_PREFIX: &str = "\x00pool:";

/// `followup`: the first prompt embeds the parent's brief.
pub const FOLLOWUP_RELATION: &str = "followup";

#[must_use]
pub fn pool_value(name: &str) -> String {
    format!("{POOL_PREFIX}{name}")
}

/// The pool behind a picker value, or `None` when it names an account, Auto or
/// no-account.
#[must_use]
pub fn pool_name(value: &str) -> Option<&str> {
    value.strip_prefix(POOL_PREFIX)
}

/// An account whose provider serves an OpenAI-compatible endpoint: its declared
/// models replace the per-adapter family lists.
#[must_use]
pub fn is_compatible_provider(provider: &str) -> bool {
    provider.ends_with("-compatible")
}

/// Trailing slashes off, but a bare root stays `/`.
#[must_use]
pub fn normalize_dir(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches('/');
    if stripped.is_empty() { "/".to_owned() } else { stripped.to_owned() }
}

/// The form fields that reach the request. Per-adapter model and effort are
/// separate fields on purpose: switching harness must not lose the other one's
/// choice, which is what the web UI's form does too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnFields {
    pub machine_id: String,
    pub working_dir: String,
    pub name: String,
    pub prompt: String,
    pub adapter_id: String,
    /// Empty leaves the account's default mode to the server.
    pub permission_mode: String,
    pub model_claude: String,
    pub model_codex: String,
    /// Used instead of the family lists when the account is compatible-endpoint.
    pub model_account: String,
    pub effort_claude: String,
    pub effort_codex: String,
    /// Codex only; empty leaves the account's setting.
    pub service_tier: String,
    /// Empty is Auto, [`NO_ACCOUNT`] is the machine's own login, a
    /// [`POOL_PREFIX`] value is a pool, anything else is an account name.
    pub account: String,
    /// Disambiguates an account name shared across providers. A caller that
    /// knows the live provider passes it to [`build_spawn_body`] instead; this
    /// field is what a profile or a draft writes.
    pub account_provider: String,
    pub labels: Vec<String>,
    pub context_items: Vec<String>,
    pub context_auto: bool,
}

/// Empty string to `None`, trimming first.
fn some_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) }
}

fn some_value(value: &str) -> Option<String> {
    if value.is_empty() { None } else { Some(value.to_owned()) }
}

/// The model the request carries: a compatible-endpoint account drives the
/// picker from its own declared models, so that field wins over the
/// per-adapter family lists.
#[must_use]
pub fn model_of(fields: &SpawnFields, spawn_provider: Option<&str>) -> Option<String> {
    let compatible = spawn_provider.is_some_and(is_compatible_provider);
    if compatible {
        return some_value(&fields.model_account);
    }
    some_value(if fields.adapter_id == "codex" {
        &fields.model_codex
    } else {
        &fields.model_claude
    })
}

/// The one place form choices become a request.
///
/// `spawn_provider` disambiguates an account name shared across providers.
/// `env` is already reduced to complete rows; `followup_parent` and `profile_id`
/// come from whatever opened the form.
#[must_use]
pub fn build_spawn_body(
    fields: &SpawnFields,
    spawn_provider: Option<&str>,
    env: BTreeMap<String, String>,
    followup_parent: Option<&str>,
    profile_id: Option<uuid::Uuid>,
) -> SpawnRequest {
    // A caller that knows the live pick wins; the field is the stored fallback a
    // profile or draft wrote.
    let spawn_provider =
        spawn_provider.or_else(|| Some(fields.account_provider.as_str()).filter(|p| !p.is_empty()));
    let no_account = fields.account == NO_ACCOUNT;
    let pool = pool_name(&fields.account).map(str::to_owned);
    let named_account = some_trimmed(&fields.account);
    let bound = no_account || pool.is_some();
    let codex = fields.adapter_id == "codex";

    SpawnRequest {
        machine_id: fields.machine_id.clone(),
        working_dir: normalize_dir(fields.working_dir.trim()),
        adapter_id: some_value(&fields.adapter_id),
        name: some_trimmed(&fields.name),
        prompt: some_trimmed(&fields.prompt),
        prompt_name: None,
        permission_mode: parse_mode(&fields.permission_mode),
        effort: some_value(if codex { &fields.effort_codex } else { &fields.effort_claude }),
        service_tier: if codex { some_value(&fields.service_tier) } else { None },
        model: model_of(fields, spawn_provider),
        env,
        account: if bound { None } else { named_account.clone() },
        provider: if bound { None } else { spawn_provider.map(str::to_owned) },
        no_account,
        // Auto hands the choice to the server; a pool is the bounded form of it.
        auto_account: !no_account && pool.is_none() && named_account.is_none(),
        pool,
        save_draft: false,
        auto_archive: false,
        env_keys: Vec::new(),
        attachment_names: Vec::new(),
        label_ids: fields.labels.clone(),
        spawn_capability: None,
        relation: followup_parent.map(|_| FOLLOWUP_RELATION.to_owned()),
        parent_session_id: followup_parent.map(str::to_owned),
        context: Some(SpawnContext {
            items: fields.context_items.clone(),
            auto: fields.context_auto,
        }),
        profile_id,
    }
}

fn parse_mode(value: &str) -> Option<PermissionMode> {
    match value {
        "ask" => Some(PermissionMode::Ask),
        "auto" => Some(PermissionMode::Auto),
        "yolo" => Some(PermissionMode::Yolo),
        "whip" => Some(PermissionMode::Whip),
        _ => None,
    }
}

/// Complete rows only: a key with no value is still being typed.
#[must_use]
pub fn env_map(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter()
        .filter_map(|(key, value)| {
            let key = key.trim();
            (!key.is_empty() && !value.is_empty()).then(|| (key.to_owned(), value.clone()))
        })
        .collect()
}

/// A draft carries env var *names* and attachment names, never values or bytes.
#[must_use]
pub fn draft_body(
    spawn: &SpawnRequest,
    env_keys: Vec<String>,
    attachment_names: Vec<String>,
) -> SpawnRequest {
    SpawnRequest {
        env: BTreeMap::new(),
        env_keys,
        attachment_names,
        save_draft: true,
        ..spawn.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        NO_ACCOUNT, SpawnFields, build_spawn_body, draft_body, is_compatible_provider,
        normalize_dir, pool_name, pool_value,
    };

    fn fields() -> SpawnFields {
        SpawnFields {
            machine_id: "m-1".to_owned(),
            working_dir: "/home/dev/cctui".to_owned(),
            adapter_id: "claude-code".to_owned(),
            ..SpawnFields::default()
        }
    }

    fn body(f: &SpawnFields) -> cctui_proto::api::SpawnRequest {
        build_spawn_body(f, None, BTreeMap::new(), None, None)
    }

    #[test]
    fn a_trailing_slash_goes_but_the_root_stays() {
        assert_eq!(normalize_dir("/home/dev/"), "/home/dev");
        assert_eq!(normalize_dir("/home/dev///"), "/home/dev");
        assert_eq!(normalize_dir("/"), "/");
        assert_eq!(normalize_dir("///"), "/");
        assert_eq!(normalize_dir(""), "");
    }

    #[test]
    fn an_empty_field_becomes_none_rather_than_an_empty_string() {
        let b = body(&fields());
        assert_eq!(b.name, None);
        assert_eq!(b.prompt, None);
        assert_eq!(b.model, None);
        assert_eq!(b.effort, None);
        assert_eq!(b.permission_mode, None, "the server resolves the account default");
        assert_eq!(b.account, None);
    }

    #[test]
    fn whitespace_only_text_counts_as_empty() {
        let mut f = fields();
        f.name = "   ".to_owned();
        f.prompt = "\n\t ".to_owned();
        let b = body(&f);
        assert_eq!(b.name, None);
        assert_eq!(b.prompt, None);
    }

    #[test]
    fn the_model_and_effort_follow_the_chosen_harness() {
        let mut f = fields();
        f.model_claude = "opus".to_owned();
        f.model_codex = "gpt-5.6".to_owned();
        f.effort_claude = "high".to_owned();
        f.effort_codex = "medium".to_owned();

        let claude = body(&f);
        assert_eq!(claude.model.as_deref(), Some("opus"));
        assert_eq!(claude.effort.as_deref(), Some("high"));

        f.adapter_id = "codex".to_owned();
        let codex = body(&f);
        assert_eq!(codex.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(codex.effort.as_deref(), Some("medium"));
        assert_eq!(
            f.model_claude, "opus",
            "switching harness must not lose the other one's choice"
        );
    }

    #[test]
    fn the_service_tier_is_codex_only() {
        let mut f = fields();
        f.service_tier = "fast".to_owned();
        assert_eq!(body(&f).service_tier, None, "claude-code carries no tier");
        f.adapter_id = "codex".to_owned();
        assert_eq!(body(&f).service_tier.as_deref(), Some("fast"));
    }

    #[test]
    fn a_compatible_account_drives_the_model_instead_of_the_family_lists() {
        let mut f = fields();
        f.model_claude = "opus".to_owned();
        f.model_account = "llama-3.3-70b".to_owned();
        let plain = build_spawn_body(&f, Some("anthropic"), BTreeMap::new(), None, None);
        assert_eq!(plain.model.as_deref(), Some("opus"));

        let compatible =
            build_spawn_body(&f, Some("fireworks-compatible"), BTreeMap::new(), None, None);
        assert_eq!(compatible.model.as_deref(), Some("llama-3.3-70b"));
        assert!(is_compatible_provider("fireworks-compatible"));
        assert!(!is_compatible_provider("anthropic"));
    }

    #[test]
    fn a_blank_account_is_auto_and_the_server_chooses() {
        let b = body(&fields());
        assert!(b.auto_account);
        assert!(!b.no_account);
        assert_eq!(b.account, None);
        assert_eq!(b.pool, None);
    }

    #[test]
    fn a_named_account_carries_its_provider_and_is_not_auto() {
        let mut f = fields();
        f.account = "main".to_owned();
        let b = build_spawn_body(&f, Some("anthropic"), BTreeMap::new(), None, None);
        assert_eq!(b.account.as_deref(), Some("main"));
        assert_eq!(b.provider.as_deref(), Some("anthropic"));
        assert!(!b.auto_account);
        assert!(!b.no_account);
    }

    #[test]
    fn the_no_account_sentinel_drops_the_account_and_its_provider() {
        let mut f = fields();
        f.account = NO_ACCOUNT.to_owned();
        let b = build_spawn_body(&f, Some("anthropic"), BTreeMap::new(), None, None);
        assert!(b.no_account);
        assert!(!b.auto_account);
        assert_eq!(b.account, None);
        assert_eq!(b.provider, None, "an ambient login has no provider to route");
    }

    #[test]
    fn a_pool_pick_is_bounded_auto() {
        let mut f = fields();
        f.account = pool_value("p-1");
        let b = build_spawn_body(&f, Some("anthropic"), BTreeMap::new(), None, None);
        assert_eq!(b.pool.as_deref(), Some("p-1"));
        assert_eq!(b.account, None);
        assert_eq!(b.provider, None);
        assert!(!b.auto_account, "the pool already bounds the choice");
        assert!(!b.no_account);
        assert_eq!(pool_name("main"), None);
    }

    #[test]
    fn labels_and_context_travel_as_they_are() {
        let mut f = fields();
        f.labels = vec!["l-1".to_owned(), "l-2".to_owned()];
        f.context_items = vec!["notes".to_owned()];
        f.context_auto = true;
        let b = body(&f);
        assert_eq!(b.label_ids, ["l-1", "l-2"]);
        let context = b.context.expect("a context block");
        assert_eq!(context.items, ["notes"]);
        assert!(context.auto);
    }

    #[test]
    fn a_followup_names_its_parent_and_its_relation() {
        let b = build_spawn_body(&fields(), None, BTreeMap::new(), Some("s-parent"), None);
        assert_eq!(b.relation.as_deref(), Some("followup"));
        assert_eq!(b.parent_session_id.as_deref(), Some("s-parent"));

        let root = body(&fields());
        assert_eq!(root.relation, None);
        assert_eq!(root.parent_session_id, None);
    }

    #[test]
    fn every_permission_mode_round_trips_and_junk_is_the_default() {
        for (text, mode) in [
            ("ask", cctui_proto::adapter::PermissionMode::Ask),
            ("auto", cctui_proto::adapter::PermissionMode::Auto),
            ("yolo", cctui_proto::adapter::PermissionMode::Yolo),
            ("whip", cctui_proto::adapter::PermissionMode::Whip),
        ] {
            let mut f = fields();
            f.permission_mode = text.to_owned();
            assert_eq!(body(&f).permission_mode, Some(mode));
        }
        let mut f = fields();
        f.permission_mode = "sideways".to_owned();
        assert_eq!(body(&f).permission_mode, None);
    }

    #[test]
    fn an_env_row_counts_only_once_both_halves_are_there() {
        let rows = vec![
            ("TOKEN".to_owned(), "secret".to_owned()),
            ("  SPACED  ".to_owned(), "v".to_owned()),
            ("NOVALUE".to_owned(), String::new()),
            (String::new(), "orphan".to_owned()),
        ];
        let map = super::env_map(&rows);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("TOKEN").map(String::as_str), Some("secret"));
        assert_eq!(map.get("SPACED").map(String::as_str), Some("v"), "the key is trimmed");
    }

    #[test]
    fn the_provider_field_stands_in_when_the_caller_passes_none() {
        let mut f = fields();
        f.account = "main".to_owned();
        f.account_provider = "openai".to_owned();
        assert_eq!(
            build_spawn_body(&f, None, BTreeMap::new(), None, None).provider.as_deref(),
            Some("openai")
        );
        assert_eq!(
            build_spawn_body(&f, Some("anthropic"), BTreeMap::new(), None, None)
                .provider
                .as_deref(),
            Some("anthropic"),
            "a live pick wins over the stored one"
        );
    }

    #[test]
    fn env_reaches_the_request_but_a_draft_keeps_only_the_names() {
        let mut env = BTreeMap::new();
        env.insert("TOKEN".to_owned(), "secret".to_owned());
        let spawn = build_spawn_body(&fields(), None, env, None, None);
        assert_eq!(spawn.env.get("TOKEN").map(String::as_str), Some("secret"));
        assert!(!spawn.save_draft);

        let draft = draft_body(&spawn, vec!["TOKEN".to_owned()], vec!["notes.md".to_owned()]);
        assert!(draft.env.is_empty(), "a draft never stores a value");
        assert_eq!(draft.env_keys, ["TOKEN"]);
        assert_eq!(draft.attachment_names, ["notes.md"]);
        assert!(draft.save_draft);
        assert_eq!(draft.machine_id, spawn.machine_id);
    }
}
