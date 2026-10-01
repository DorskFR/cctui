//! Which account or pool the spawn bills against.
//!
//! The picker offers only what the harness can actually run: the server
//! refuses an account with no credential in the harness's family, so offering
//! one is how a spawn fails after the dialog closed.

use cctui_client::accounts::{Account, AccountPool};
use cctui_client::usage::AccountUsageEntry;
use cctui_clientcore::spawn::{NO_ACCOUNT, SpawnFields, pool_name, pool_value};
use cctui_clientcore::spawn_accounts as rules;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};

use super::SpawnSection;
use crate::app::action::Effect;
use crate::theme;

/// One offered row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The value the request carries: `""` auto, [`NO_ACCOUNT`], a
    /// pool value, or an account name.
    pub value: String,
    pub label: String,
    /// `62%` when the account reported usage.
    pub hint: Option<String>,
}

#[derive(Debug, Default)]
pub struct AccountSection {
    pub accounts: Vec<Account>,
    pub pools: Vec<AccountPool>,
    pub usage: Vec<AccountUsageEntry>,
    /// The harness the form holds, mirrored on every key so the filter and the
    /// trait's borrowed answers agree without reaching back into the form.
    pub harness: String,
    /// The harness the pick forces, when it forces one.
    pub override_harness: Option<String>,
    /// The picker value, in the web UI's value space.
    pub value: String,
    pub open: bool,
    pub cursor: usize,
    /// What the server said when it refused the last spawn.
    pub error: Option<String>,
}

impl AccountSection {
    /// The accounts and pools this harness can run, then auto and the ambient
    /// machine login.
    #[must_use]
    pub fn choices(&self) -> Vec<Choice> {
        let harness = self.harness();
        let mut out = Vec::with_capacity(self.accounts.len() + self.pools.len() + 2);

        let members: Vec<rules::PoolMembers<'_>> = self
            .pools
            .iter()
            .map(|p| rules::PoolMembers {
                members: p.members.iter().map(|m| m.account_id.as_str()).collect(),
            })
            .collect();
        let lookup = |id: &str| -> Option<Vec<&str>> {
            self.accounts.iter().find(|a| a.id == id).map(Account::provider_names)
        };
        for i in rules::compatible_pools(&members, &lookup, harness) {
            let pool = &self.pools[i];
            out.push(Choice {
                value: pool_value(&pool.name),
                label: format!("pool: {}", pool.name),
                hint: None,
            });
        }

        for account in &self.accounts {
            if !rules::account_backs_adapter(Some(&account.provider_names()), harness) {
                continue;
            }
            out.push(Choice {
                value: account.name.clone(),
                label: match account.emoji.as_deref() {
                    Some(emoji) if !emoji.is_empty() => format!("{emoji} {}", account.name),
                    _ => account.name.clone(),
                },
                hint: self.used_pct(&account.id).map(|pct| format!("{pct}%")),
            });
        }

        out.push(Choice {
            value: String::new(),
            label: "auto (most allocation left)".to_owned(),
            hint: None,
        });
        out.push(Choice {
            value: NO_ACCOUNT.to_owned(),
            label: "machine login (no account)".to_owned(),
            hint: None,
        });
        out
    }

    fn harness(&self) -> &str {
        if self.harness.is_empty() { "claude-code" } else { &self.harness }
    }

    /// Mirrors the form's harness and recomputes what the pick forces.
    pub fn sync(&mut self, fields: &SpawnFields) {
        if self.harness != fields.adapter_id {
            self.harness.clone_from(&fields.adapter_id);
        }
        self.override_harness = self.forced_harness();
    }

    fn forced_harness(&self) -> Option<String> {
        let account = self.picked()?;
        let providers = account.provider_names();
        if rules::account_backs_adapter(Some(&providers), self.harness()) {
            return None;
        }
        Some(rules::effective_adapter_for(Some(&providers), self.harness()))
    }

    fn used_pct(&self, account_id: &str) -> Option<u32> {
        let windows: Vec<rules::UsageWindow> = self
            .usage
            .iter()
            .filter(|u| u.account.to_string() == account_id)
            .flat_map(|u| &u.windows)
            .map(|w| rules::UsageWindow {
                key: w.key.clone(),
                utilization: w.utilization.unwrap_or_default(),
            })
            .collect();
        rules::headline_pct(&windows)
    }

