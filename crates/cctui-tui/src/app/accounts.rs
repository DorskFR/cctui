//! The accounts slice: identities, their provider credentials, and the two
//! levers that drain one — pool eligibility and pool weight.
//!
//! Only `pool_eligible` and `pool_weight` are writable on an account identity
//! (`PATCH /accounts/{id}`); enabling/disabling a credential is not an API, so
//! "toggle" is the eligibility veto and "deprioritise" is the weight.
//!
//! Usage belongs to the usage lane: the text here comes from
//! `usage::account_summary`, and this slice draws no gauges and keeps no usage
//! state of its own.

use cctui_client::{Account, AccountRedirect};
use cctui_clientcore::accounts::{AccountRef, RedirectChip, RedirectRef, redirect_chips};

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// Which pane the keyboard is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Accounts,
    Pools,
}

/// A modal over the slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// `R`: spend a reset, naming the credit it would spend.
    ConfirmReset {
        provider_id: String,
        /// The credit upstream offered; `None` lets the server pick.
        credit_id: Option<String>,
        account: String,
        credit: String,
    },
    /// `x`: pick the account to redirect launches to.
    Redirect { account: String, targets: Vec<Target>, selected: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub name: String,
    /// Families both accounts have: one redirect rule is written per family.
    pub families: Vec<String>,
}

#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Accounts {
    pub rows: Vec<Account>,
    pub redirects: Vec<AccountRedirect>,
    pub selected: usize,
    pub focus: Focus,
    /// Whether the detail pane is open over the list.
    pub detail: bool,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    /// The server refused an edit: the slice stops offering them until the next
    /// successful write elsewhere.
    pub read_only: bool,
    pub mode: Option<Mode>,
}

impl Accounts {
    #[must_use]
    pub fn selected_row(&self) -> Option<&Account> {
        self.rows.get(self.selected)
    }

    /// The shape `cctui-clientcore` reads accounts as.
    #[must_use]
    pub fn refs(&self) -> Vec<AccountRef> {
        self.rows
            .iter()
            .map(|a| AccountRef {
                id: a.id.clone(),
                name: a.name.clone(),
                user_id: a.user_id.clone(),
                pool_eligible: a.pool_eligible,
            })
            .collect()
    }

    /// Redirect rules pointing this account's launches elsewhere.
    #[must_use]
    pub fn chips(&self, account_id: &str) -> Vec<RedirectChip> {
        let rules: Vec<RedirectRef> = self
            .redirects
            .iter()
            .map(|r| RedirectRef {
                id: r.id.clone(),
                from_account: r.from_account.clone(),
                to_account: r.to_account.clone(),
                family: r.family.clone(),
                expires_at: r.expires_at.map(|t| t.to_rfc3339()),
            })
            .collect();
        redirect_chips(&rules, &self.refs(), account_id)
    }

    /// Enabled over total, for the Overview tile.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        (self.rows.iter().filter(|a| a.pool_eligible).count(), self.rows.len())
    }
}

/// The keyboard context the accounts slice claims, or `None` to use the view's
/// own. A form in either pane outranks the pane itself.
#[must_use]
pub const fn key_context(app: &App) -> Option<crate::config::keymap::Context> {
    use crate::config::keymap::Context;
    if !matches!(app.slice, super::slice::Slice::Accounts) {
        return None;
    }
    if app.accounts.mode.is_some() || app.pools.form_open() {
        return Some(Context::AccountsForm);
    }
    if matches!(app.accounts.focus, Focus::Pools) {
        return Some(Context::AccountPools);
    }
    None
}

/// The usage lane's one-line reading for an account, or `None` when it has not
/// fetched or the credential reports no percentage window.
#[must_use]
pub fn usage_text(app: &App, account_id: &str) -> Option<String> {
    let id = uuid::Uuid::parse_str(account_id).ok()?;
    super::usage::account_summary(app, id)
}

/// Provider kinds as one cell: `anthropic`, or `anthropic+openai`.
#[must_use]
pub fn kinds_text(account: &Account) -> String {
    if account.providers.is_empty() {
        return "-".to_owned();
    }
    let mut families: Vec<&str> = Vec::new();
    for provider in &account.providers {
        if !families.contains(&provider.family.as_str()) {
            families.push(&provider.family);
        }
    }
    families.join("+")
}

