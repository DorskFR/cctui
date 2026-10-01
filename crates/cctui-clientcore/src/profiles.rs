//! Spawn-profile rules, shared with the web UI's `spawn/profiles.ts`.
//!
//! The form they read and write is [`crate::spawn::SpawnFields`]; the picker
//! value space (`NO_ACCOUNT`, pools) is [`crate::spawn`]'s too.
//!
//! A profile carries the compute knobs only: harness, account binding, model,
//! effort, permission mode, service tier. Everything else about a spawn — where
//! it runs, the prompt, labels, env — stays whatever the form already held.

use crate::spawn::{NO_ACCOUNT, SpawnFields, is_compatible_provider, pool_name, pool_value};

/// The knobs a profile stores. `None` means "leave it to the harness or the
/// account"; at most one of `account_id` / `pool_id` / `no_account` is set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileSpec {
    pub harness: String,
    pub account_id: Option<String>,
    pub pool_id: Option<String>,
    pub no_account: bool,
    pub model_alias: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub service_tier: Option<String>,
}

/// An account as the rules need to see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountRef {
    pub id: String,
    pub name: String,
    pub emoji: Option<String>,
    pub providers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolRef {
    pub id: String,
    pub name: String,
}

/// Wording the chain line borrows from the caller, so the TUI and the web can
/// each use their own.
#[derive(Clone, Copy, Debug)]
pub struct ChainLabels<'a> {
    pub auto: &'a str,
    pub no_account: &'a str,
    pub default_model: &'a str,
    pub default_effort: &'a str,
    pub default_mode: &'a str,
}

/// Which form model field a harness reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelField {
    Account,
    Codex,
    Claude,
}

#[must_use]
pub fn adapter_label(adapter: &str) -> &'static str {
    if adapter == "codex" { "Codex" } else { "Claude Code" }
}

/// Mirrors the server's provider → harness family mapping.
#[must_use]
pub fn adapter_for_provider(provider: &str) -> &'static str {
    if provider == "fireworks" {
        "opencode"
    } else if provider.contains("openai") {
        "codex"
    } else {
        "claude-code"
    }
}

/// The provider credential backing a harness on this account, if any.
#[must_use]
pub fn provider_for_adapter<'a>(account: Option<&'a AccountRef>, adapter: &str) -> Option<&'a str> {
    account?.providers.iter().map(String::as_str).find(|p| adapter_for_provider(p) == adapter)
}

#[must_use]
pub fn model_field(harness: &str, account: Option<&AccountRef>) -> ModelField {
    match provider_for_adapter(account, harness) {
        Some(provider) if is_compatible_provider(provider) => ModelField::Account,
        _ if harness == "codex" => ModelField::Codex,
        _ => ModelField::Claude,
    }
}

fn blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[must_use]
pub fn account_by_name<'a>(accounts: &'a [AccountRef], name: &str) -> Option<&'a AccountRef> {
    (!name.is_empty()).then(|| accounts.iter().find(|a| a.name == name)).flatten()
}

#[must_use]
pub fn account_by_id<'a>(accounts: &'a [AccountRef], id: Option<&str>) -> Option<&'a AccountRef> {
    let id = id?;
    accounts.iter().find(|a| a.id == id)
}

fn pool_by_id<'a>(pools: &'a [PoolRef], id: Option<&str>) -> Option<&'a PoolRef> {
    let id = id?;
    pools.iter().find(|p| p.id == id)
}

/// The form's account value for a spec: empty for Auto, the no-account
/// sentinel, a pool value, or the account name. A pool or account that no
/// longer exists falls back to Auto.
#[must_use]
pub fn account_pick(spec: &ProfileSpec, accounts: &[AccountRef], pools: &[PoolRef]) -> String {
    if spec.no_account {
        return NO_ACCOUNT.to_owned();
    }
    if let Some(pool) = pool_by_id(pools, spec.pool_id.as_deref()) {
        return pool_value(&pool.name);
    }
    account_by_id(accounts, spec.account_id.as_deref()).map(|a| a.name.clone()).unwrap_or_default()
}