    fn picked(&self) -> Option<&Account> {
        if self.value.is_empty() || self.value == NO_ACCOUNT {
            return None;
        }
        pool_name(&self.value)
            .map_or_else(|| self.accounts.iter().find(|a| a.name == self.value), |_| None)
    }

    fn provider_behind_pick(&self) -> Option<&str> {
        let account = self.picked()?;
        let harness = self.harness();
        account
            .providers
            .iter()
            .map(|p| p.provider.as_str())
            .find(|provider| rules::adapter_for_provider(provider) == harness)
    }

    fn own_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let names: Vec<&str> = self.accounts.iter().map(|a| a.name.as_str()).collect();
        if rules::stale_account_pick(&self.value, &names) {
            out.push(format!("account \"{}\" is gone — pick another or use auto", self.value));
        } else if let Some(account) = self.picked() {
            let providers = account.provider_names();
            if !rules::account_backs_adapter(Some(&providers), self.harness()) {
                out.push(format!(
                    "{} has no credential for {} — it will run on {}",
                    account.name,
                    self.harness(),
                    rules::effective_adapter_for(Some(&providers), self.harness())
                ));
            }
        }
        if let Some(error) = &self.error {
            out.push(error.clone());
        }
        out
    }

    /// The label of the current pick, for the collapsed row.
    #[must_use]
    pub fn summary(&self) -> String {
        let choices = self.choices();
        choices.iter().find(|c| c.value == self.value).map_or_else(
            || {
                if self.value.is_empty() {
                    "auto".to_owned()
                } else {
                    format!("{} (gone)", self.value)
                }
            },
            |c| {
                c.hint
                    .as_ref()
                    .map_or_else(|| c.label.clone(), |hint| format!("{}  {hint} used", c.label))
            },
        )
    }

    fn own_lines(&self, focused: Option<usize>) -> Vec<Line<'static>> {
        let marker = if focused.is_some() { "›" } else { " " };
        let mut out = vec![Line::from(vec![
            Span::styled(format!(" {marker} "), theme::section_title()),
            Span::styled(
                format!("[{}]", self.summary()),
                if focused.is_some() { theme::bold() } else { theme::dim() },
            ),
        ])];
        if self.open {
            for (i, choice) in self.choices().iter().enumerate() {
                let picked = if choice.value == self.value { "•" } else { " " };
                let style = if i == self.cursor { theme::selected() } else { theme::dim() };
                let hint = choice.hint.as_deref().unwrap_or_default();
                out.push(Line::from(Span::styled(
                    format!("      {picked} {:<28} {hint}", choice.label),
                    style,
                )));
            }
        }
        for problem in self.own_problems() {
            out.push(Line::from(Span::styled(format!("      ! {problem}"), theme::error())));
        }
        out
    }

    fn own_handle(&mut self, key: KeyEvent) -> Vec<Effect> {
        let choices = self.choices();
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter if !self.open => {
                self.open = true;
                self.cursor = choices.iter().position(|c| c.value == self.value).unwrap_or(0);
            }
            KeyCode::Enter => {
                if let Some(choice) = choices.get(self.cursor) {
                    self.value.clone_from(&choice.value);
                    self.error = None;
                }
                self.open = false;
            }
            KeyCode::Esc => self.open = false,
            KeyCode::Char('j') | KeyCode::Down if self.open => {
                self.cursor = (self.cursor + 1).min(choices.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up if self.open => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            _ => {}
        }
        Vec::new()
    }

    /// A refusal the dialog should show on this row rather than as a toast
    /// that scrolls away. An ambiguous account gets the way out appended.
    #[allow(dead_code, reason = "the dialog's submit path calls this; it lands with the skeleton")]
    pub fn set_error(&mut self, message: &str) {
        self.error = Some(spawn_error_hint(message));
    }
}

/// `several accounts` is the server saying it cannot choose; auto is the
/// answer, so the message says so.
#[must_use]
pub fn spawn_error_hint(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("several accounts") || lower.contains("ambiguous") {
        return format!("{message} — choose auto, or name one account");
    }
    message.to_owned()
}

