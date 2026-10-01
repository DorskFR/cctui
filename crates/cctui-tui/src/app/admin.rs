//! The Access slice: users, and per-user tokens, machines and API keys.
//!
//! Admin-only, checked against the `/me` scopes before the slice can be
//! entered at all, which is the same check the server makes on every route
//! here.
//!
//! A minted or rotated secret exists in exactly one place: the dialog showing
//! it. It never reaches a toast, a log line or the draft store, and the buffer
//! is overwritten when the dialog closes, so a later render cannot recover it.

use cctui_client::{ApiKey, Client, MintKey, UpdateUser, User, UserMachine, UserToken};
use cctui_clientcore::admin::{ALL_SCOPES, visible_order};

use super::action::{Action, Effect};
use super::state::App;
use super::toast::Level;

pub const ADMIN_SCOPE: &str = "admin";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Users,
    Tokens,
    Machines,
    Keys,
}

impl Tab {
    pub const ORDER: [Self; 4] = [Self::Users, Self::Tokens, Self::Machines, Self::Keys];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Users => "Users",
            Self::Tokens => "Tokens",
            Self::Machines => "Machines",
            Self::Keys => "Keys",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ORDER.iter().position(|t| *t == self).unwrap_or(0)
    }

    #[must_use]
    pub fn step(self, delta: isize) -> Self {
        let len = Self::ORDER.len();
        let next = (self.index().cast_signed() + delta).rem_euclid(len.cast_signed());
        Self::ORDER[next as usize]
    }
}

/// Which text the one-line form is collecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormKind {
    NewUser,
    RenameUser { id: String },
    MintToken { user_id: String },
    RelabelToken { user_id: String, token_id: String },
}

impl FormKind {
    #[must_use]
    pub const fn prompt(&self) -> &'static str {
        match self {
            Self::NewUser => "new user name",
            Self::RenameUser { .. } => "rename to",
            Self::MintToken { .. } => "token label (optional)",
            Self::RelabelToken { .. } => "token label (blank clears)",
        }
    }

    /// Whether an empty value is a legal answer.
    #[must_use]
    pub const fn optional(&self) -> bool {
        matches!(self, Self::MintToken { .. } | Self::RelabelToken { .. })
    }
}

/// A one-keystroke confirmation: reversible, or at least non-destructive of data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    RevokeUser { id: String, name: String },
    RevokeToken { user_id: String, token_id: String },
    RevokeMachine { id: String, name: String },
    RotateMachine { id: String, name: String },
    RevokeKey { user_id: String, key_id: String },
}

impl Pending {
    #[must_use]
    pub fn question(&self) -> String {
        match self {
            Self::RevokeUser { name, .. } => {
                format!("revoke {name}? every token and machine stops working")
            }
            Self::RevokeToken { .. } => "revoke this token?".to_owned(),
            Self::RevokeMachine { name, .. } => format!("revoke {name}?"),
            Self::RotateMachine { name, .. } => {
                format!("rotate {name}'s key? the old key stops working at once")
            }
            Self::RevokeKey { .. } => "revoke this key?".to_owned(),
        }
    }
}

/// A hard delete: the name has to be typed out, because nothing comes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purge {
    User { id: String },
    Token { user_id: String, token_id: String },
    Machine { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Form {
        kind: FormKind,
        text: String,
    },
    Confirm(Pending),
    /// `typed` has to match `name` before the purge is sent.
    TypedConfirm {
        what: Purge,
        name: String,
        typed: String,
    },
    /// The scope checkbox list, minting a new key or re-granting an existing
    /// one in place. Editing leaves the secret alone, so the key keeps working.
    KeyScopes {
        user_id: String,
        key_id: Option<String>,
        label: String,
        /// Which of the user's ceiling scopes the cursor is on.
        cursor: usize,
        granted: Vec<bool>,
    },
    /// The one copy of a secret that exists anywhere.
    Secret {
        what: &'static str,
        secret: String,
    },
}

#[derive(Debug, Default)]
pub struct Access {
    pub open: bool,
    pub tab: Tab,
    pub users: Vec<User>,
    pub selected_user: usize,
    pub tokens: Vec<UserToken>,
    pub machines: Vec<UserMachine>,
    pub keys: Vec<ApiKey>,
    /// The selected user's scope ceiling, which a minted key cannot exceed.
    pub ceiling: Vec<String>,
    pub row: usize,
    pub show_revoked: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub mode: Option<Mode>,
}

impl Access {
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode.clone().unwrap_or(Mode::Browse)
    }

    #[must_use]
    pub fn user(&self) -> Option<&User> {
        self.users.get(self.selected_user)
    }

    /// The rows the current tab draws, as indices into its own list.
    #[must_use]
    pub fn rows(&self) -> Vec<usize> {
        let revoked: Vec<bool> = match self.tab {
            Tab::Users => self.users.iter().map(User::revoked).collect(),
            Tab::Tokens => self.tokens.iter().map(|t| t.revoked_at.is_some()).collect(),
            Tab::Machines => self.machines.iter().map(|m| m.revoked_at.is_some()).collect(),
            Tab::Keys => self.keys.iter().map(|k| k.revoked_at.is_some()).collect(),
        };
        visible_order(&revoked, self.show_revoked)
    }

    /// Which entry of the underlying list the cursor is on.
    #[must_use]
    pub fn cursor(&self) -> Option<usize> {
        self.rows().get(self.row).copied()
    }

    fn clamp(&mut self) {
        let len = self.rows().len();
        self.row = if len == 0 { 0 } else { self.row.min(len - 1) };
        let users = self.users.len();
        self.selected_user = if users == 0 { 0 } else { self.selected_user.min(users - 1) };
        if self.tab == Tab::Users {
            self.selected_user = self.cursor().unwrap_or(0);
        }
    }

    /// Overwrite the secret's buffer in place, then drop it. Equal-length
    /// replacement does not reallocate, so the bytes that held it are cleared.
    fn forget_secret(&mut self) {
        if let Some(Mode::Secret { secret, .. }) = self.mode.as_mut() {
            let len = secret.len();
            secret.replace_range(.., &"\0".repeat(len));
            secret.clear();
        }
        self.mode = None;
    }
}

/// What a non-admin is told if they reach for the slice anyway.
pub const DENIED: &str = "Access needs the admin scope";

#[must_use]
pub fn is_admin(app: &App) -> bool {
    match &app.auth {
        super::identity::AuthState::Identified(identity) => {
            identity.scopes.iter().any(|s| s == ADMIN_SCOPE)
        }
        _ => false,
    }
}