/// `1` reads as nothing to say; anything else is the deprioritisation.
#[must_use]
pub fn weight_text(account: &Account) -> String {
    if (account.pool_weight - 1.0).abs() < f32::EPSILON {
        String::new()
    } else {
        format!("w{:.2}", account.pool_weight)
    }
}

/// Weight steps, clamped: the server requires a finite positive number, and a
/// 0 weight would silently remove the account from every aggregate.
const WEIGHT_MIN: f32 = 0.25;
const WEIGHT_MAX: f32 = 4.0;
const WEIGHT_STEP: f32 = 0.25;

#[must_use]
pub fn stepped_weight(current: f32, up: bool) -> f32 {
    let next = if up { current + WEIGHT_STEP } else { current - WEIGHT_STEP };
    next.clamp(WEIGHT_MIN, WEIGHT_MAX)
}

pub enum AccountAction {
    Open,
    Close,
    Refresh,
    Loaded(Vec<Account>),
    Failed(String),
    RedirectsLoaded(Vec<AccountRedirect>),
    SelectNext,
    SelectPrev,
    /// `Enter`: the detail pane.
    ToggleDetail,
    /// `Tab`: hand the keyboard to the pools pane and back.
    ToggleFocus,
    /// `e`: the owner's veto on pool membership.
    ToggleEligible,
    /// `+` / `-`.
    Weight {
        up: bool,
    },
    /// `R`: open the confirm, then spend it.
    StartReset,
    /// `x`: open the target picker, or clear the rules this account has.
    StartRedirect,
    /// The server refused a write.
    Refused(String),
    /// A claim came back; the credit it named is worth saying.
    ResetDone(Box<cctui_client::LimitResetOutcome>),
    Commit,
    Cancel,
    PickNext,
    PickPrev,
}