impl SpawnSection for AccountSection {
    fn title(&self) -> &'static str {
        "Account"
    }

    fn rows(&self, _fields: &SpawnFields) -> usize {
        1
    }

    fn lines(
        &self,
        focused: Option<usize>,
        width: u16,
        _fields: &SpawnFields,
    ) -> Vec<Line<'static>> {
        super::clamp_rows(self.own_lines(focused), width)
    }

    fn handle(&mut self, _row: usize, key: KeyEvent, fields: &mut SpawnFields) -> Vec<Effect> {
        self.sync(fields);
        let effects = self.own_handle(key);
        self.override_harness = self.forced_harness();
        effects
    }

    /// Nothing: `account`, `provider`, `pool`, `no_account` and `auto_account`
    /// are one coupled rule, and the shared request builder owns it.
    fn apply(&self, _request: &mut cctui_proto::api::SpawnRequest) {}

    fn problems(&self) -> Vec<String> {
        self.own_problems()
    }

    fn harness_override(&self) -> Option<&str> {
        self.override_harness.as_deref()
    }

    fn account_pick(&self) -> Option<&str> {
        Some(&self.value)
    }

    fn selected_provider(&self) -> Option<&str> {
        self.provider_behind_pick()
    }

    fn receive(&mut self, data: &super::SpawnData) {
        self.accounts.clone_from(&data.accounts);
        self.pools.clone_from(&data.pools);
        self.usage.clone_from(&data.usage);
        self.override_harness = self.forced_harness();
    }
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::spawn::{NO_ACCOUNT, pool_name, pool_value};

    use super::{AccountSection, SpawnSection, spawn_error_hint};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Ids are uuids because the usage rows key off them: one stable id per
    /// name, built from its bytes rather than randomly.
    fn id_of(name: &str) -> String {
        use std::fmt::Write as _;
        let mut hex = String::new();
        for b in name.bytes().cycle().take(16) {
            let _ = write!(hex, "{b:02x}");
        }
        format!(
            "{}-{}-4{}-8{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[13..16],
            &hex[17..20],
            &hex[20..32]
        )
    }

    fn account(name: &str, providers: &[&str]) -> cctui_client::accounts::Account {
        let providers: Vec<_> = providers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": format!("cred-{p}"),
                    "provider": p,
                    "family": p,
                    "managed": false,
                    "needs_reauth": false,
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": id_of(name),
            "name": name,
            "user_id": id_of("user"),
            "providers": providers,
            "pool_eligible": true,
            "pool_weight": 1.0,
        }))
        .expect("an account")
    }

    fn pool(name: &str, members: &[&str]) -> cctui_client::accounts::AccountPool {
        serde_json::from_value(serde_json::json!({
            "id": format!("pid-{name}"),
            "user_id": id_of("user"),
            "name": name,
            "strategy": "most_left",
            "failover": true,
            "members": members
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    serde_json::json!({
                        "account_id": id_of(m),
                        "name": m,
                        "position": i,
                        "owned": true,
                        "pool_eligible": true,
                    })
                })
                .collect::<Vec<_>>(),
        }))
        .expect("a pool")
    }

    fn usage(
        account_id: &str,
        key: &str,
        utilization: f64,
    ) -> cctui_client::usage::AccountUsageEntry {
        serde_json::from_value(serde_json::json!({
            "account_id": account_id,
            "account": account_id,
            "windows": [{"key": key, "utilization": utilization}],
        }))
        .expect("usage")
    }

    fn fields(harness: &str) -> cctui_clientcore::spawn::SpawnFields {
        cctui_clientcore::spawn::SpawnFields {
            adapter_id: harness.to_owned(),
            ..cctui_clientcore::spawn::SpawnFields::default()
        }
    }

    fn press(s: &mut AccountSection, code: KeyCode) {
        let mut f = fields(&s.harness.clone());
        s.handle(0, key(code), &mut f);
    }

    fn section() -> AccountSection {
        AccountSection {
            accounts: vec![
                account("personal-max", &["anthropic"]),
                account("work-team", &["openai"]),
            ],
            pools: vec![pool("personal", &["personal-max"])],
            usage: vec![usage(&id_of("personal-max"), "session", 62.0)],
            harness: "claude-code".to_owned(),
            ..AccountSection::default()
        }
    }

    fn values(s: &AccountSection) -> Vec<String> {
        s.choices().into_iter().map(|c| c.value).collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn only_accounts_that_can_run_the_harness_are_offered() {
        let mut s = section();
        assert_eq!(
            values(&s),
            [
                pool_value("personal"),
                "personal-max".to_owned(),
                String::new(),
                NO_ACCOUNT.to_owned(),
            ],
            "the openai account cannot back claude-code"
        );

        s.harness = "codex".to_owned();
        assert_eq!(
            values(&s),
            ["work-team".to_owned(), String::new(), NO_ACCOUNT.to_owned()],
            "the openai account arrives, and the all-anthropic pool goes"
        );
    }

    #[test]
    fn auto_and_the_machine_login_are_always_offered() {
        let s = AccountSection::default();
        assert_eq!(values(&s), [String::new(), NO_ACCOUNT.to_owned()]);
        assert_eq!(s.summary(), "auto (most allocation left)");
    }

    #[test]
    fn the_picker_shows_the_accounts_usage() {
        let s = section();
        let personal =
            s.choices().into_iter().find(|c| c.value == "personal-max").expect("offered");
        assert_eq!(personal.hint.as_deref(), Some("62%"));
    }

    #[test]
    fn opening_starts_on_the_current_pick_and_enter_stores_the_new_one() {
        let mut s = section();
        press(&mut s, KeyCode::Char(' '));
        assert!(s.open);
        assert_eq!(s.cursor, 2, "the list opens on Auto, which is what is picked");

        press(&mut s, KeyCode::Char('k'));
        press(&mut s, KeyCode::Enter);
        assert!(!s.open);
        assert_eq!(s.account_pick().expect("a pick"), "personal-max");
        assert_eq!(s.summary(), "personal-max  62% used");
    }

    #[test]
    fn escape_closes_the_list_without_picking() {
        let mut s = section();
        press(&mut s, KeyCode::Char(' '));
        press(&mut s, KeyCode::Char('j'));
        press(&mut s, KeyCode::Esc);
        assert!(!s.open);
        assert_eq!(s.account_pick().expect("a pick"), "", "the pick is unchanged");
    }

    #[test]
    fn a_pool_pick_is_distinguishable_from_an_account_of_the_same_name() {
        let mut s = section();
        s.accounts.push(account("personal", &["anthropic"]));
        s.value = pool_value("personal");
        assert_eq!(pool_name(s.account_pick().expect("a pick")), Some("personal"));
        assert!(s.selected_provider().is_none(), "a pool resolves server-side");
        assert!(s.own_problems().is_empty(), "a pool pick is never stale");
    }

    #[test]
    fn the_provider_behind_the_pick_drives_the_model_catalog() {
        let mut s = section();
        s.value = "personal-max".to_owned();
        assert_eq!(s.selected_provider(), Some("anthropic"));
        s.harness = "codex".to_owned();
        s.value = "work-team".to_owned();
        assert_eq!(s.selected_provider(), Some("openai"));
    }

    #[test]
    fn an_account_from_another_family_asks_the_form_to_switch_harness() {
        let mut s = section();
        s.value = "work-team".to_owned();
        s.override_harness = s.forced_harness();
        assert_eq!(s.harness_override(), Some("codex"));
        assert!(s.own_problems()[0].contains("has no credential for claude-code"));

        s.harness = "codex".to_owned();
        s.override_harness = s.forced_harness();
        assert!(s.harness_override().is_none(), "nothing to switch once it fits");
        assert!(s.own_problems().is_empty());
    }

    #[test]
    fn a_pick_that_went_away_is_flagged() {
        let mut s = section();
        s.value = "retired".to_owned();
        assert!(s.own_problems()[0].contains("is gone"), "{:?}", s.own_problems());
        assert_eq!(s.summary(), "retired (gone)");
    }

    #[test]
    fn an_ambiguous_account_refusal_names_the_way_out() {
        assert_eq!(
            spawn_error_hint("several accounts match"),
            "several accounts match — choose auto, or name one account"
        );
        assert_eq!(spawn_error_hint("machine offline"), "machine offline");

        let mut s = section();
        s.set_error("several accounts match");
        assert!(s.own_problems()[0].contains("choose auto"));
        press(&mut s, KeyCode::Char(' '));
        press(&mut s, KeyCode::Enter);
        assert!(s.error.is_none(), "a fresh pick clears the refusal");
    }
}