/// The work the effects runner does for this slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessEffect {
    Users,
    /// Everything the detail tabs show for one user, in one round trip set.
    Detail {
        user_id: String,
    },
    CreateUser {
        name: String,
    },
    UpdateUser {
        id: String,
        name: Option<String>,
        disabled: Option<bool>,
    },
    RevokeUser {
        id: String,
    },
    PurgeUser {
        id: String,
    },
    MintToken {
        user_id: String,
        label: Option<String>,
    },
    RelabelToken {
        user_id: String,
        token_id: String,
        label: Option<String>,
    },
    RevokeToken {
        user_id: String,
        token_id: String,
    },
    PurgeToken {
        user_id: String,
        token_id: String,
    },
    RevokeMachine {
        user_id: String,
        id: String,
    },
    PurgeMachine {
        user_id: String,
        id: String,
    },
    RotateMachine {
        user_id: String,
        id: String,
    },
    MintKey {
        user_id: String,
        label: Option<String>,
        scopes: Vec<String>,
    },
    SetKeyScopes {
        user_id: String,
        key_id: String,
        scopes: Vec<String>,
    },
    RevokeKey {
        user_id: String,
        key_id: String,
    },
}

pub enum AccessAction {
    Open,
    Close,
    Refresh,
    UsersLoaded(Vec<User>),
    DetailLoaded {
        tokens: Vec<UserToken>,
        machines: Vec<UserMachine>,
        keys: Vec<ApiKey>,
        ceiling: Vec<String>,
    },
    Failed(String),
    SelectNext,
    SelectPrev,
    NextTab,
    PrevTab,
    ToggleRevoked,
    StartNew,
    StartRename,
    ToggleDisable,
    StartRevoke,
    StartRotate,
    StartPurge,
    /// Space in the scope dialog: grant or drop the scope under the cursor.
    ToggleScope,
    /// Re-grant the scopes of the key the cursor is on, secret untouched.
    StartKeyScopes,
    Key(crossterm::event::KeyEvent),
    Commit,
    Cancel,
    /// A mint or rotate returned the one copy of a secret.
    Secret {
        what: &'static str,
        secret: String,
    },
    CopySecret,
    /// A mutation succeeded: say so without naming anything it returned.
    Done(String),
    /// The dispatchers panel is its own admin surface; this hands off to it.
    OpenDispatchers,
}

#[allow(clippy::too_many_lines)]
pub fn reduce_access(app: &mut App, action: AccessAction) -> Vec<Effect> {
    match action {
        AccessAction::Open => {
            if !is_admin(app) {
                app.toast(Level::Warn, DENIED);
                return Vec::new();
            }
            app.access.open = true;
            super::slice::go_to(app, super::slice::Slice::Access)
        }
        AccessAction::Close => {
            app.access.forget_secret();
            app.access.open = false;
            super::slice::go_to(app, super::slice::Slice::Sessions)
        }
        AccessAction::Refresh => refresh(app),
        AccessAction::UsersLoaded(users) => {
            app.access.users = users;
            app.access.loading = false;
            app.access.error = None;
            app.access.clamp();
            detail(app)
        }
        AccessAction::DetailLoaded { tokens, machines, keys, ceiling } => {
            app.access.tokens = tokens;
            app.access.machines = machines;
            app.access.keys = keys;
            app.access.ceiling = ceiling;
            app.access.loading = false;
            app.access.clamp();
            Vec::new()
        }
        AccessAction::Failed(message) => {
            app.access.loading = false;
            app.access.error = Some(message);
            Vec::new()
        }
        // While the scope dialog is up it owns the cursor: the checkboxes are
        // what j/k walks, not the table underneath.
        AccessAction::SelectNext => {
            if let Some(Mode::KeyScopes { cursor, .. }) = app.access.mode.as_mut() {
                *cursor = (*cursor + 1).min(ALL_SCOPES.len() - 1);
                return Vec::new();
            }
            let len = app.access.rows().len();
            if len > 0 {
                app.access.row = (app.access.row + 1).min(len - 1);
            }
            on_cursor_moved(app)
        }
        AccessAction::SelectPrev => {
            if let Some(Mode::KeyScopes { cursor, .. }) = app.access.mode.as_mut() {
                *cursor = cursor.saturating_sub(1);
                return Vec::new();
            }
            app.access.row = app.access.row.saturating_sub(1);
            on_cursor_moved(app)
        }
        AccessAction::NextTab => switch_tab(app, 1),
        AccessAction::PrevTab => switch_tab(app, -1),
        AccessAction::ToggleRevoked => {
            app.access.show_revoked = !app.access.show_revoked;
            app.access.row = 0;
            app.access.clamp();
            Vec::new()
        }
        AccessAction::StartNew => start_new(app),
        AccessAction::StartRename => start_rename(app),
        AccessAction::ToggleDisable => toggle_disable(app),
        AccessAction::StartRevoke => start_revoke(app),
        AccessAction::StartRotate => start_rotate(app),
        AccessAction::StartPurge => start_purge(app),
        AccessAction::StartKeyScopes => start_key_scopes(app),
        AccessAction::ToggleScope => {
            if let Some(Mode::KeyScopes { cursor, granted, .. }) = app.access.mode.as_mut()
                && let Some(flag) = granted.get_mut(*cursor)
            {
                *flag = !*flag;
            }
            Vec::new()
        }
        AccessAction::Key(key) => {
            key_in_mode(app, key);
            Vec::new()
        }
        AccessAction::Commit => commit(app),
        AccessAction::Cancel => {
            if app.access.mode.is_some() {
                app.access.forget_secret();
                return Vec::new();
            }
            reduce_access(app, AccessAction::Close)
        }
        AccessAction::Secret { what, secret } => {
            app.access.mode = Some(Mode::Secret { what, secret });
            refresh(app)
        }
        AccessAction::CopySecret => {
            let Some(Mode::Secret { what, secret }) = app.access.mode.clone() else {
                return Vec::new();
            };
            vec![Effect::Copy { text: secret, label: what }]
        }
        AccessAction::Done(message) => {
            app.toast(Level::Info, message);
            refresh(app)
        }
        AccessAction::OpenDispatchers => {
            super::dispatchers::reduce_dispatchers(app, super::dispatchers::DispatcherAction::Open)
        }
    }
}

/// Entering the slice: the user list first, the selected user's detail after.
pub fn on_enter(app: &mut App) -> Vec<Effect> {
    refresh(app)
}

fn refresh(app: &mut App) -> Vec<Effect> {
    app.access.loading = true;
    vec![Effect::Access(Box::new(AccessEffect::Users))]
}

fn detail(app: &App) -> Vec<Effect> {
    let Some(user) = app.access.user() else { return Vec::new() };
    vec![Effect::Access(Box::new(AccessEffect::Detail { user_id: user.id.clone() }))]
}