#[allow(clippy::too_many_lines)]
pub fn reduce_accounts(app: &mut App, action: AccountAction) -> Vec<Effect> {
    match action {
        AccountAction::Open => {
            let mut effects = super::slice::go_to(app, super::slice::Slice::Accounts);
            if !app.accounts.loaded {
                effects.extend(refresh(app));
            }
            aim_at_handoff(app);
            effects
        }
        AccountAction::Close => {
            if app.accounts.mode.take().is_some() {
                return Vec::new();
            }
            if app.accounts.detail {
                app.accounts.detail = false;
                return Vec::new();
            }
            if matches!(app.accounts.focus, Focus::Pools) {
                app.accounts.focus = Focus::Accounts;
                return Vec::new();
            }
            super::slice::go_to(app, super::slice::Slice::Sessions)
        }
        AccountAction::Refresh => refresh(app),
        AccountAction::Loaded(rows) => {
            app.accounts.rows = rows;
            app.accounts.loading = false;
            app.accounts.loaded = true;
            app.accounts.error = None;
            clamp(app);
            aim_at_handoff(app);
            Vec::new()
        }
        AccountAction::Failed(message) => {
            app.accounts.loading = false;
            app.accounts.loaded = true;
            app.accounts.error = Some(message);
            Vec::new()
        }
        AccountAction::RedirectsLoaded(rules) => {
            app.accounts.redirects = rules;
            Vec::new()
        }
        AccountAction::SelectNext => {
            let len = app.accounts.rows.len();
            if len > 0 {
                app.accounts.selected = (app.accounts.selected + 1).min(len - 1);
            }
            Vec::new()
        }
        AccountAction::SelectPrev => {
            app.accounts.selected = app.accounts.selected.saturating_sub(1);
            Vec::new()
        }
        AccountAction::ToggleDetail => {
            app.accounts.detail = !app.accounts.detail;
            Vec::new()
        }
        AccountAction::ToggleFocus => {
            app.accounts.focus = match app.accounts.focus {
                Focus::Accounts => Focus::Pools,
                Focus::Pools => Focus::Accounts,
            };
            Vec::new()
        }
        AccountAction::ToggleEligible => toggle_eligible(app),
        AccountAction::Weight { up } => weight(app, up),
        AccountAction::StartReset => start_reset(app),
        AccountAction::StartRedirect => start_redirect(app),
        AccountAction::Refused(message) => {
            app.accounts.read_only = true;
            app.toast(Level::Warn, message);
            Vec::new()
        }
        AccountAction::ResetDone(outcome) => {
            let credit = outcome.credit_id.clone().unwrap_or_else(|| "the next credit".to_owned());
            let text = if outcome.reset() {
                format!("reset claimed on {credit}")
            } else if outcome.reused {
                format!("already claimed: {} ({credit})", outcome.outcome)
            } else {
                format!("upstream said {} ({credit})", outcome.outcome)
            };
            app.toast(if outcome.reset() { Level::Info } else { Level::Warn }, text);
            // The windows may have moved, so the usage lane must re-read them
            // rather than serve what it already has.
            let mut effects = refresh(app);
            effects.extend(super::usage::reduce_usage(app, super::usage::UsageAction::Refresh));
            effects
        }
        // One set of form keys serves both panes: at most one form is open.
        AccountAction::Commit => {
            if app.accounts.mode.is_some() {
                return commit(app);
            }
            super::pools::reduce_pools(app, super::pools::PoolAction::Commit)
        }
        AccountAction::Cancel => {
            if app.accounts.mode.take().is_some() {
                return Vec::new();
            }
            super::pools::reduce_pools(app, super::pools::PoolAction::Cancel)
        }
        AccountAction::PickNext => {
            if let Some(Mode::Redirect { targets, selected, .. }) = app.accounts.mode.as_mut() {
                if !targets.is_empty() {
                    *selected = (*selected + 1).min(targets.len() - 1);
                }
                return Vec::new();
            }
            super::pools::reduce_pools(app, super::pools::PoolAction::PickNext)
        }
        AccountAction::PickPrev => {
            if let Some(Mode::Redirect { selected, .. }) = app.accounts.mode.as_mut() {
                *selected = selected.saturating_sub(1);
                return Vec::new();
            }
            super::pools::reduce_pools(app, super::pools::PoolAction::PickPrev)
        }
    }
}

/// One gesture fills both panes: the pools pane is part of this slice, not a
/// view of its own.
fn refresh(app: &mut App) -> Vec<Effect> {
    app.accounts.loading = true;
    app.pools.loading = true;
    vec![Effect::FetchAccounts, Effect::FetchRedirects, Effect::FetchAccountPools]
}

/// `Enter` on a row of the usage panel asks for that account: take the cursor
/// there, and clear the hand-off so it is not re-read on the next open.
fn aim_at_handoff(app: &mut App) {
    let Some(wanted) = app.usage.open_account.map(|id| id.to_string()) else { return };
    // Held, not consumed, until the row it names exists: the hand-off can land
    // before the list it points into.
    let Some(index) = app.accounts.rows.iter().position(|row| row.id == wanted) else { return };
    app.accounts.selected = index;
    app.accounts.detail = true;
    app.usage.open_account = None;
}

fn clamp(app: &mut App) {
    let len = app.accounts.rows.len();
    app.accounts.selected = if len == 0 { 0 } else { app.accounts.selected.min(len - 1) };
}

/// Whether a write may be attempted at all. A 403 already seen is reported
/// rather than re-sent.
fn writable(app: &mut App) -> bool {
    if app.accounts.read_only {
        app.toast(Level::Warn, "this key may not edit accounts");
        return false;
    }
    true
}

fn toggle_eligible(app: &mut App) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(row) = app.accounts.selected_row() else { return Vec::new() };
    let (id, name, next) = (row.id.clone(), row.name.clone(), !row.pool_eligible);
    app.toast(
        Level::Info,
        if next { format!("{name} may be pooled") } else { format!("{name} withheld from pools") },
    );
    vec![Effect::UpdateAccount {
        id,
        request: Box::new(cctui_client::UpdateAccount {
            pool_eligible: Some(next),
            ..Default::default()
        }),
    }]
}

