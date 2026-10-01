//! Rebinding a running session's gateway account.

use cctui_clientcore::account_switch::{
    Binding, Credential, LIMITED_PCT, Option_, recommended, switch_options,
};

use super::action::Effect;
use super::state::{App, View};
use super::toast::Level;

#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub session_id: String,
    pub bindings: Vec<Binding>,
    /// Which binding is being rebound; a session may carry one per family.
    pub binding_ix: usize,
    pub credentials: Vec<Credential>,
    pub options: Vec<Option_>,
    pub focused: usize,
    pub loading: bool,
    pub switching: bool,
    pub error: Option<String>,
}

impl Picker {
    #[allow(clippy::missing_const_for_fn, reason = "Vec::new is not const here")]
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            bindings: Vec::new(),
            binding_ix: 0,
            credentials: Vec::new(),
            options: Vec::new(),
            focused: 0,
            loading: true,
            switching: false,
            error: None,
        }
    }

    #[allow(clippy::missing_const_for_fn, reason = "Vec deref is not const")]
    pub fn binding(&self) -> std::option::Option<&Binding> {
        self.bindings.get(self.binding_ix)
    }

    fn rebuild(&mut self) {
        self.options = self
            .binding()
            .map(|b| switch_options(b, &self.credentials, LIMITED_PCT))
            .unwrap_or_default();
        self.focused = recommended(&self.options).unwrap_or(0);
    }
}