/// Moving the cursor on the Users tab changes whose detail the other tabs show.
fn on_cursor_moved(app: &mut App) -> Vec<Effect> {
    if app.access.tab != Tab::Users {
        return Vec::new();
    }
    let before = app.access.selected_user;
    app.access.selected_user = app.access.cursor().unwrap_or(0);
    if app.access.selected_user == before {
        return Vec::new();
    }
    detail(app)
}

fn switch_tab(app: &mut App, delta: isize) -> Vec<Effect> {
    app.access.tab = app.access.tab.step(delta);
    app.access.row = 0;
    app.access.clamp();
    if app.access.tab == Tab::Users { Vec::new() } else { detail(app) }
}

fn start_new(app: &mut App) -> Vec<Effect> {
    let kind = match app.access.tab {
        Tab::Users => FormKind::NewUser,
        Tab::Tokens => {
            let Some(user) = app.access.user() else { return Vec::new() };
            FormKind::MintToken { user_id: user.id.clone() }
        }
        Tab::Keys => {
            let Some(user) = app.access.user() else { return Vec::new() };
            let granted = vec![false; ALL_SCOPES.len()];
            app.access.mode = Some(Mode::KeyScopes {
                user_id: user.id.clone(),
                key_id: None,
                label: String::new(),
                cursor: 0,
                granted,
            });
            return Vec::new();
        }
        // A machine enrols itself; there is nothing to create here.
        Tab::Machines => {
            app.toast(Level::Info, "a machine is created by `cctui enroll` on the machine");
            return Vec::new();
        }
    };
    app.access.mode = Some(Mode::Form { kind, text: String::new() });
    Vec::new()
}

fn start_rename(app: &mut App) -> Vec<Effect> {
    match app.access.tab {
        Tab::Users => {
            let Some(user) = app.access.user() else { return Vec::new() };
            let text = user.name.clone();
            app.access.mode =
                Some(Mode::Form { kind: FormKind::RenameUser { id: user.id.clone() }, text });
        }
        Tab::Tokens => {
            let (Some(user), Some(index)) = (app.access.user(), app.access.cursor()) else {
                return Vec::new();
            };
            let Some(token) = app.access.tokens.get(index) else { return Vec::new() };
            let kind =
                FormKind::RelabelToken { user_id: user.id.clone(), token_id: token.id.clone() };
            let text = token.label.clone().unwrap_or_default();
            app.access.mode = Some(Mode::Form { kind, text });
        }
        Tab::Machines | Tab::Keys => {}
    }
    Vec::new()
}

fn toggle_disable(app: &mut App) -> Vec<Effect> {
    if app.access.tab != Tab::Users {
        return Vec::new();
    }
    let Some(user) = app.access.user() else { return Vec::new() };
    if user.revoked() {
        app.toast(Level::Warn, "a revoked user cannot be re-enabled");
        return Vec::new();
    }
    let disabled = user.disabled_at.is_none();
    vec![Effect::Access(Box::new(AccessEffect::UpdateUser {
        id: user.id.clone(),
        name: None,
        disabled: Some(disabled),
    }))]
}

fn start_revoke(app: &mut App) -> Vec<Effect> {
    let Some(index) = app.access.cursor() else { return Vec::new() };
    let user_id = app.access.user().map(|u| u.id.clone());
    let pending = match app.access.tab {
        Tab::Users => app
            .access
            .users
            .get(index)
            .map(|u| Pending::RevokeUser { id: u.id.clone(), name: u.name.clone() }),
        Tab::Tokens => user_id
            .zip(app.access.tokens.get(index))
            .map(|(user_id, t)| Pending::RevokeToken { user_id, token_id: t.id.clone() }),
        Tab::Machines => app
            .access
            .machines
            .get(index)
            .map(|m| Pending::RevokeMachine { id: m.id.clone(), name: m.label().to_owned() }),
        Tab::Keys => user_id
            .zip(app.access.keys.get(index))
            .map(|(user_id, k)| Pending::RevokeKey { user_id, key_id: k.id.clone() }),
    };
    if let Some(pending) = pending {
        app.access.mode = Some(Mode::Confirm(pending));
    }
    Vec::new()
}

fn start_rotate(app: &mut App) -> Vec<Effect> {
    if app.access.tab != Tab::Machines {
        return Vec::new();
    }
    let Some(index) = app.access.cursor() else { return Vec::new() };
    if let Some(machine) = app.access.machines.get(index) {
        app.access.mode = Some(Mode::Confirm(Pending::RotateMachine {
            id: machine.id.clone(),
            name: machine.label().to_owned(),
        }));
    }
    Vec::new()
}

/// The scope dialog over an existing key, prefilled with what it already holds.
fn start_key_scopes(app: &mut App) -> Vec<Effect> {
    if app.access.tab != Tab::Keys {
        return Vec::new();
    }
    let (Some(user), Some(index)) = (app.access.user(), app.access.cursor()) else {
        return Vec::new();
    };
    let user_id = user.id.clone();
    let Some(key) = app.access.keys.get(index) else { return Vec::new() };
    if key.revoked_at.is_some() {
        app.toast(Level::Warn, "a revoked key cannot be re-granted");
        return Vec::new();
    }
    let granted = ALL_SCOPES.iter().map(|s| key.scopes.iter().any(|k| k == s)).collect();
    app.access.mode = Some(Mode::KeyScopes {
        user_id,
        key_id: Some(key.id.clone()),
        label: key.label.clone().unwrap_or_default(),
        cursor: 0,
        granted,
    });
    Vec::new()
}

fn start_purge(app: &mut App) -> Vec<Effect> {
    let Some(index) = app.access.cursor() else { return Vec::new() };
    let user_id = app.access.user().map(|u| u.id.clone());
    let what = match app.access.tab {
        Tab::Users => {
            app.access.users.get(index).map(|u| (Purge::User { id: u.id.clone() }, u.name.clone()))
        }
        Tab::Tokens => user_id.zip(app.access.tokens.get(index)).map(|(user_id, t)| {
            let name = t.label.clone().unwrap_or_else(|| short(&t.id));
            (Purge::Token { user_id, token_id: t.id.clone() }, name)
        }),
        Tab::Machines => app
            .access
            .machines
            .get(index)
            .map(|m| (Purge::Machine { id: m.id.clone() }, m.label().to_owned())),
        // The server offers no key purge: a revoke is the end of a key.
        Tab::Keys => None,
    };
    match what {
        Some((what, name)) => {
            app.access.mode = Some(Mode::TypedConfirm { what, name, typed: String::new() });
        }
        None if app.access.tab == Tab::Keys => {
            app.toast(Level::Info, "a key is revoked, not purged");
        }
        None => {}
    }
    Vec::new()
}