/// The knobs a form is currently set to — the seed for a new profile.
#[must_use]
pub fn spec_from_form(
    form: &SpawnFields,
    accounts: &[AccountRef],
    pools: &[PoolRef],
) -> ProfileSpec {
    let harness =
        if form.adapter_id.is_empty() { "claude-code".to_owned() } else { form.adapter_id.clone() };
    let pool = pool_name(&form.account);
    let account = if pool.is_none() { account_by_name(accounts, &form.account) } else { None };
    let model = match model_field(&harness, account) {
        ModelField::Account => &form.model_account,
        ModelField::Codex => &form.model_codex,
        ModelField::Claude => &form.model_claude,
    };
    ProfileSpec {
        account_id: account.map(|a| a.id.clone()),
        pool_id: pool.and_then(|name| pools.iter().find(|p| p.name == name).map(|p| p.id.clone())),
        no_account: form.account == NO_ACCOUNT,
        model_alias: blank(model),
        effort: blank(if harness == "codex" { &form.effort_codex } else { &form.effort_claude }),
        permission_mode: blank(&form.permission_mode),
        service_tier: if harness == "codex" { blank(&form.service_tier) } else { None },
        harness,
    }
}

/// `form` with the profile's knobs written over it; every other field of the
/// caller's form is its own business.
#[must_use]
pub fn apply_spec(
    form: &SpawnFields,
    spec: &ProfileSpec,
    accounts: &[AccountRef],
    pools: &[PoolRef],
) -> SpawnFields {
    let account = account_by_id(accounts, spec.account_id.as_deref());
    let mut out = form.clone();
    out.adapter_id.clone_from(&spec.harness);
    out.account = account_pick(spec, accounts, pools);
    provider_for_adapter(account, &spec.harness)
        .unwrap_or_default()
        .clone_into(&mut out.account_provider);
    out.permission_mode = spec.permission_mode.clone().unwrap_or_default();
    out.service_tier = spec.service_tier.clone().unwrap_or_default();
    let model = spec.model_alias.clone().unwrap_or_default();
    match model_field(&spec.harness, account) {
        ModelField::Account => out.model_account = model,
        ModelField::Codex => out.model_codex = model,
        ModelField::Claude => out.model_claude = model,
    }
    let effort = spec.effort.clone().unwrap_or_default();
    if spec.harness == "codex" {
        out.effort_codex = effort;
    } else {
        out.effort_claude = effort;
    }
    out
}

/// How many knobs differ between two specs — the adjust panel's change count.
#[must_use]
pub fn spec_changes(a: &ProfileSpec, b: &ProfileSpec) -> usize {
    usize::from(a.harness != b.harness)
        + usize::from(a.account_id != b.account_id)
        + usize::from(a.pool_id != b.pool_id)
        + usize::from(a.no_account != b.no_account)
        + usize::from(a.model_alias != b.model_alias)
        + usize::from(a.effort != b.effort)
        + usize::from(a.permission_mode != b.permission_mode)
        + usize::from(a.service_tier != b.service_tier)
}

#[must_use]
pub fn same_spec(a: &ProfileSpec, b: &ProfileSpec) -> bool {
    spec_changes(a, b) == 0
}

/// The one-line summary under a profile's name.
#[must_use]
pub fn spec_chain(
    spec: &ProfileSpec,
    accounts: &[AccountRef],
    pools: &[PoolRef],
    labels: ChainLabels<'_>,
    model_label: &dyn Fn(&str, &str) -> String,
) -> String {
    let account = account_by_id(accounts, spec.account_id.as_deref());
    let pool = pool_by_id(pools, spec.pool_id.as_deref());
    let account_text = if spec.no_account {
        labels.no_account.to_owned()
    } else if let Some(pool) = pool {
        pool.name.clone()
    } else if let Some(account) = account {
        account
            .emoji
            .as_deref()
            .filter(|e| !e.is_empty())
            .map_or_else(|| account.name.clone(), |emoji| format!("{emoji} {}", account.name))
    } else {
        labels.auto.to_owned()
    };
    let mode = match spec.permission_mode.as_deref() {
        Some(mode) if !mode.is_empty() => capitalize(mode),
        _ => labels.default_mode.to_owned(),
    };
    let model = spec
        .model_alias
        .as_deref()
        .map_or_else(|| labels.default_model.to_owned(), |alias| model_label(&spec.harness, alias));
    let effort = spec.effort.clone().unwrap_or_else(|| labels.default_effort.to_owned());
    [adapter_label(&spec.harness).to_owned(), account_text, model, effort, mode].join(" · ")
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |first| first.to_uppercase().collect::<String>() + chars.as_str())
}

