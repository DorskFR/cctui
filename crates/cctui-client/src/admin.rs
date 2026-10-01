//! `/admin/users/*`, `/admin/machines/*` and `/users/{id}/{acls,keys,tokens}`.
//!
//! The proto structs for these routes are serialize-only, so the rows are
//! mirrored here as deserialize-only. A mint or rotate reply carries the one
//! copy of a secret that exists: the server keeps only its hash.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ClientError;
use crate::rest::Client;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub disabled_at: Option<DateTime<Utc>>,
    #[serde(default = "yes")]
    pub can_dispatch: bool,
    #[serde(default)]
    pub last_seen_at: Option<DateTime<Utc>>,
}

const fn yes() -> bool {
    true
}

impl User {
    #[must_use]
    pub const fn revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// `revoked` outranks `disabled`: one is permanent, the other a switch.
    #[must_use]
    pub const fn state(&self) -> &'static str {
        if self.revoked_at.is_some() {
            "revoked"
        } else if self.disabled_at.is_some() {
            "disabled"
        } else {
            "active"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UserToken {
    pub id: String,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub token_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UserMachine {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub kind: String,
    #[serde(default)]
    pub key_preview: Option<String>,
    #[serde(default)]
    pub liveness: cctui_proto::models::MachineLiveness,
}

impl UserMachine {
    /// What the row is called: the operator's override, else the enrolled name.
    #[must_use]
    pub fn label(&self) -> &str {
        self.display_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub label: Option<String>,
    #[serde(default)]
    pub key_preview: Option<String>,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// The user's scope ceiling: what any of its keys may be granted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UserAcls {
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// A newly created user and its first credential, returned once.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatedUser {
    pub id: String,
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MintedToken {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MintedKey {
    pub id: String,
    pub key: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// A rotated machine or user credential, returned once.
#[derive(Debug, Clone, Deserialize)]
pub struct Rotated {
    pub key: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateUser {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_dispatch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MintKey {
    pub label: Option<String>,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl Client {
    pub async fn users(&self) -> Result<Vec<User>, ClientError> {
        self.get_as("get_admin_users", &[]).await
    }

    pub async fn create_user(&self, name: &str) -> Result<CreatedUser, ClientError> {
        self.post_as("post_admin_users", &[], &serde_json::json!({ "name": name })).await
    }

    pub async fn update_user(&self, id: &str, request: &UpdateUser) -> Result<(), ClientError> {
        let body = serde_json::to_value(request)
            .map_err(|source| ClientError::Decode { route: "patch_admin_users_by_id", source })?;
        self.patch("patch_admin_users_by_id", &[("id", id)], Some(&body)).await.map(drop)
    }

    /// Revoke: permanent, and invalidates every token and machine the user has.
    pub async fn revoke_user(&self, id: &str) -> Result<(), ClientError> {
        self.delete("delete_admin_users_by_id", &[("id", id)]).await.map(drop)
    }

    pub async fn purge_user(&self, id: &str) -> Result<(), ClientError> {
        self.delete("delete_admin_users_by_id_purge", &[("id", id)]).await.map(drop)
    }

    pub async fn user_tokens(&self, id: &str) -> Result<Vec<UserToken>, ClientError> {
        self.get_as("get_admin_users_by_id_tokens", &[("id", id)]).await
    }

    pub async fn user_machines(&self, id: &str) -> Result<Vec<UserMachine>, ClientError> {
        self.get_as("get_admin_users_by_id_machines", &[("id", id)]).await
    }

    pub async fn user_keys(&self, id: &str) -> Result<Vec<ApiKey>, ClientError> {
        self.get_as("get_users_by_id_keys", &[("id", id)]).await
    }

    pub async fn user_acls(&self, id: &str) -> Result<UserAcls, ClientError> {
        self.get_as("get_users_by_id_acls", &[("id", id)]).await
    }

    pub async fn mint_token(
        &self,
        user_id: &str,
        label: Option<&str>,
    ) -> Result<MintedToken, ClientError> {
        self.post_as(
            "post_users_by_id_tokens",
            &[("id", user_id)],
            &serde_json::json!({
                "label": label,
                "expires_at": serde_json::Value::Null,
            }),
        )
        .await
    }

    pub async fn relabel_token(
        &self,
        user_id: &str,
        token_id: &str,
        label: Option<&str>,
    ) -> Result<(), ClientError> {
        self.patch(
            "patch_admin_users_by_id_tokens_by_token",
            &[("id", user_id), ("token_id", token_id)],
            Some(&serde_json::json!({ "label": label })),
        )
        .await
        .map(drop)
    }

    pub async fn revoke_token(&self, user_id: &str, token_id: &str) -> Result<(), ClientError> {
        self.delete(
            "delete_admin_users_by_id_tokens_by_token",
            &[("id", user_id), ("token_id", token_id)],
        )
        .await
        .map(drop)
    }

    pub async fn purge_token(&self, user_id: &str, token_id: &str) -> Result<(), ClientError> {
        self.delete(
            "delete_admin_users_by_id_tokens_by_token_purge",
            &[("id", user_id), ("token_id", token_id)],
        )
        .await
        .map(drop)
    }

    pub async fn revoke_machine(&self, id: &str) -> Result<(), ClientError> {
        self.delete("delete_admin_machines_by_id", &[("id", id)]).await.map(drop)
    }

    pub async fn purge_machine(&self, id: &str) -> Result<(), ClientError> {
        self.delete("delete_admin_machines_by_id_purge", &[("id", id)]).await.map(drop)
    }

    /// Rotate a machine's key. The old one stops working the moment this
    /// returns, so the reply has to reach the daemon or it is locked out.
    pub async fn rotate_machine(&self, id: &str) -> Result<Rotated, ClientError> {
        self.post_as("post_admin_machines_by_id_rotate", &[("id", id)], &serde_json::json!({}))
            .await
    }

    /// The grant is clamped to the owner's ceiling server-side.
    pub async fn mint_key(
        &self,
        user_id: &str,
        request: &MintKey,
    ) -> Result<MintedKey, ClientError> {
        self.post_as("post_users_by_id_keys", &[("id", user_id)], request).await
    }

    /// Replace a key's granted scopes in place. The secret is untouched, so the
    /// key keeps working; the grant is still clamped to the owner's ceiling.
    pub async fn set_key_scopes(
        &self,
        user_id: &str,
        key_id: &str,
        scopes: &[String],
    ) -> Result<(), ClientError> {
        self.patch(
            "patch_users_by_id_keys_by_kid_acls",
            &[("id", user_id), ("kid", key_id)],
            Some(&serde_json::json!({ "scopes": scopes })),
        )
        .await
        .map(drop)
    }

    pub async fn revoke_key(&self, user_id: &str, key_id: &str) -> Result<(), ClientError> {
        self.delete("delete_users_by_id_keys_by_kid", &[("id", user_id), ("kid", key_id)])
            .await
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiKey, User, UserMachine, UserToken};

    fn user_json(disabled: &str, revoked: &str) -> String {
        format!(
            r#"{{"id":"u-1","name":"dorsk","created_at":"2026-01-01T00:00:00Z",
               "revoked_at":{revoked},"disabled_at":{disabled}}}"#
        )
    }

    const STAMP: &str = r#""2026-02-01T00:00:00Z""#;

    #[test]
    fn a_user_row_defaults_dispatch_to_allowed_as_the_server_does() {
        let user: User = serde_json::from_str(&user_json("null", "null")).expect("a user");
        assert!(user.can_dispatch);
        assert_eq!(user.state(), "active");
        assert!(!user.revoked());
    }

    #[test]
    fn a_revoked_user_outranks_a_disabled_one() {
        let disabled: User = serde_json::from_str(&user_json(STAMP, "null")).expect("a user");
        assert_eq!(disabled.state(), "disabled");
        assert!(!disabled.revoked());
        let revoked: User = serde_json::from_str(&user_json(STAMP, STAMP)).expect("a user");
        assert_eq!(revoked.state(), "revoked");
        assert!(revoked.revoked());
    }

    #[test]
    fn rows_minted_before_the_preview_column_decode_without_one() {
        let token: UserToken = serde_json::from_str(
            r#"{"id":"t-1","label":null,"created_at":"2026-01-01T00:00:00Z",
                "expires_at":null,"revoked_at":null}"#,
        )
        .expect("a token");
        assert_eq!(token.token_preview, None);

        let key: ApiKey = serde_json::from_str(
            r#"{"id":"k-1","label":"ci","kind":"user","created_at":"2026-01-01T00:00:00Z",
                "expires_at":null,"revoked_at":null}"#,
        )
        .expect("a key");
        assert_eq!(key.key_preview, None);
        assert!(key.scopes.is_empty());
    }

    #[test]
    fn a_machine_shows_its_display_name_when_it_has_a_real_one() {
        let json = |display: &str| {
            format!(
                r#"{{"id":"m-1","user_id":"u-1","name":"thinkpad","display_name":{display},
                   "first_seen_at":"2026-01-01T00:00:00Z","last_seen_at":"2026-01-02T00:00:00Z",
                   "revoked_at":null,"kind":"persistent"}}"#
            )
        };
        let plain: UserMachine = serde_json::from_str(&json("null")).expect("a machine");
        assert_eq!(plain.label(), "thinkpad");
        let blank: UserMachine = serde_json::from_str(&json(r#""""#)).expect("a machine");
        assert_eq!(blank.label(), "thinkpad", "a blank override is not a name");
        let named: UserMachine = serde_json::from_str(&json(r#""laptop""#)).expect("a machine");
        assert_eq!(named.label(), "laptop");
    }
}