/// A short stand-in name for a row that has no label of its own.
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn key_in_mode(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode, KeyModifiers};
    let Some(mode) = app.access.mode.as_mut() else { return };
    let target = match mode {
        Mode::Form { text, .. } => text,
        Mode::TypedConfirm { typed, .. } => typed,
        Mode::KeyScopes { label, .. } => label,
        // A confirmation and the secret dialog take no text.
        Mode::Confirm(_) | Mode::Secret { .. } | Mode::Browse => return,
    };
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => target.push(c),
        KeyCode::Backspace => {
            target.pop();
        }
        _ => {}
    }
}

#[allow(clippy::too_many_lines)]
fn commit(app: &mut App) -> Vec<Effect> {
    let Some(mode) = app.access.mode.clone() else { return Vec::new() };
    match mode {
        // Enter on the Users tab opens that user's tokens.
        Mode::Browse if app.access.tab == Tab::Users => switch_tab(app, 1),
        Mode::Browse => Vec::new(),
        Mode::Form { kind, text } => {
            let value = text.trim().to_owned();
            if value.is_empty() && !kind.optional() {
                app.toast(Level::Warn, "a name is required");
                return Vec::new();
            }
            let label = (!value.is_empty()).then_some(value.clone());
            app.access.mode = None;
            let effect = match kind {
                FormKind::NewUser => AccessEffect::CreateUser { name: value },
                FormKind::RenameUser { id } => {
                    AccessEffect::UpdateUser { id, name: Some(value), disabled: None }
                }
                FormKind::MintToken { user_id } => AccessEffect::MintToken { user_id, label },
                FormKind::RelabelToken { user_id, token_id } => {
                    AccessEffect::RelabelToken { user_id, token_id, label }
                }
            };
            vec![Effect::Access(Box::new(effect))]
        }
        Mode::Confirm(pending) => {
            app.access.mode = None;
            let user_id = app.access.user().map(|u| u.id.clone()).unwrap_or_default();
            let effect = match pending {
                Pending::RevokeUser { id, .. } => AccessEffect::RevokeUser { id },
                Pending::RevokeToken { user_id, token_id } => {
                    AccessEffect::RevokeToken { user_id, token_id }
                }
                Pending::RevokeMachine { id, .. } => AccessEffect::RevokeMachine { user_id, id },
                Pending::RotateMachine { id, .. } => AccessEffect::RotateMachine { user_id, id },
                Pending::RevokeKey { user_id, key_id } => {
                    AccessEffect::RevokeKey { user_id, key_id }
                }
            };
            vec![Effect::Access(Box::new(effect))]
        }
        Mode::TypedConfirm { what, name, typed } => {
            if typed.trim() != name {
                app.toast(Level::Warn, format!("type {name} exactly to purge it"));
                return Vec::new();
            }
            app.access.mode = None;
            let effect = match what {
                Purge::User { id } => AccessEffect::PurgeUser { id },
                Purge::Token { user_id, token_id } => {
                    AccessEffect::PurgeToken { user_id, token_id }
                }
                Purge::Machine { id } => {
                    let user_id = app.access.user().map(|u| u.id.clone()).unwrap_or_default();
                    AccessEffect::PurgeMachine { user_id, id }
                }
            };
            vec![Effect::Access(Box::new(effect))]
        }
        Mode::KeyScopes { user_id, key_id, label, granted, .. } => {
            let scopes: Vec<String> = ALL_SCOPES
                .iter()
                .zip(&granted)
                .filter(|(_, on)| **on)
                .map(|(name, _)| (*name).to_owned())
                .collect();
            if scopes.is_empty() {
                app.toast(Level::Warn, "a key with no scope can do nothing — grant at least one");
                return Vec::new();
            }
            app.access.mode = None;
            let effect = if let Some(key_id) = key_id {
                AccessEffect::SetKeyScopes { user_id, key_id, scopes }
            } else {
                let label = label.trim();
                AccessEffect::MintKey {
                    user_id,
                    label: (!label.is_empty()).then(|| label.to_owned()),
                    scopes,
                }
            };
            vec![Effect::Access(Box::new(effect))]
        }
        // Enter dismisses the secret, and the buffer goes with it.
        Mode::Secret { .. } => {
            app.access.forget_secret();
            Vec::new()
        }
    }
}

/// The scopes the mint dialog offers, each with whether the owner's ceiling
/// allows it. Granting outside the ceiling is refused server-side, so the
/// dialog shows it as unavailable rather than letting the mint fail.
#[must_use]
pub fn mintable_scopes(ceiling: &[String]) -> Vec<(&'static str, bool)> {
    ALL_SCOPES.iter().map(|name| (*name, ceiling.iter().any(|c| c == name))).collect()
}

/// Everything the detail tabs need for one user. A missing ACL or key list is
/// not fatal: an admin reading another user's ceiling can be refused while the
/// tokens still list.
pub async fn load_detail(
    server: &Client,
    user_id: &str,
) -> Result<AccessAction, cctui_client::ClientError> {
    let tokens = server.user_tokens(user_id).await?;
    let machines = server.user_machines(user_id).await?;
    let keys = server.user_keys(user_id).await.unwrap_or_default();
    let ceiling = server.user_acls(user_id).await.map(|a| a.scopes).unwrap_or_default();
    Ok(AccessAction::DetailLoaded { tokens, machines, keys, ceiling })
}