/// `base`, else `base 2`, `base 3`… — whatever the list lacks.
#[must_use]
pub fn unique_profile_name(base: &str, existing: &[String]) -> String {
    let taken: Vec<String> = existing.iter().map(|n| n.to_lowercase()).collect();
    if !taken.contains(&base.to_lowercase()) {
        return base.to_owned();
    }
    for i in 2.. {
        let candidate = format!("{base} {i}");
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("the search is unbounded")
}

/// Which profile the strip opens on: the machine's last-used one while it
/// still exists, else the first.
#[must_use]
pub fn initial_profile(ids: &[String], last_used: Option<&str>) -> Option<String> {
    last_used
        .filter(|id| ids.iter().any(|known| known == id))
        .map(str::to_owned)
        .or_else(|| ids.first().cloned())
}

/// Move `id` to `index` (clamped). An id the list lacks changes nothing.
#[must_use]
pub fn move_profile(ids: &[String], id: &str, index: usize) -> Vec<String> {
    let mut next = ids.to_vec();
    let Some(from) = next.iter().position(|known| known == id) else { return next };
    let row = next.remove(from);
    next.insert(index.min(next.len()), row);
    next
}

/// Drop `id` onto `target`: it takes the target's slot.
#[must_use]
pub fn move_profile_onto(ids: &[String], id: &str, target: &str) -> Vec<String> {
    let Some(to) = ids.iter().position(|known| known == target) else { return ids.to_vec() };
    if id == target {
        return ids.to_vec();
    }
    move_profile(ids, id, to)
}

#[cfg(test)]
mod tests {
    use super::{
        AccountRef, ChainLabels, ModelField, NO_ACCOUNT, PoolRef, ProfileSpec, SpawnFields,
        account_pick, adapter_for_provider, apply_spec, initial_profile, model_field, move_profile,
        move_profile_onto, pool_value, same_spec, spec_chain, spec_changes, spec_from_form,
        unique_profile_name,
    };

    fn account(id: &str, name: &str, providers: &[&str]) -> AccountRef {
        AccountRef {
            id: id.to_owned(),
            name: name.to_owned(),
            emoji: None,
            providers: providers.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn accounts() -> Vec<AccountRef> {
        vec![
            AccountRef {
                emoji: Some("🐼".to_owned()),
                ..account("a1", "personal", &["anthropic"])
            },
            account("a2", "compat", &["anthropic-compatible"]),
            account("a3", "oai", &["openai"]),
        ]
    }

    fn pools() -> Vec<PoolRef> {
        vec![PoolRef { id: "pool1".to_owned(), name: "shared".to_owned() }]
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_provider_maps_to_the_harness_that_can_run_it() {
        assert_eq!(adapter_for_provider("anthropic"), "claude-code");
        assert_eq!(adapter_for_provider("anthropic-compatible"), "claude-code");
        assert_eq!(adapter_for_provider("openai"), "codex");
        assert_eq!(adapter_for_provider("openai-compatible"), "codex");
        assert_eq!(adapter_for_provider("fireworks"), "opencode");
    }

    #[test]
    fn a_compatible_endpoint_account_drives_its_own_model_list() {
        let accounts = accounts();
        assert_eq!(model_field("claude-code", None), ModelField::Claude);
        assert_eq!(model_field("codex", None), ModelField::Codex);
        assert_eq!(model_field("claude-code", Some(&accounts[0])), ModelField::Claude);
        assert_eq!(model_field("claude-code", Some(&accounts[1])), ModelField::Account);
        assert_eq!(model_field("codex", Some(&accounts[2])), ModelField::Codex);
    }

    #[test]
    fn the_account_value_names_a_pool_an_account_or_auto() {
        let (accounts, pools) = (accounts(), pools());
        let spec = ProfileSpec { harness: "claude-code".to_owned(), ..ProfileSpec::default() };
        assert_eq!(account_pick(&spec, &accounts, &pools), "");
        assert_eq!(
            account_pick(&ProfileSpec { no_account: true, ..spec.clone() }, &accounts, &pools),
            NO_ACCOUNT
        );
        assert_eq!(
            account_pick(
                &ProfileSpec { pool_id: Some("pool1".to_owned()), ..spec.clone() },
                &accounts,
                &pools
            ),
            pool_value("shared")
        );
        assert_eq!(
            account_pick(
                &ProfileSpec { account_id: Some("a1".to_owned()), ..spec.clone() },
                &accounts,
                &pools
            ),
            "personal"
        );
        assert_eq!(
            account_pick(
                &ProfileSpec { account_id: Some("gone".to_owned()), ..spec },
                &accounts,
                &pools
            ),
            "",
            "an account that no longer exists falls back to Auto"
        );
    }

    fn form() -> SpawnFields {
        SpawnFields {
            adapter_id: "claude-code".to_owned(),
            account: "personal".to_owned(),
            account_provider: "anthropic".to_owned(),
            model_claude: "fable".to_owned(),
            model_codex: "gpt-5.6".to_owned(),
            model_account: "llama".to_owned(),
            effort_claude: "medium".to_owned(),
            effort_codex: "high".to_owned(),
            permission_mode: "yolo".to_owned(),
            ..SpawnFields::default()
        }
    }

    #[test]
    fn a_spec_reads_the_field_its_harness_uses() {
        let (accounts, pools) = (accounts(), pools());
        let spec = spec_from_form(&form(), &accounts, &pools);
        assert_eq!(spec.harness, "claude-code");
        assert_eq!(spec.account_id.as_deref(), Some("a1"));
        assert_eq!(spec.model_alias.as_deref(), Some("fable"), "the claude field");
        assert_eq!(spec.effort.as_deref(), Some("medium"));
        assert_eq!(spec.service_tier, None, "service tier is codex-only");

        let codex = SpawnFields {
            adapter_id: "codex".to_owned(),
            account: "oai".to_owned(),
            service_tier: "fast".to_owned(),
            ..form()
        };
        let spec = spec_from_form(&codex, &accounts, &pools);
        assert_eq!(spec.model_alias.as_deref(), Some("gpt-5.6"));
        assert_eq!(spec.effort.as_deref(), Some("high"));
        assert_eq!(spec.service_tier.as_deref(), Some("fast"));

        let compat = SpawnFields { account: "compat".to_owned(), ..form() };
        assert_eq!(
            spec_from_form(&compat, &accounts, &pools).model_alias.as_deref(),
            Some("llama"),
            "a compatible endpoint reads the account's own model"
        );
    }

    #[test]
    fn a_pool_pick_round_trips_through_the_spec() {
        let (accounts, pools) = (accounts(), pools());
        let picked = SpawnFields { account: pool_value("shared"), ..form() };
        let spec = spec_from_form(&picked, &accounts, &pools);
        assert_eq!(spec.pool_id.as_deref(), Some("pool1"));
        assert_eq!(spec.account_id, None);
        assert_eq!(apply_spec(&form(), &spec, &accounts, &pools).account, pool_value("shared"));
    }

    #[test]
    fn applying_a_spec_leaves_every_other_field_alone() {
        let (accounts, pools) = (accounts(), pools());
        let spec = ProfileSpec {
            harness: "codex".to_owned(),
            account_id: Some("a3".to_owned()),
            model_alias: Some("gpt-5.6-sol".to_owned()),
            effort: Some("high".to_owned()),
            permission_mode: Some("yolo".to_owned()),
            service_tier: Some("fast".to_owned()),
            ..ProfileSpec::default()
        };
        let out = apply_spec(&form(), &spec, &accounts, &pools);
        assert_eq!(out.adapter_id, "codex");
        assert_eq!(out.account, "oai");
        assert_eq!(out.account_provider, "openai");
        assert_eq!(out.model_codex, "gpt-5.6-sol");
        assert_eq!(out.effort_codex, "high");
        assert_eq!(out.service_tier, "fast");
        assert_eq!(out.model_claude, "fable", "the claude model survives the switch");
        assert_eq!(out.effort_claude, "medium", "and so does its effort");
    }

    #[test]
    fn an_empty_spec_clears_the_fields_it_owns() {
        let (accounts, pools) = (accounts(), pools());
        let spec = ProfileSpec { harness: "claude-code".to_owned(), ..ProfileSpec::default() };
        let out = apply_spec(&form(), &spec, &accounts, &pools);
        assert_eq!(out.model_claude, "");
        assert_eq!(out.effort_claude, "");
        assert_eq!(out.permission_mode, "");
        assert_eq!(out.account, "");
    }

    #[test]
    fn a_form_and_its_own_spec_differ_in_nothing() {
        let (accounts, pools) = (accounts(), pools());
        let spec = spec_from_form(&form(), &accounts, &pools);
        let round =
            spec_from_form(&apply_spec(&form(), &spec, &accounts, &pools), &accounts, &pools);
        assert!(same_spec(&spec, &round));
        assert_eq!(spec_changes(&spec, &round), 0);
    }

    #[test]
    fn the_change_count_counts_every_knob_that_moved() {
        let base = ProfileSpec { harness: "claude-code".to_owned(), ..ProfileSpec::default() };
        let moved = ProfileSpec {
            harness: "codex".to_owned(),
            effort: Some("high".to_owned()),
            ..base.clone()
        };
        assert_eq!(spec_changes(&base, &moved), 2);
        assert!(!same_spec(&base, &moved));
    }

    #[test]
    fn the_chain_line_names_each_knob_or_its_default() {
        let (accounts, pools) = (accounts(), pools());
        let labels = ChainLabels {
            auto: "Auto",
            no_account: "none",
            default_model: "default model",
            default_effort: "default",
            default_mode: "Default",
        };
        let spec = ProfileSpec {
            harness: "claude-code".to_owned(),
            account_id: Some("a1".to_owned()),
            model_alias: Some("fable".to_owned()),
            effort: Some("medium".to_owned()),
            permission_mode: Some("yolo".to_owned()),
            ..ProfileSpec::default()
        };
        assert_eq!(
            spec_chain(&spec, &accounts, &pools, labels, &|_, alias| alias.to_owned()),
            "Claude Code · 🐼 personal · fable · medium · Yolo"
        );

        let bare = ProfileSpec { harness: "codex".to_owned(), ..ProfileSpec::default() };
        assert_eq!(
            spec_chain(&bare, &accounts, &pools, labels, &|_, alias| alias.to_owned()),
            "Codex · Auto · default model · default · Default"
        );

        let none = ProfileSpec { no_account: true, ..bare };
        assert_eq!(
            spec_chain(&none, &accounts, &pools, labels, &|_, alias| alias.to_owned()),
            "Codex · none · default model · default · Default"
        );
    }

    #[test]
    fn a_new_profile_takes_the_first_free_name() {
        assert_eq!(unique_profile_name("Default", &[]), "Default");
        assert_eq!(unique_profile_name("Default", &ids(&["Default"])), "Default 2");
        assert_eq!(
            unique_profile_name("Default", &ids(&["default", "Default 2"])),
            "Default 3",
            "the match is case-insensitive"
        );
    }

    #[test]
    fn the_strip_opens_on_the_machines_last_profile_while_it_exists() {
        let list = ids(&["p1", "p2"]);
        assert_eq!(initial_profile(&list, Some("p2")).as_deref(), Some("p2"));
        assert_eq!(initial_profile(&list, Some("gone")).as_deref(), Some("p1"));
        assert_eq!(initial_profile(&list, None).as_deref(), Some("p1"));
        assert_eq!(initial_profile(&[], Some("p1")), None);
    }

    #[test]
    fn reordering_moves_one_row_and_clamps() {
        let list = ids(&["a", "b", "c"]);
        assert_eq!(move_profile(&list, "a", 2), ids(&["b", "c", "a"]));
        assert_eq!(move_profile(&list, "c", 0), ids(&["c", "a", "b"]));
        assert_eq!(move_profile(&list, "a", 99), ids(&["b", "c", "a"]));
        assert_eq!(move_profile(&list, "gone", 0), list, "an unknown id changes nothing");
        assert_eq!(move_profile_onto(&list, "c", "a"), ids(&["c", "a", "b"]));
        assert_eq!(move_profile_onto(&list, "a", "a"), list);
        assert_eq!(move_profile_onto(&list, "a", "gone"), list);
    }
}
