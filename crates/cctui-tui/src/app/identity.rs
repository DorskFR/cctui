//! Who the configured key belongs to, and what it is allowed to do.

use cctui_proto::api::me::MeResponse;

use super::action::Effect;
use super::state::App;

/// The identity behind the key, once `GET /me` has answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub role: String,
    pub user_name: Option<String>,
    pub scopes: Vec<String>,
    pub token_preview: String,
}

impl Identity {
    #[must_use]
    pub fn from_response(me: MeResponse) -> Self {
        Self {
            role: me.role,
            user_name: me.user_name,
            scopes: me.scopes,
            token_preview: me.token_preview,
        }
    }

    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.has_scope("admin")
    }

    #[must_use]
    pub fn label(&self) -> String {
        self.user_name
            .as_deref()
            .map_or_else(|| self.role.clone(), |name| format!("{name} ({})", self.role))
    }
}

/// What the TUI currently knows about its credential.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuthState {
    /// `GET /me` has not answered yet, or the network is down.
    #[default]
    Unknown,
    Identified(Identity),
    /// The server refused the key: 401.
    Rejected,
}

impl AuthState {
    /// Role gate for admin-only surfaces. An unresolved identity is not admin:
    /// a gate that opens while we are still asking is not a gate.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Identified(id) if id.is_admin())
    }

    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        matches!(self, Self::Identified(id) if id.has_scope(scope))
    }

    #[must_use]
    pub const fn identity(&self) -> Option<&Identity> {
        match self {
            Self::Identified(id) => Some(id),
            _ => None,
        }
    }

    /// The status-bar chip. `None` while unknown, so a TUI that has not asked
    /// yet says nothing rather than guessing.
    #[must_use]
    pub fn chip(&self) -> Option<AuthChip> {
        match self {
            Self::Unknown => None,
            Self::Identified(id) => Some(AuthChip { text: id.label(), rejected: false }),
            Self::Rejected => Some(AuthChip { text: REJECTED_MESSAGE.to_owned(), rejected: true }),
        }
    }
}

/// What a 401 tells the user to do about it.
pub const REJECTED_MESSAGE: &str = "key rejected — run `cctui login`";

/// One status-bar chip: text plus whether it reads as an error.
pub struct AuthChip {
    pub text: String,
    pub rejected: bool,
}

/// Identity transitions, kept out of the shared reducer.
pub enum AuthAction {
    /// `GET /me` answered.
    Identified(Box<MeResponse>),
    /// The server refused the key.
    Rejected,
}

pub fn reduce_auth(app: &mut App, action: AuthAction) -> Vec<Effect> {
    match action {
        AuthAction::Identified(me) => {
            app.auth = AuthState::Identified(Identity::from_response(*me));
        }
        AuthAction::Rejected => app.auth = AuthState::Rejected,
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{AuthAction, AuthState, Identity, REJECTED_MESSAGE, reduce_auth};
    use crate::app::App;
    use cctui_proto::api::me::MeResponse;

    fn me(role: &str, scopes: &[&str]) -> MeResponse {
        MeResponse {
            role: role.to_owned(),
            user_id: None,
            user_name: Some("dorsk".to_owned()),
            machine_id: None,
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            token_preview: "cctui_u_ab12…ef34".to_owned(),
        }
    }

    #[test]
    fn an_unknown_identity_gates_admin_shut() {
        let state = AuthState::Unknown;
        assert!(!state.is_admin());
        assert!(!state.has_scope("read"));
        assert!(state.chip().is_none());
    }

    #[test]
    fn scopes_drive_the_gate_not_the_role_string() {
        let state = AuthState::Identified(Identity::from_response(me("admin", &["read"])));
        assert!(!state.is_admin());
        assert!(state.has_scope("read"));
    }

    #[test]
    fn the_chip_names_the_user_and_role() {
        let mut app = App::new();
        let _ = reduce_auth(&mut app, AuthAction::Identified(Box::new(me("admin", &["admin"]))));
        assert!(app.auth.is_admin());
        assert_eq!(app.auth.chip().expect("chip").text, "dorsk (admin)");
    }

    #[test]
    fn a_rejected_key_says_what_to_run() {
        let mut app = App::new();
        let _ = reduce_auth(&mut app, AuthAction::Rejected);
        let chip = app.auth.chip().expect("chip");
        assert!(chip.rejected);
        assert_eq!(chip.text, REJECTED_MESSAGE);
    }
}