/// Runs one access effect. Errors become a toast or the panel's error line;
/// a reply that carries a secret becomes the one dialog that holds it.
#[allow(clippy::too_many_lines)]
pub async fn run(effect: AccessEffect, server: &Client) -> Vec<Action> {
    let done = |message: String| vec![Action::Access(AccessAction::Done(message))];
    let failed = |what: &str, e: &cctui_client::ClientError| {
        tracing::warn!(%e, what, "an access mutation failed");
        vec![Action::Access(AccessAction::Failed(format!("{what} failed: {e}")))]
    };
    match effect {
        AccessEffect::Users => match server.users().await {
            Ok(users) => vec![Action::Access(AccessAction::UsersLoaded(users))],
            Err(e) => {
                tracing::warn!(%e, "listing users failed");
                vec![Action::Access(AccessAction::Failed(list_error(&e)))]
            }
        },
        AccessEffect::Detail { user_id } => match load_detail(server, &user_id).await {
            Ok(action) => vec![Action::Access(action)],
            Err(e) => failed("loading the user", &e),
        },
        AccessEffect::CreateUser { name } => match server.create_user(&name).await {
            // The reply's `key` is the user's only credential and is never logged.
            Ok(created) => {
                vec![Action::Access(AccessAction::Secret { what: "user key", secret: created.key })]
            }
            Err(e) => failed("creating the user", &e),
        },
        AccessEffect::UpdateUser { id, name, disabled } => {
            let request = UpdateUser { name: name.clone(), can_dispatch: None, disabled };
            match server.update_user(&id, &request).await {
                Ok(()) => done(match (name, disabled) {
                    (Some(name), _) => format!("renamed to {name}"),
                    (_, Some(true)) => "user disabled".to_owned(),
                    (_, Some(false)) => "user enabled".to_owned(),
                    _ => "user updated".to_owned(),
                }),
                Err(e) => failed("updating the user", &e),
            }
        }
        AccessEffect::RevokeUser { id } => match server.revoke_user(&id).await {
            Ok(()) => done("user revoked".to_owned()),
            Err(e) => failed("revoking the user", &e),
        },
        AccessEffect::PurgeUser { id } => match server.purge_user(&id).await {
            Ok(()) => done("user purged".to_owned()),
            Err(e) => failed("purging the user", &e),
        },
        AccessEffect::MintToken { user_id, label } => {
            match server.mint_token(&user_id, label.as_deref()).await {
                Ok(minted) => vec![Action::Access(AccessAction::Secret {
                    what: "user token",
                    secret: minted.token,
                })],
                Err(e) => failed("minting the token", &e),
            }
        }
        AccessEffect::RelabelToken { user_id, token_id, label } => {
            match server.relabel_token(&user_id, &token_id, label.as_deref()).await {
                Ok(()) => done("token relabelled".to_owned()),
                Err(e) => failed("relabelling the token", &e),
            }
        }
        AccessEffect::RevokeToken { user_id, token_id } => {
            match server.revoke_token(&user_id, &token_id).await {
                Ok(()) => done("token revoked".to_owned()),
                Err(e) => failed("revoking the token", &e),
            }
        }
        AccessEffect::PurgeToken { user_id, token_id } => {
            match server.purge_token(&user_id, &token_id).await {
                Ok(()) => done("token purged".to_owned()),
                Err(e) => failed("purging the token", &e),
            }
        }
        AccessEffect::RevokeMachine { id, .. } => match server.revoke_machine(&id).await {
            Ok(()) => done("machine revoked".to_owned()),
            Err(e) => failed("revoking the machine", &e),
        },
        AccessEffect::PurgeMachine { id, .. } => match server.purge_machine(&id).await {
            Ok(()) => done("machine purged".to_owned()),
            Err(e) => failed("purging the machine", &e),
        },
        AccessEffect::RotateMachine { id, .. } => match server.rotate_machine(&id).await {
            Ok(rotated) => vec![Action::Access(AccessAction::Secret {
                what: "machine key",
                secret: rotated.key,
            })],
            Err(e) => failed("rotating the machine key", &e),
        },
        AccessEffect::MintKey { user_id, label, scopes } => {
            let request = MintKey { label, scopes, expires_at: None };
            match server.mint_key(&user_id, &request).await {
                Ok(minted) => vec![Action::Access(AccessAction::Secret {
                    what: "api key",
                    secret: minted.key,
                })],
                Err(e) => failed("minting the key", &e),
            }
        }
        AccessEffect::SetKeyScopes { user_id, key_id, scopes } => {
            match server.set_key_scopes(&user_id, &key_id, &scopes).await {
                Ok(()) => done(format!("key scopes set to {}", scopes.join(", "))),
                Err(e) => failed("setting the key scopes", &e),
            }
        }
        AccessEffect::RevokeKey { user_id, key_id } => {
            match server.revoke_key(&user_id, &key_id).await {
                Ok(()) => done("key revoked".to_owned()),
                Err(e) => failed("revoking the key", &e),
            }
        }
    }
}