/// `resets_in_secs` is resolved against the wall clock here so the reducer and
/// the fixture-checked ordering stay free of a clock.
pub async fn load(
    server: &cctui_client::Client,
    session_id: &str,
) -> Result<(Vec<Binding>, Vec<Credential>), cctui_client::ClientError> {
    let bindings = server.session_bindings(session_id).await?;
    let usage = server.accounts_usage().await?;
    let now = chrono::Utc::now();
    Ok((
        bindings
            .into_iter()
            .map(|b| Binding {
                family: b.family,
                account_id: b.account_id.to_string(),
                account_name: b.account_name,
            })
            .collect(),
        usage
            .into_iter()
            .map(|entry| Credential {
                account_id: entry.account.to_string(),
                account_name: entry.account_name,
                provider: entry.provider,
                windows: entry
                    .windows
                    .into_iter()
                    .map(|w| cctui_clientcore::account_switch::Window {
                        pct: w.utilization.unwrap_or(0.0),
                        resets_in_secs: w.resets_at.map(|at| (at - now).num_seconds()),
                    })
                    .collect(),
            })
            .collect(),
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountSwitchAction {
    Open,
    Close,
    SelectNext,
    SelectPrev,
    /// Cycle to the next provider family the session is bound in.
    NextBinding,
    Loaded {
        bindings: Vec<Binding>,
        credentials: Vec<Credential>,
    },
    Failed(String),
    Commit,
    Switched {
        account_name: String,
        family: String,
    },
    SwitchFailed(String),
}

pub fn reduce_account_switch(app: &mut App, action: AccountSwitchAction) -> Vec<Effect> {
    match action {
        AccountSwitchAction::Open => {
            let Some(session_id) = subject(app) else {
                app.toast(Level::Warn, "no session selected");
                return Vec::new();
            };
            app.account_switch = Some(Picker::new(session_id.clone()));
            app.router.push(View::AccountSwitch);
            vec![Effect::FetchAccountSwitch { session_id }]
        }
        AccountSwitchAction::Close => {
            close(app);
            Vec::new()
        }
        AccountSwitchAction::SelectNext => {
            if let Some(p) = app.account_switch.as_mut()
                && !p.options.is_empty()
            {
                p.focused = (p.focused + 1) % p.options.len();
            }
            Vec::new()
        }
        AccountSwitchAction::SelectPrev => {
            if let Some(p) = app.account_switch.as_mut()
                && !p.options.is_empty()
            {
                p.focused = p.focused.checked_sub(1).unwrap_or(p.options.len() - 1);
            }
            Vec::new()
        }
        AccountSwitchAction::NextBinding => {
            if let Some(p) = app.account_switch.as_mut()
                && !p.bindings.is_empty()
            {
                p.binding_ix = (p.binding_ix + 1) % p.bindings.len();
                p.rebuild();
            }
            Vec::new()
        }
        AccountSwitchAction::Loaded { bindings, credentials } => {
            if let Some(p) = app.account_switch.as_mut() {
                p.bindings = bindings;
                p.credentials = credentials;
                p.binding_ix = 0;
                p.loading = false;
                p.error = None;
                p.rebuild();
            }
            Vec::new()
        }
        AccountSwitchAction::Failed(message) => {
            if let Some(p) = app.account_switch.as_mut() {
                p.loading = false;
                p.error = Some(message);
            }
            Vec::new()
        }
        AccountSwitchAction::Commit => commit(app),
        AccountSwitchAction::Switched { account_name, family } => {
            close(app);
            app.toast(Level::Info, format!("{family} binding switched to {account_name}"));
            Vec::new()
        }
        AccountSwitchAction::SwitchFailed(message) => {
            if let Some(p) = app.account_switch.as_mut() {
                p.switching = false;
                p.error = Some(message);
            }
            Vec::new()
        }
    }
}

/// The info popup names its own session; otherwise the selected row is it.
fn subject(app: &App) -> std::option::Option<String> {
    app.diagnose.as_ref().map(|d| d.session_id.clone()).or_else(|| app.selected_session_id())
}

fn close(app: &mut App) {
    app.account_switch = None;
    if app.view() == View::AccountSwitch {
        app.router.pop();
    }
}

fn commit(app: &mut App) -> Vec<Effect> {
    let Some(p) = app.account_switch.as_mut() else { return Vec::new() };
    if p.switching {
        return Vec::new();
    }
    let Some(option) = p.options.get(p.focused).cloned() else { return Vec::new() };
    let Some(binding) = p.bindings.get(p.binding_ix).cloned() else { return Vec::new() };
    if option.current {
        p.error = Some("the session is already on that account".to_owned());
        return Vec::new();
    }
    p.switching = true;
    p.error = None;
    vec![Effect::SwitchSessionAccount {
        session_id: p.session_id.clone(),
        account: option.account_id,
        account_name: option.account_name,
        family: binding.family,
    }]
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::account_switch::{Binding, Credential, Window};

    use super::{AccountSwitchAction, Picker};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::app_with_sessions;

    fn dispatch(app: &mut App, action: AccountSwitchAction) -> Vec<Effect> {
        reduce(app, Action::AccountSwitch(action))
    }

    fn cred(name: &str, provider: &str, pct: f64) -> Credential {
        Credential {
            account_id: format!("{name}-id"),
            account_name: name.to_owned(),
            provider: provider.to_owned(),
            windows: vec![Window { pct, resets_in_secs: Some(600) }],
        }
    }

    fn loaded() -> AccountSwitchAction {
        AccountSwitchAction::Loaded {
            bindings: vec![
                Binding {
                    family: "anthropic".to_owned(),
                    account_id: "alice-id".to_owned(),
                    account_name: "alice".to_owned(),
                },
                Binding {
                    family: "openai".to_owned(),
                    account_id: "oai-id".to_owned(),
                    account_name: "oai".to_owned(),
                },
            ],
            credentials: vec![
                cred("alice", "anthropic", 91.0),
                cred("bob", "anthropic", 99.0),
                cred("carol", "anthropic", 12.0),
                cred("oai", "openai", 5.0),
                cred("oai2", "openai", 7.0),
            ],
        }
    }

    fn open(app: &mut App) -> Vec<Effect> {
        let effects = dispatch(app, AccountSwitchAction::Open);
        dispatch(app, loaded());
        effects
    }

    fn picker(app: &App) -> &Picker {
        app.account_switch.as_ref().expect("picker open")
    }

    fn rows(app: &App) -> Vec<&str> {
        picker(app).options.iter().map(|o| o.account_name.as_str()).collect()
    }

    #[test]
    fn opening_asks_for_the_selected_sessions_bindings() {
        let mut app = app_with_sessions();
        let effects = dispatch(&mut app, AccountSwitchAction::Open);
        assert_eq!(app.view(), View::AccountSwitch);
        assert!(picker(&app).loading);
        match effects.as_slice() {
            [Effect::FetchAccountSwitch { session_id }] => {
                assert_eq!(*session_id, picker(&app).session_id);
            }
            _ => panic!("expected one fetch effect"),
        }
    }

    #[test]
    fn the_picker_opens_on_the_recommendation_with_the_current_account_pinned() {
        let mut app = app_with_sessions();
        open(&mut app);
        assert_eq!(rows(&app), vec!["alice", "carol", "bob"]);
        assert!(picker(&app).options[0].current);
        assert!(picker(&app).options[2].limited);
        assert_eq!(picker(&app).focused, 1, "carol is the recommendation");
        assert!(!picker(&app).loading);
    }

    #[test]
    fn cycling_the_binding_reshapes_the_list_for_that_family() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, AccountSwitchAction::NextBinding);
        assert_eq!(rows(&app), vec!["oai", "oai2"]);
        assert_eq!(picker(&app).binding().expect("binding").family, "openai");
        assert_eq!(picker(&app).focused, 1);
        dispatch(&mut app, AccountSwitchAction::NextBinding);
        assert_eq!(rows(&app), vec!["alice", "carol", "bob"], "it wraps");
    }

    #[test]
    fn the_focus_wraps_both_ways() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, AccountSwitchAction::SelectPrev);
        assert_eq!(picker(&app).focused, 0);
        dispatch(&mut app, AccountSwitchAction::SelectPrev);
        assert_eq!(picker(&app).focused, 2);
        dispatch(&mut app, AccountSwitchAction::SelectNext);
        assert_eq!(picker(&app).focused, 0);
    }

    #[test]
    fn committing_posts_the_identity_id_and_the_bindings_family() {
        let mut app = app_with_sessions();
        open(&mut app);
        let effects = dispatch(&mut app, AccountSwitchAction::Commit);
        match effects.as_slice() {
            [Effect::SwitchSessionAccount { account, account_name, family, .. }] => {
                assert_eq!(account, "carol-id");
                assert_eq!(account_name, "carol");
                assert_eq!(family, "anthropic");
            }
            _ => panic!("expected one switch effect"),
        }
        assert!(picker(&app).switching, "a second Enter is ignored while it is in flight");
        assert!(dispatch(&mut app, AccountSwitchAction::Commit).is_empty());
    }

    #[test]
    fn committing_the_other_family_posts_that_family() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, AccountSwitchAction::NextBinding);
        let effects = dispatch(&mut app, AccountSwitchAction::Commit);
        match effects.as_slice() {
            [Effect::SwitchSessionAccount { account, family, .. }] => {
                assert_eq!(account, "oai2-id");
                assert_eq!(family, "openai");
            }
            _ => panic!("expected one switch effect"),
        }
    }

    #[test]
    fn committing_the_account_already_in_use_posts_nothing() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, AccountSwitchAction::SelectPrev);
        assert!(dispatch(&mut app, AccountSwitchAction::Commit).is_empty());
        assert!(picker(&app).error.is_some());
        assert!(!picker(&app).switching);
    }

    #[test]
    fn a_successful_switch_closes_the_picker_and_toasts() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, AccountSwitchAction::Commit);
        dispatch(
            &mut app,
            AccountSwitchAction::Switched {
                account_name: "carol".to_owned(),
                family: "anthropic".to_owned(),
            },
        );
        assert!(app.account_switch.is_none());
        assert_ne!(app.view(), View::AccountSwitch);
        assert!(app.toasts.latest().expect("a toast").text.contains("carol"));
    }

    #[test]
    fn a_rejected_switch_keeps_the_picker_open_with_the_reason() {
        let mut app = app_with_sessions();
        open(&mut app);
        dispatch(&mut app, AccountSwitchAction::Commit);
        dispatch(&mut app, AccountSwitchAction::SwitchFailed("409 already there".to_owned()));
        assert_eq!(app.view(), View::AccountSwitch);
        assert_eq!(picker(&app).error.as_deref(), Some("409 already there"));
        assert!(!picker(&app).switching, "the operator can pick again");
    }

    #[test]
    fn a_failed_load_stops_the_spinner_and_offers_nothing() {
        let mut app = app_with_sessions();
        dispatch(&mut app, AccountSwitchAction::Open);
        dispatch(&mut app, AccountSwitchAction::Failed("502".to_owned()));
        assert!(!picker(&app).loading);
        assert!(picker(&app).options.is_empty());
        assert!(dispatch(&mut app, AccountSwitchAction::Commit).is_empty());
    }

    #[test]
    fn closing_posts_nothing_and_leaves_the_view() {
        let mut app = app_with_sessions();
        open(&mut app);
        assert!(dispatch(&mut app, AccountSwitchAction::Close).is_empty());
        assert!(app.account_switch.is_none());
        assert_ne!(app.view(), View::AccountSwitch);
    }
}