fn weight(app: &mut App, up: bool) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(row) = app.accounts.selected_row() else { return Vec::new() };
    let next = stepped_weight(row.pool_weight, up);
    if (next - row.pool_weight).abs() < f32::EPSILON {
        let edge = if up { "heaviest" } else { "lightest" };
        app.toast(Level::Warn, format!("{} is already the {edge} a pool allows", row.name));
        return Vec::new();
    }
    let id = row.id.clone();
    vec![Effect::UpdateAccount {
        id,
        request: Box::new(cctui_client::UpdateAccount {
            pool_weight: Some(next),
            ..Default::default()
        }),
    }]
}

/// The credential a reset would be claimed on and the credit it would spend.
///
/// The usage lane's rows are authoritative: they name the provider row the
/// reset endpoint takes and the credit upstream offered. Without a reading,
/// the first subscription credential is the target and the server picks the
/// credit.
fn reset_target(app: &App, account: &Account) -> Option<(String, Option<String>, String)> {
    let id = uuid::Uuid::parse_str(&account.id).ok();
    let offered = id.and_then(|id| {
        app.usage.accounts.iter().find(|entry| entry.account == id && entry.limit_reset.is_some())
    });
    if let Some(entry) = offered {
        let reset = entry.limit_reset.as_ref()?;
        let title = reset.title.clone().unwrap_or_else(|| "an unnamed credit".to_owned());
        let credit = if reset.available {
            title
        } else {
            let why =
                reset.ineligible_reason.clone().unwrap_or_else(|| "not claimable yet".to_owned());
            format!("{title} ({why})")
        };
        return Some((entry.account_id.to_string(), reset.credit_id.clone(), credit));
    }
    let provider =
        account.providers.iter().find(|p| matches!(p.family.as_str(), "anthropic" | "openai"))?;
    Some((
        provider.id.clone(),
        None,
        "no credit reported — the server picks the first available".to_owned(),
    ))
}

fn start_reset(app: &mut App) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(row) = app.accounts.selected_row() else { return Vec::new() };
    let Some((provider_id, credit_id, credit)) = reset_target(app, row) else {
        app.toast(Level::Warn, format!("{} has no provider with a limit reset", row.name));
        return Vec::new();
    };
    app.accounts.mode =
        Some(Mode::ConfirmReset { provider_id, credit_id, account: row.name.clone(), credit });
    Vec::new()
}

fn start_redirect(app: &mut App) -> Vec<Effect> {
    if !writable(app) {
        return Vec::new();
    }
    let Some(row) = app.accounts.selected_row() else { return Vec::new() };
    let (id, name) = (row.id.clone(), row.name.clone());
    let existing = app.accounts.chips(&id);
    if !existing.is_empty() {
        app.toast(Level::Info, format!("cleared the redirect on {name}"));
        return existing.into_iter().map(|chip| Effect::DeleteRedirect { id: chip.id }).collect();
    }
    let families: Vec<String> = row.providers.iter().map(|p| p.family.clone()).collect();
    let targets: Vec<Target> = app
        .accounts
        .rows
        .iter()
        .filter(|other| other.id != id)
        .map(|other| Target {
            id: other.id.clone(),
            name: other.name.clone(),
            families: other
                .providers
                .iter()
                .map(|p| p.family.clone())
                .filter(|family| families.contains(family))
                .collect(),
        })
        .filter(|target| !target.families.is_empty())
        .collect();
    if targets.is_empty() {
        app.toast(Level::Warn, format!("no other account shares a provider family with {name}"));
        return Vec::new();
    }
    app.accounts.mode = Some(Mode::Redirect { account: name, targets, selected: 0 });
    Vec::new()
}

fn commit(app: &mut App) -> Vec<Effect> {
    let Some(mode) = app.accounts.mode.take() else { return Vec::new() };
    match mode {
        Mode::ConfirmReset { provider_id, credit_id, .. } => {
            vec![Effect::ClaimLimitReset { provider_id, credit_id }]
        }
        Mode::Redirect { targets, selected, .. } => {
            let Some(target) = targets.get(selected) else { return Vec::new() };
            let Some(from) = app.accounts.selected_row().map(|row| row.id.clone()) else {
                return Vec::new();
            };
            let mut families: Vec<String> = target.families.clone();
            families.dedup();
            app.toast(Level::Info, format!("redirecting to {}", target.name));
            families
                .into_iter()
                .map(|family| Effect::PutRedirect {
                    account_id: from.clone(),
                    to_account: target.id.clone(),
                    family,
                })
                .collect()
        }
    }
}