/// Why the user list is empty, in the words the view shows.
fn list_error(e: &cctui_client::ClientError) -> String {
    match e {
        cctui_client::ClientError::Forbidden { .. } => "this key may not list users".to_owned(),
        other => format!("could not list users: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use cctui_client::{ApiKey, User, UserMachine, UserToken};

    use super::{AccessAction, AccessEffect, Mode, Pending, Purge, Tab, is_admin};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};

    const SECRET: &str = "cctui_u_SUPERSECRET";

    fn stamp() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(0).expect("a stamp")
    }

    fn user(id: &str, name: &str, disabled: bool, revoked: bool) -> User {
        User {
            id: id.to_owned(),
            name: name.to_owned(),
            created_at: stamp(),
            revoked_at: revoked.then(stamp),
            disabled_at: disabled.then(stamp),
            can_dispatch: true,
            last_seen_at: None,
        }
    }

    fn token(id: &str, label: Option<&str>, revoked: bool) -> UserToken {
        UserToken {
            id: id.to_owned(),
            label: label.map(str::to_owned),
            created_at: stamp(),
            expires_at: None,
            revoked_at: revoked.then(stamp),
            token_preview: Some("cctui_u_ab12…ef34".to_owned()),
        }
    }

    fn machine(id: &str, name: &str) -> UserMachine {
        UserMachine {
            id: id.to_owned(),
            name: name.to_owned(),
            display_name: None,
            last_seen_at: stamp(),
            revoked_at: None,
            kind: "persistent".to_owned(),
            key_preview: Some("cctui_m_cd34…ab12".to_owned()),
            liveness: cctui_proto::models::MachineLiveness::Online,
        }
    }

    fn key(id: &str, label: &str, scopes: &[&str]) -> ApiKey {
        ApiKey {
            id: id.to_owned(),
            label: Some(label.to_owned()),
            key_preview: Some("cctui_k_ef56…7890".to_owned()),
            kind: "user".to_owned(),
            created_at: stamp(),
            expires_at: None,
            revoked_at: None,
            last_used_at: None,
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn app_with_scope(scopes: &[&str]) -> App {
        let mut app = App::new();
        app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
            role: "admin".to_owned(),
            user_name: Some("dorsk".to_owned()),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            token_preview: "abc".to_owned(),
        });
        app
    }

    fn act(app: &mut App, action: AccessAction) -> Vec<Effect> {
        reduce(app, Action::Access(action))
    }

    fn effects(effects: Vec<Effect>) -> Vec<AccessEffect> {
        effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Access(effect) => Some(*effect),
                _ => None,
            })
            .collect()
    }

    fn loaded(app: &mut App) {
        act(
            app,
            AccessAction::UsersLoaded(vec![
                user("u-1", "dorsk", false, false),
                user("u-2", "nanachi", true, false),
            ]),
        );
        act(
            app,
            AccessAction::DetailLoaded {
                tokens: vec![token("t-1", Some("laptop"), false), token("t-2", None, true)],
                machines: vec![machine("m-1", "thinkpad")],
                keys: vec![key("k-1", "ci", &["read"])],
                ceiling: vec!["read".to_owned(), "dispatch".to_owned()],
            },
        );
    }

    fn admin_app() -> App {
        let mut app = app_with_scope(&["admin"]);
        act(&mut app, AccessAction::Open);
        loaded(&mut app);
        app
    }

    #[test]
    fn a_non_admin_cannot_open_the_slice_or_reach_it_by_its_number() {
        let mut app = app_with_scope(&["read", "dispatch", "enroll"]);
        assert!(!is_admin(&app));

        assert!(act(&mut app, AccessAction::Open).is_empty());
        assert!(!app.access.open);
        assert_ne!(app.view(), crate::app::View::Access);
        assert_eq!(app.toasts.latest().expect("a toast").text, super::DENIED);

        // The tab number is the other way in, and it is gated by the same scope.
        let access = crate::app::slice::TABS
            .iter()
            .position(|t| t.slice == Some(crate::app::slice::Slice::Access))
            .expect("the access tab");
        reduce(&mut app, Action::Slice(crate::app::slice::SliceAction::Switch(access + 1)));
        assert_ne!(app.slice, crate::app::slice::Slice::Access);
        assert_ne!(app.view(), crate::app::View::Access);
    }

    #[test]
    fn an_unidentified_key_is_not_an_admin() {
        let mut app = App::new();
        assert!(!is_admin(&app));
        assert!(act(&mut app, AccessAction::Open).is_empty());
    }

    #[test]
    fn opening_lists_users_then_the_selected_users_detail() {
        let mut app = app_with_scope(&["admin"]);
        assert_eq!(effects(act(&mut app, AccessAction::Open)), vec![AccessEffect::Users]);
        assert_eq!(app.view(), crate::app::View::Access);

        let next =
            act(&mut app, AccessAction::UsersLoaded(vec![user("u-1", "dorsk", false, false)]));
        assert_eq!(effects(next), vec![AccessEffect::Detail { user_id: "u-1".to_owned() }]);
    }

    #[test]
    fn the_tabs_cycle_and_each_one_is_about_the_selected_user() {
        let mut app = admin_app();
        assert_eq!(app.access.tab, Tab::Users);
        act(&mut app, AccessAction::SelectNext);
        assert_eq!(app.access.user().expect("a user").name, "nanachi");

        let next = act(&mut app, AccessAction::NextTab);
        assert_eq!(app.access.tab, Tab::Tokens);
        assert_eq!(effects(next), vec![AccessEffect::Detail { user_id: "u-2".to_owned() }]);
        act(&mut app, AccessAction::PrevTab);
        assert_eq!(app.access.tab, Tab::Users);
        act(&mut app, AccessAction::PrevTab);
        assert_eq!(app.access.tab, Tab::Keys, "the tabs wrap");
    }

    #[test]
    fn revoked_rows_are_hidden_until_they_are_asked_for() {
        let mut app = admin_app();
        act(&mut app, AccessAction::NextTab);
        assert_eq!(app.access.rows(), vec![0], "the revoked token is out of the way");
        act(&mut app, AccessAction::ToggleRevoked);
        assert_eq!(app.access.rows(), vec![0, 1]);
    }

    #[test]
    fn creating_a_user_posts_the_name_and_shows_its_key_once() {
        let mut app = admin_app();
        act(&mut app, AccessAction::StartNew);
        for c in "bot".chars() {
            act(
                &mut app,
                AccessAction::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
        }
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::CreateUser { name: "bot".to_owned() }]
        );
    }

    #[test]
    fn a_nameless_user_is_refused_rather_than_sent() {
        let mut app = admin_app();
        act(&mut app, AccessAction::StartNew);
        assert!(act(&mut app, AccessAction::Commit).is_empty());
        assert!(matches!(app.access.mode(), Mode::Form { .. }), "still asking");
    }

    #[test]
    fn renaming_prefills_the_current_name_and_patches_the_user() {
        let mut app = admin_app();
        act(&mut app, AccessAction::StartRename);
        match app.access.mode() {
            Mode::Form { text, .. } => assert_eq!(text, "dorsk"),
            other => panic!("expected the rename form, got {other:?}"),
        }
        act(
            &mut app,
            AccessAction::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('2'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::UpdateUser {
                id: "u-1".to_owned(),
                name: Some("dorsk2".to_owned()),
                disabled: None,
            }]
        );
    }

    #[test]
    fn disable_is_a_switch_both_ways_but_not_for_a_revoked_user() {
        let mut app = admin_app();
        assert_eq!(
            effects(act(&mut app, AccessAction::ToggleDisable)),
            vec![AccessEffect::UpdateUser {
                id: "u-1".to_owned(),
                name: None,
                disabled: Some(true),
            }]
        );
        act(&mut app, AccessAction::SelectNext);
        assert_eq!(
            effects(act(&mut app, AccessAction::ToggleDisable)),
            vec![AccessEffect::UpdateUser {
                id: "u-2".to_owned(),
                name: None,
                disabled: Some(false),
            }],
            "an already disabled user is re-enabled"
        );

        act(&mut app, AccessAction::UsersLoaded(vec![user("u-9", "gone", false, true)]));
        assert!(act(&mut app, AccessAction::ToggleDisable).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("revoked"));
    }

    #[test]
    fn every_tab_revokes_the_row_it_is_on_through_its_own_endpoint() {
        let mut app = admin_app();
        act(&mut app, AccessAction::StartRevoke);
        assert!(matches!(app.access.mode(), Mode::Confirm(Pending::RevokeUser { .. })));
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::RevokeUser { id: "u-1".to_owned() }]
        );

        act(&mut app, AccessAction::NextTab);
        act(&mut app, AccessAction::StartRevoke);
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::RevokeToken {
                user_id: "u-1".to_owned(),
                token_id: "t-1".to_owned(),
            }]
        );

        act(&mut app, AccessAction::NextTab);
        act(&mut app, AccessAction::StartRevoke);
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::RevokeMachine { user_id: "u-1".to_owned(), id: "m-1".to_owned() }]
        );

        act(&mut app, AccessAction::NextTab);
        act(&mut app, AccessAction::StartRevoke);
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::RevokeKey { user_id: "u-1".to_owned(), key_id: "k-1".to_owned() }]
        );
    }

    #[test]
    fn a_revoke_can_be_backed_out_of_before_it_is_sent() {
        let mut app = admin_app();
        act(&mut app, AccessAction::StartRevoke);
        act(&mut app, AccessAction::Cancel);
        assert_eq!(app.access.mode, None);
        assert!(app.access.open, "backing out of a dialog does not leave the slice");
    }

    #[test]
    fn rotating_a_machine_key_asks_first_and_only_on_the_machines_tab() {
        let mut app = admin_app();
        assert!(act(&mut app, AccessAction::StartRotate).is_empty());
        assert_eq!(app.access.mode, None, "there is nothing to rotate on the users tab");

        act(&mut app, AccessAction::NextTab);
        act(&mut app, AccessAction::NextTab);
        assert_eq!(app.access.tab, Tab::Machines);
        act(&mut app, AccessAction::StartRotate);
        assert!(matches!(app.access.mode(), Mode::Confirm(Pending::RotateMachine { .. })));
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::RotateMachine { user_id: "u-1".to_owned(), id: "m-1".to_owned() }]
        );
    }

    #[test]
    fn a_purge_needs_the_name_typed_out_exactly() {
        let mut app = admin_app();
        act(&mut app, AccessAction::StartPurge);
        assert!(matches!(app.access.mode(), Mode::TypedConfirm { what: Purge::User { .. }, .. }));

        assert!(act(&mut app, AccessAction::Commit).is_empty(), "an empty answer purges nothing");
        for c in "dors".chars() {
            act(
                &mut app,
                AccessAction::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
        }
        assert!(act(&mut app, AccessAction::Commit).is_empty(), "a near miss purges nothing");
        act(
            &mut app,
            AccessAction::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('k'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::PurgeUser { id: "u-1".to_owned() }]
        );
    }

    #[test]
    fn minting_a_key_sends_the_scopes_that_were_ticked() {
        let mut app = admin_app();
        for _ in 0..3 {
            act(&mut app, AccessAction::NextTab);
        }
        assert_eq!(app.access.tab, Tab::Keys);
        act(&mut app, AccessAction::StartNew);

        assert!(act(&mut app, AccessAction::Commit).is_empty(), "no scope, no key");
        assert!(app.toasts.latest().expect("a toast").text.contains("at least one"));

        act(&mut app, AccessAction::ToggleScope);
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::MintKey {
                user_id: "u-1".to_owned(),
                label: None,
                scopes: vec!["read".to_owned()],
            }]
        );
    }

    #[test]
    fn re_granting_a_key_prefills_its_scopes_and_leaves_the_secret_alone() {
        let mut app = admin_app();
        for _ in 0..3 {
            act(&mut app, AccessAction::NextTab);
        }
        assert_eq!(app.access.tab, Tab::Keys);
        act(&mut app, AccessAction::StartKeyScopes);
        match app.access.mode() {
            Mode::KeyScopes { key_id, granted, label, .. } => {
                assert_eq!(key_id.as_deref(), Some("k-1"));
                assert_eq!(label, "ci");
                assert_eq!(granted, vec![true, false, false, false], "read is already held");
            }
            other => panic!("expected the scope dialog, got {other:?}"),
        }

        // Dropping the only scope it holds is refused: a key with none is useless.
        act(&mut app, AccessAction::ToggleScope);
        assert!(act(&mut app, AccessAction::Commit).is_empty());
        act(&mut app, AccessAction::ToggleScope);

        // j/k walks the checkboxes while the dialog is up, so dispatch is reachable.
        act(&mut app, AccessAction::SelectNext);
        act(&mut app, AccessAction::ToggleScope);
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::SetKeyScopes {
                user_id: "u-1".to_owned(),
                key_id: "k-1".to_owned(),
                scopes: vec!["read".to_owned(), "dispatch".to_owned()],
            }]
        );
    }

    #[test]
    fn re_granting_is_a_patch_not_a_mint_so_no_secret_comes_back() {
        let mut app = admin_app();
        for _ in 0..3 {
            act(&mut app, AccessAction::NextTab);
        }
        act(&mut app, AccessAction::StartKeyScopes);
        match effects(act(&mut app, AccessAction::Commit)).as_slice() {
            [AccessEffect::SetKeyScopes { .. }] => {}
            other => panic!("expected an in-place patch, got {other:?}"),
        }
        assert_eq!(app.access.mode, None);
        assert!(
            !matches!(app.access.mode(), Mode::Secret { .. }),
            "editing a grant never shows a secret"
        );
    }

    #[test]
    fn a_revoked_key_cannot_be_re_granted() {
        let mut app = admin_app();
        act(
            &mut app,
            AccessAction::DetailLoaded {
                tokens: Vec::new(),
                machines: Vec::new(),
                keys: vec![ApiKey { revoked_at: Some(stamp()), ..key("k-9", "old", &["read"]) }],
                ceiling: vec!["read".to_owned()],
            },
        );
        for _ in 0..3 {
            act(&mut app, AccessAction::NextTab);
        }
        act(&mut app, AccessAction::ToggleRevoked);
        act(&mut app, AccessAction::StartKeyScopes);
        assert_eq!(app.access.mode, None);
        assert!(app.toasts.latest().expect("a toast").text.contains("revoked"));
    }

    #[test]
    fn the_scope_dialog_is_only_on_the_keys_tab() {
        let mut app = admin_app();
        assert!(act(&mut app, AccessAction::StartKeyScopes).is_empty());
        assert_eq!(app.access.mode, None);
    }

    #[test]
    fn the_mint_dialog_marks_the_scopes_the_users_ceiling_does_not_allow() {
        let app = admin_app();
        assert_eq!(
            super::mintable_scopes(&app.access.ceiling),
            vec![("read", true), ("dispatch", true), ("enroll", false), ("admin", false),]
        );
    }

    #[test]
    fn minting_a_token_takes_an_optional_label() {
        let mut app = admin_app();
        act(&mut app, AccessAction::NextTab);
        act(&mut app, AccessAction::StartNew);
        assert_eq!(
            effects(act(&mut app, AccessAction::Commit)),
            vec![AccessEffect::MintToken { user_id: "u-1".to_owned(), label: None }],
            "a blank label is a legal answer"
        );
    }

    #[test]
    fn the_dispatchers_panel_is_linked_rather_than_rebuilt() {
        let mut app = admin_app();
        act(&mut app, AccessAction::OpenDispatchers);
        assert_eq!(app.view(), crate::app::View::Dispatchers);
    }

    #[test]
    fn a_failed_listing_is_reported_on_the_panel() {
        let mut app = app_with_scope(&["admin"]);
        act(&mut app, AccessAction::Open);
        act(&mut app, AccessAction::Failed("this key may not list users".to_owned()));
        assert!(!app.access.loading);
        assert_eq!(app.access.error.as_deref(), Some("this key may not list users"));
    }

    #[test]
    fn the_cursor_stays_inside_the_rows_a_tab_actually_has() {
        let mut app = admin_app();
        for _ in 0..9 {
            act(&mut app, AccessAction::SelectNext);
        }
        assert_eq!(app.access.row, 1);
        act(&mut app, AccessAction::UsersLoaded(vec![user("u-1", "dorsk", false, false)]));
        assert_eq!(app.access.row, 0, "a shorter list brings it back");
    }

    #[test]
    fn a_minted_secret_is_shown_once_and_never_told_to_anything_else() {
        let mut app = admin_app();
        act(&mut app, AccessAction::Secret { what: "api key", secret: SECRET.to_owned() });
        match app.access.mode() {
            Mode::Secret { what, secret } => {
                assert_eq!(what, "api key");
                assert_eq!(secret, SECRET);
            }
            other => panic!("expected the secret dialog, got {other:?}"),
        }
        // Not a toast, not the status line, not the composer, not a draft.
        assert!(app.toasts.latest().is_none_or(|t| !t.text.contains(SECRET)));
        assert!(!format!("{:?}", app.drafts).contains(SECRET));
        assert!(!app.message_input.lines().join("\n").contains(SECRET));
    }

    #[test]
    fn copying_a_secret_names_the_clipboard_and_not_the_secret() {
        let mut app = admin_app();
        act(&mut app, AccessAction::Secret { what: "machine key", secret: SECRET.to_owned() });
        match act(&mut app, AccessAction::CopySecret).as_slice() {
            [Effect::Copy { text, label }] => {
                assert_eq!(text, SECRET);
                assert_eq!(*label, "machine key", "the label is what gets said, not the key");
            }
            other => panic!("expected one clipboard effect, got {}", other.len()),
        }
    }

    /// The acceptance criterion: once the dialog is closed, no later render and
    /// no part of the app's own state can produce the secret again.
    #[test]
    fn a_closed_secret_dialog_leaves_the_secret_in_no_later_render() {
        let mut app = admin_app();
        act(&mut app, AccessAction::Secret { what: "api key", secret: SECRET.to_owned() });
        let shown = crate::testsupport::render_screen(&mut app);
        assert!(shown.contains("SUPERSECRET"), "it is shown once: {shown}");

        act(&mut app, AccessAction::Commit);
        assert_eq!(app.access.mode, None);
        let after = crate::testsupport::render_screen(&mut app);
        assert!(!after.contains("SUPERSECRET"), "the secret came back on a later render: {after}");
        assert!(!format!("{:?}", app.access).contains("SUPERSECRET"), "the state still holds it");

        // Leaving and re-entering the slice cannot recover it either.
        act(&mut app, AccessAction::Close);
        act(&mut app, AccessAction::Open);
        let reopened = crate::testsupport::render_screen(&mut app);
        assert!(!reopened.contains("SUPERSECRET"), "reopening showed it again: {reopened}");
    }

    #[test]
    fn escaping_the_secret_dialog_also_wipes_the_buffer() {
        let mut app = admin_app();
        act(&mut app, AccessAction::Secret { what: "user key", secret: SECRET.to_owned() });
        act(&mut app, AccessAction::Cancel);
        assert_eq!(app.access.mode, None);
        assert!(!format!("{:?}", app.access).contains("SUPERSECRET"));
        let after = crate::testsupport::render_screen(&mut app);
        assert!(!after.contains("SUPERSECRET"), "{after}");
    }
}

/// Captures every tracing event emitted on this thread, so a test can assert
/// that a secret never reaches the log. A hand-rolled subscriber keeps this
/// free of a `tracing-subscriber` dependency.
#[cfg(test)]
mod log_capture {
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct Captured(Arc<Mutex<String>>);

    impl Captured {
        pub fn text(&self) -> String {
            self.0.lock().expect("the capture lock").clone()
        }

        pub fn handle(&self) -> Arc<Mutex<String>> {
            Arc::clone(&self.0)
        }
    }

    pub struct Subscriber(pub Arc<Mutex<String>>);

    struct Visitor(Arc<Mutex<String>>);

    impl tracing::field::Visit for Visitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let line = format!("{}={value:?}\n", field.name());
            let mut sink = self.0.lock().expect("the capture lock");
            let _ = sink.write_str(&line);
        }
    }

    impl tracing::Subscriber for Subscriber {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::Id, values: &tracing::span::Record<'_>) {
            values.record(&mut Visitor(Arc::clone(&self.0)));
        }

        fn record_follows_from(&self, _: &tracing::Id, _: &tracing::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut sink = self.0.lock().expect("the capture lock");
            sink.push_str(event.metadata().name());
            sink.push('\n');
            drop(sink);
            event.record(&mut Visitor(Arc::clone(&self.0)));
        }

        fn enter(&self, _: &tracing::Id) {}

        fn exit(&self, _: &tracing::Id) {}
    }
}