#[cfg(test)]
pub mod tests {
    use cctui_client::{Account, AccountProvider, AccountRedirect};

    use super::{AccountAction, Focus, Mode, kinds_text, weight_text};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    pub fn provider(id: &str, family: &str) -> AccountProvider {
        AccountProvider {
            id: id.to_owned(),
            provider: family.to_owned(),
            family: family.to_owned(),
            managed: false,
            needs_reauth: false,
            last_auth_error: None,
            est_cost_usd: 1.5,
            total_tokens: 1000,
            last_used_at: None,
            header_pin: false,
        }
    }

    pub fn account(id: &str, name: &str, families: &[&str]) -> Account {
        Account {
            id: id.to_owned(),
            name: name.to_owned(),
            emoji: None,
            user_id: "u1".to_owned(),
            user_name: None,
            providers: families
                .iter()
                .enumerate()
                .map(|(i, family)| provider(&format!("{id}-p{i}"), family))
                .collect(),
            pool_eligible: true,
            pool_weight: 1.0,
        }
    }

    fn act(app: &mut App, action: AccountAction) -> Vec<Effect> {
        reduce(app, Action::Accounts(action))
    }

    /// One `GET /accounts/usage` row for `account`, offering a reset on its own
    /// provider row — the shape the usage lane holds.
    fn usage_row(
        account: &str,
        provider_row: &str,
        reset: Option<cctui_client::LimitResetStatusView>,
    ) -> cctui_client::AccountUsageEntry {
        cctui_client::AccountUsageEntry {
            account_id: uuid::Uuid::parse_str(provider_row).expect("a uuid"),
            provider: "anthropic".to_owned(),
            windows: Vec::new(),
            age_secs: 0,
            account: uuid::Uuid::parse_str(account).expect("a uuid"),
            account_name: "alice@max".to_owned(),
            account_emoji: None,
            header_pin: false,
            provider_status: None,
            limit_reset: reset,
        }
    }

    const ID_A: &str = "aaaaaaaa-1111-4111-8111-111111111111";
    const PROVIDER_A: &str = "99999999-9999-4999-8999-999999999999";

    fn loaded() -> App {
        let mut app = App::new();
        act(
            &mut app,
            AccountAction::Loaded(vec![
                account("a1", "alice@max", &["anthropic"]),
                account("a2", "ops-codex", &["openai"]),
                account("a3", "both", &["anthropic", "openai"]),
            ]),
        );
        app
    }