#[cfg(test)]
mod log_tests {
    use super::log_capture::{Captured, Subscriber};
    use super::{AccessAction, Mode};
    use crate::app::{Action, App, reduce};

    const SECRET: &str = "cctui_k_NEVERLOGGED";

    /// The acceptance criterion: minting, showing, copying and dismissing a
    /// secret emits no log line that carries it.
    #[test]
    fn a_secret_never_reaches_the_log() {
        let captured = Captured::default();
        let guard = tracing::subscriber::set_default(Subscriber(captured.handle()));

        let mut app = App::new();
        app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
            role: "admin".to_owned(),
            user_name: Some("dorsk".to_owned()),
            scopes: vec!["admin".to_owned()],
            token_preview: "abc".to_owned(),
        });
        reduce(&mut app, Action::Access(AccessAction::Open));
        reduce(
            &mut app,
            Action::Access(AccessAction::Secret { what: "api key", secret: SECRET.to_owned() }),
        );
        assert!(matches!(app.access.mode(), Mode::Secret { .. }));
        reduce(&mut app, Action::Access(AccessAction::CopySecret));
        reduce(&mut app, Action::Access(AccessAction::Commit));

        // A failure on a neighbouring path still logs, so the capture is live.
        reduce(&mut app, Action::Access(AccessAction::Failed("forbidden".to_owned())));
        tracing::warn!("capture is wired");

        let log = captured.text();
        drop(guard);
        assert!(log.contains("capture is wired"), "the capture caught nothing: {log:?}");
        assert!(!log.contains(SECRET), "the secret reached the log: {log}");
        assert!(!log.contains("NEVERLOGGED"), "a fragment of the secret reached the log: {log}");
    }
}