    #[test]
    fn opening_fetches_the_list_and_the_rules_once() {
        let mut app = App::new();
        let effects = act(&mut app, AccountAction::Open);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchAccounts)));
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchRedirects)));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::FetchAccountPools)),
            "the pools pane is part of this slice"
        );
        assert_eq!(app.slice, crate::app::slice::Slice::Accounts);

        act(&mut app, AccountAction::Loaded(vec![account("a1", "alice@max", &["anthropic"])]));
        assert!(act(&mut app, AccountAction::Open).is_empty(), "reopening reuses the list");
        assert!(!act(&mut app, AccountAction::Refresh).is_empty());
    }

    #[test]
    fn e_flips_the_pool_veto_and_patches_only_that_field() {
        let mut app = loaded();
        let effects = act(&mut app, AccountAction::ToggleEligible);
        let [Effect::UpdateAccount { id, request }] = effects.as_slice() else {
            panic!("one PATCH");
        };
        assert_eq!(id, "a1");
        assert_eq!(request.pool_eligible, Some(false));
        assert_eq!(request.pool_weight, None, "an absent field is left alone server-side");
        assert_eq!(request.name, None);
    }

    #[test]
    fn plus_and_minus_step_the_weight_and_stop_at_the_edges() {
        let mut app = loaded();
        let effects = act(&mut app, AccountAction::Weight { up: false });
        let Some(Effect::UpdateAccount { request, .. }) = effects.first() else {
            panic!("a PATCH");
        };
        assert_eq!(request.pool_weight, Some(0.75));

        app.accounts.rows[0].pool_weight = 0.25;
        assert!(
            act(&mut app, AccountAction::Weight { up: false }).is_empty(),
            "a zero weight would silently drop the account from every aggregate"
        );
        assert!(app.toasts.latest().expect("a toast").text.contains("lightest"));
    }

    #[test]
    fn the_weight_cell_says_nothing_at_one() {
        let mut account = account("a1", "alice", &["anthropic"]);
        assert_eq!(weight_text(&account), "");
        account.pool_weight = 0.5;
        assert_eq!(weight_text(&account), "w0.50");
    }

    #[test]
    fn kinds_collapse_to_the_families_present() {
        assert_eq!(kinds_text(&account("a1", "a", &["anthropic"])), "anthropic");
        assert_eq!(kinds_text(&account("a3", "c", &["anthropic", "openai"])), "anthropic+openai");
        assert_eq!(kinds_text(&account("a0", "none", &[])), "-");
    }

    #[test]
    fn a_reset_confirm_names_the_credit_the_usage_lane_reported() {
        let mut app = loaded();
        app.accounts.rows[0].id = ID_A.to_owned();
        app.usage.accounts = vec![usage_row(
            ID_A,
            PROVIDER_A,
            Some(cctui_client::LimitResetStatusView {
                kind: "claude".to_owned(),
                available: true,
                title: Some("Full reset (Weekly + 5 hr)".to_owned()),
                credit_id: Some("c-7".to_owned()),
                ineligible_reason: None,
                next_available_at: None,
            }),
        )];
        act(&mut app, AccountAction::StartReset);
        let Some(Mode::ConfirmReset { provider_id, credit, .. }) = app.accounts.mode.clone() else {
            panic!("a confirm");
        };
        assert_eq!(
            provider_id, PROVIDER_A,
            "the usage row names the provider the reset endpoint takes"
        );
        assert_eq!(credit, "Full reset (Weekly + 5 hr)");

        let effects = act(&mut app, AccountAction::Commit);
        let [Effect::ClaimLimitReset { provider_id, credit_id }] = effects.as_slice() else {
            panic!("one claim");
        };
        assert_eq!(provider_id, PROVIDER_A);
        assert_eq!(credit_id.as_deref(), Some("c-7"));
    }

    #[test]
    fn a_reset_offered_but_not_claimable_yet_says_why_in_the_confirm() {
        let mut app = loaded();
        app.accounts.rows[0].id = ID_A.to_owned();
        app.usage.accounts = vec![usage_row(
            ID_A,
            PROVIDER_A,
            Some(cctui_client::LimitResetStatusView {
                kind: "claude".to_owned(),
                available: false,
                title: Some("Cedar ember".to_owned()),
                credit_id: Some("c-9".to_owned()),
                ineligible_reason: Some("not_at_wall".to_owned()),
                next_available_at: None,
            }),
        )];
        act(&mut app, AccountAction::StartReset);
        let Some(Mode::ConfirmReset { credit, .. }) = app.accounts.mode.clone() else {
            panic!("a confirm");
        };
        assert_eq!(credit, "Cedar ember (not_at_wall)");
    }

    #[test]
    fn a_reset_confirm_without_a_usage_reading_says_so_rather_than_inventing_one() {
        let mut app = loaded();
        act(&mut app, AccountAction::StartReset);
        let Some(Mode::ConfirmReset { credit, .. }) = app.accounts.mode.clone() else {
            panic!("a confirm");
        };
        assert!(credit.contains("no credit reported"));
        let effects = act(&mut app, AccountAction::Commit);
        let [Effect::ClaimLimitReset { credit_id, .. }] = effects.as_slice() else {
            panic!("one claim");
        };
        assert_eq!(*credit_id, None, "the server picks the first available credit");
    }

    #[test]
    fn a_claim_reports_the_outcome_and_drops_the_stale_usage() {
        let mut app = loaded();
        let effects = act(
            &mut app,
            AccountAction::ResetDone(Box::new(cctui_client::LimitResetOutcome {
                provider: "anthropic".to_owned(),
                outcome: "reset".to_owned(),
                credit_id: Some("c-7".to_owned()),
                next_available_at: None,
                weekly_resets_at: None,
                reused: false,
            })),
        );
        assert!(app.toasts.latest().expect("a toast").text.contains("reset claimed on c-7"));
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchAccounts)), "the list refreshes");
    }

    #[test]
    fn an_unconfirmed_claim_is_not_reported_as_a_success() {
        let mut app = loaded();
        act(
            &mut app,
            AccountAction::ResetDone(Box::new(cctui_client::LimitResetOutcome {
                provider: "openai".to_owned(),
                outcome: "already_redeemed".to_owned(),
                credit_id: None,
                next_available_at: None,
                weekly_resets_at: None,
                reused: true,
            })),
        );
        let toast = app.toasts.latest().expect("a toast");
        assert!(toast.text.contains("already_redeemed"));
        assert_eq!(toast.level, crate::app::toast::Level::Warn);
    }

    #[test]
    fn x_offers_only_targets_sharing_a_provider_family() {
        let mut app = loaded();
        act(&mut app, AccountAction::StartRedirect);
        let Some(Mode::Redirect { targets, .. }) = app.accounts.mode.clone() else {
            panic!("a picker");
        };
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["both"], "ops-codex has no anthropic credential");
        assert_eq!(targets[0].families, vec!["anthropic"]);

        let effects = act(&mut app, AccountAction::Commit);
        let [Effect::PutRedirect { account_id, to_account, family }] = effects.as_slice() else {
            panic!("one rule");
        };
        assert_eq!(
            (account_id.as_str(), to_account.as_str(), family.as_str()),
            ("a1", "a3", "anthropic")
        );
    }

    #[test]
    fn a_redirect_is_written_once_per_shared_family() {
        let mut app = loaded();
        act(&mut app, AccountAction::SelectNext);
        act(&mut app, AccountAction::SelectNext);
        act(&mut app, AccountAction::StartRedirect);
        let Some(Mode::Redirect { targets, .. }) = app.accounts.mode.clone() else {
            panic!("a picker");
        };
        assert_eq!(targets.len(), 2, "both single-family accounts are reachable");
        act(&mut app, AccountAction::PickNext);
        let effects = act(&mut app, AccountAction::Commit);
        let [Effect::PutRedirect { account_id, to_account, family }] = effects.as_slice() else {
            panic!("one rule");
        };
        assert_eq!(
            (account_id.as_str(), to_account.as_str(), family.as_str()),
            ("a3", "a2", "openai")
        );
    }

    #[test]
    fn x_on_a_redirected_account_clears_the_rule_instead_of_asking_again() {
        let mut app = loaded();
        act(
            &mut app,
            AccountAction::RedirectsLoaded(vec![AccountRedirect {
                id: "r1".to_owned(),
                from_account: "a1".to_owned(),
                to_account: Some("a3".to_owned()),
                family: "anthropic".to_owned(),
                to_model: None,
                expires_at: None,
                reason: None,
            }]),
        );
        assert_eq!(app.accounts.chips("a1").len(), 1);
        let effects = act(&mut app, AccountAction::StartRedirect);
        let [Effect::DeleteRedirect { id }] = effects.as_slice() else { panic!("one delete") };
        assert_eq!(id, "r1");
        assert!(app.accounts.mode.is_none());
    }

    #[test]
    fn a_model_only_rule_is_not_a_redirect_chip() {
        let mut app = loaded();
        act(
            &mut app,
            AccountAction::RedirectsLoaded(vec![AccountRedirect {
                id: "r1".to_owned(),
                from_account: "a1".to_owned(),
                to_account: None,
                family: "anthropic".to_owned(),
                to_model: Some("opus".to_owned()),
                expires_at: None,
                reason: None,
            }]),
        );
        assert!(app.accounts.chips("a1").is_empty());
    }

    #[test]
    fn a_forbidden_write_makes_the_slice_read_only_and_says_so_once() {
        let mut app = loaded();
        act(&mut app, AccountAction::Refused("this key may not edit accounts".to_owned()));
        assert!(app.accounts.read_only);
        assert!(act(&mut app, AccountAction::ToggleEligible).is_empty());
        assert!(act(&mut app, AccountAction::Weight { up: true }).is_empty());
        assert!(act(&mut app, AccountAction::StartReset).is_empty());
        assert!(act(&mut app, AccountAction::StartRedirect).is_empty());
        assert!(app.accounts.mode.is_none());
    }

    #[test]
    fn esc_closes_what_is_open_before_leaving_the_slice() {
        let mut app = loaded();
        act(&mut app, AccountAction::Open);
        act(&mut app, AccountAction::ToggleDetail);
        act(&mut app, AccountAction::StartRedirect);
        act(&mut app, AccountAction::Cancel);
        assert!(app.accounts.mode.is_none());
        assert!(app.accounts.detail, "cancelling the picker leaves the detail up");
        act(&mut app, AccountAction::Close);
        assert!(!app.accounts.detail);
        assert_eq!(app.slice, crate::app::slice::Slice::Accounts);
        act(&mut app, AccountAction::Close);
        assert_eq!(app.slice, crate::app::slice::Slice::Sessions);
    }

    #[test]
    fn tab_moves_the_keyboard_to_the_pools_pane_and_the_context_follows() {
        let mut app = loaded();
        act(&mut app, AccountAction::Open);
        assert_eq!(super::key_context(&app), None);
        act(&mut app, AccountAction::ToggleFocus);
        assert_eq!(app.accounts.focus, Focus::Pools);
        assert_eq!(super::key_context(&app), Some(crate::config::keymap::Context::AccountPools));
        act(&mut app, AccountAction::StartRedirect);
        assert_eq!(
            super::key_context(&app),
            Some(crate::config::keymap::Context::AccountsForm),
            "a form outranks the pane"
        );
    }

    #[test]
    fn enter_in_the_usage_panel_opens_that_account_here() {
        let mut app = loaded();
        let wanted = uuid::Uuid::new_v4();
        app.accounts.rows[1].id = wanted.to_string();
        app.usage.open_account = Some(wanted);
        act(&mut app, AccountAction::Open);
        assert_eq!(app.accounts.selected, 1);
        assert!(app.accounts.detail, "the hand-off asks for that account, not just its row");
        assert_eq!(app.usage.open_account, None, "consumed, so it does not fire again");
    }

    #[test]
    fn a_hand_off_that_arrives_before_the_list_still_lands_on_the_row() {
        let mut app = App::new();
        let wanted = uuid::Uuid::new_v4();
        app.usage.open_account = Some(wanted);
        act(&mut app, AccountAction::Open);
        let mut row = account("a9", "late", &["anthropic"]);
        row.id = wanted.to_string();
        act(&mut app, AccountAction::Loaded(vec![account("a1", "first", &["anthropic"]), row]));
        assert_eq!(app.accounts.selected, 1);
        assert_eq!(app.usage.open_account, None);
    }

    #[test]
    fn a_shorter_list_brings_the_cursor_back() {
        let mut app = loaded();
        act(&mut app, AccountAction::SelectNext);
        act(&mut app, AccountAction::SelectNext);
        assert_eq!(app.accounts.selected, 2);
        act(&mut app, AccountAction::Loaded(vec![account("a1", "alice@max", &["anthropic"])]));
        assert_eq!(app.accounts.selected, 0);
    }

    #[test]
    fn a_failed_fetch_is_reported_rather_than_looking_account_less() {
        let mut app = App::new();
        act(&mut app, AccountAction::Open);
        act(&mut app, AccountAction::Failed("forbidden".to_owned()));
        assert!(!app.accounts.loading);
        assert!(app.accounts.loaded);
        assert_eq!(app.accounts.error.as_deref(), Some("forbidden"));
    }

    #[test]
    fn counts_are_poolable_over_total() {
        let mut app = loaded();
        app.accounts.rows[1].pool_eligible = false;
        assert_eq!(app.accounts.counts(), (2, 3));
    }
}
