//! Instance-level plugin settings: the admin-owned values a plugin declares as
//! `instanceSettings`, plus the per-plugin secret the backend proxy signs with.
//!
//! Values whose declaration says `secret` are sealed with the server's vault
//! key at rest and never leave the process: the admin API answers "set" or
//! "unset" for them, and `GET /api/v1/plugins` omits them entirely.

use std::collections::BTreeMap;

use sqlx::PgPool;

use crate::plugins::{MAX_INSTANCE_SETTING_VALUE_CHARS, PluginManifest};

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("`{0}` is not a setting this plugin declares")]
    Undeclared(String),
    #[error("`{0}` is longer than {MAX_INSTANCE_SETTING_VALUE_CHARS} characters")]
    TooLong(String),
    #[error("`{0}` must be an http or https URL with a host")]
    NotAUrl(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// What is stored for `plugin_id`, exactly as written (secrets still sealed).
async fn stored(pool: &PgPool, plugin_id: &str) -> sqlx::Result<BTreeMap<String, String>> {
    let row: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT setting_values FROM plugin_settings WHERE plugin_id = $1")
            .bind(plugin_id)
            .fetch_optional(pool)
            .await?;
    Ok(row
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default())
}

fn unseal(value: &str) -> Option<String> {
    crate::crypto::decrypt(value, &crate::crypto::vault_key())
}

/// Every declared value, secrets decrypted. Server-side use only.
pub async fn resolved(
    pool: &PgPool,
    manifest: &PluginManifest,
) -> sqlx::Result<BTreeMap<String, String>> {
    let stored = stored(pool, &manifest.id).await?;
    Ok(manifest
        .instance_settings
        .iter()
        .filter_map(|decl| {
            let raw = stored.get(&decl.key)?;
            let value = if decl.secret { unseal(raw)? } else { raw.clone() };
            (!value.is_empty()).then(|| (decl.key.clone(), value))
        })
        .collect())
}

/// The values a plugin's users may see: declared, non-secret and non-empty.
pub async fn public_values(
    pool: &PgPool,
    manifest: &PluginManifest,
) -> sqlx::Result<BTreeMap<String, String>> {
    let stored = stored(pool, &manifest.id).await?;
    Ok(manifest
        .instance_settings
        .iter()
        .filter(|decl| !decl.secret)
        .filter_map(|decl| {
            let value = stored.get(&decl.key)?;
            (!value.is_empty()).then(|| (decl.key.clone(), value.clone()))
        })
        .collect())
}

/// The admin view: non-secret values verbatim, and for every secret only
/// whether it currently holds a value.
pub struct AdminView {
    pub values: BTreeMap<String, String>,
    pub secrets_set: BTreeMap<String, bool>,
}

pub async fn admin_view(pool: &PgPool, manifest: &PluginManifest) -> sqlx::Result<AdminView> {
    let stored = stored(pool, &manifest.id).await?;
    let mut values = BTreeMap::new();
    let mut secrets_set = BTreeMap::new();
    for decl in &manifest.instance_settings {
        let value = stored.get(&decl.key).filter(|v| !v.is_empty());
        if decl.secret {
            secrets_set.insert(decl.key.clone(), value.is_some());
        } else if let Some(value) = value {
            values.insert(decl.key.clone(), value.clone());
        }
    }
    Ok(AdminView { values, secrets_set })
}

fn valid_upstream(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty())
    })
}

/// Apply a patch: every key present is written, an empty value clears the
/// setting, and a key the manifest does not declare is refused. Keys absent
/// from the patch keep their current value.
pub async fn write(
    pool: &PgPool,
    manifest: &PluginManifest,
    patch: &BTreeMap<String, String>,
) -> Result<(), SettingsError> {
    let mut next = stored(pool, &manifest.id).await?;
    for (key, value) in patch {
        let decl = manifest
            .instance_settings
            .iter()
            .find(|d| &d.key == key)
            .ok_or_else(|| SettingsError::Undeclared(key.clone()))?;
        let value = value.trim();
        if value.is_empty() {
            next.remove(key);
            continue;
        }
        if value.chars().count() > MAX_INSTANCE_SETTING_VALUE_CHARS {
            return Err(SettingsError::TooLong(key.clone()));
        }
        if decl.kind == "url" && !valid_upstream(value) {
            return Err(SettingsError::NotAUrl(key.clone()));
        }
        let stored_value = if decl.secret {
            crate::crypto::encrypt(value, &crate::crypto::vault_key())
        } else {
            value.to_owned()
        };
        next.insert(key.clone(), stored_value);
    }
    next.retain(|key, _| manifest.instance_settings.iter().any(|d| &d.key == key));
    let json = serde_json::to_value(&next).unwrap_or_else(|_| serde_json::json!({}));
    sqlx::query(
        "INSERT INTO plugin_settings (plugin_id, setting_values) VALUES ($1, $2) \
         ON CONFLICT (plugin_id) DO UPDATE SET setting_values = EXCLUDED.setting_values, updated_at = now()",
    )
    .bind(&manifest.id)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

/// The plugin's proxy secret in plaintext, `None` when it has none yet.
pub async fn proxy_secret(pool: &PgPool, plugin_id: &str) -> sqlx::Result<Option<String>> {
    let sealed: Option<Option<String>> =
        sqlx::query_scalar("SELECT proxy_secret FROM plugin_settings WHERE plugin_id = $1")
            .bind(plugin_id)
            .fetch_optional(pool)
            .await?;
    Ok(sealed.flatten().as_deref().and_then(unseal))
}

async fn store_proxy_secret(pool: &PgPool, plugin_id: &str, secret: &str) -> sqlx::Result<()> {
    let sealed = crate::crypto::encrypt(secret, &crate::crypto::vault_key());
    sqlx::query(
        "INSERT INTO plugin_settings (plugin_id, proxy_secret) VALUES ($1, $2) \
         ON CONFLICT (plugin_id) DO UPDATE SET proxy_secret = EXCLUDED.proxy_secret, \
           updated_at = now()",
    )
    .bind(plugin_id)
    .bind(sealed)
    .execute(pool)
    .await?;
    Ok(())
}

/// Replace the proxy secret and return the new plaintext; the only moment it is
/// ever readable.
pub async fn rotate_proxy_secret(pool: &PgPool, plugin_id: &str) -> sqlx::Result<String> {
    let secret = crate::auth::mint_secret();
    store_proxy_secret(pool, plugin_id, &secret).await?;
    Ok(secret)
}

/// The proxy secret, minting one when the plugin has none. Returns the
/// plaintext together with whether it was freshly generated, so an install can
/// show a new secret once and a re-install never re-shows the old one.
pub async fn ensure_proxy_secret(
    pool: &PgPool,
    plugin_id: &str,
) -> sqlx::Result<(String, bool)> {
    if let Some(existing) = proxy_secret(pool, plugin_id).await? {
        return Ok((existing, false));
    }
    Ok((rotate_proxy_secret(pool, plugin_id).await?, true))
}

pub async fn delete(pool: &PgPool, plugin_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM plugin_settings WHERE plugin_id = $1")
        .bind(plugin_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        admin_view, delete, ensure_proxy_secret, proxy_secret, public_values, resolved,
        rotate_proxy_secret, valid_upstream, write,
    };
    use crate::plugins::{PluginInstanceSetting, PluginManifest};
    use std::collections::BTreeMap;

    fn manifest() -> PluginManifest {
        let decl = |key: &str, kind: &str, secret: bool| PluginInstanceSetting {
            key: key.to_owned(),
            label: key.to_owned(),
            kind: kind.to_owned(),
            secret,
        };
        PluginManifest {
            id: "setting-test".to_owned(),
            name: "N".to_owned(),
            description: String::new(),
            version: "1".to_owned(),
            cctui_api: 1,
            icon: None,
            web: None,
            skills: vec![],
            page: None,
            styles: vec![],
            settings: vec![],
            instance_settings: vec![
                decl("upstream", "url", false),
                decl("token", "string", true),
            ],
            backend: None,
        }
    }

    fn patch(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn upstream_urls_must_be_absolute_http() {
        assert!(valid_upstream("https://ghreview.dorsk.dev"));
        assert!(valid_upstream("http://ghreview.cctui.svc.cluster.local:8790"));
        assert!(!valid_upstream("ghreview.dorsk.dev"));
        assert!(!valid_upstream("file:///etc/passwd"));
        assert!(!valid_upstream(""));
    }

    #[tokio::test]
    async fn secrets_are_sealed_and_never_read_back_while_plain_values_are() {
        let Some(url) = crate::routes::gateway::test_db_url("plugin_instance_settings") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let m = manifest();
        delete(&pool, &m.id).await.unwrap();

        write(&pool, &m, &patch(&[("upstream", "https://up.example"), ("token", "s3cr3t")]))
            .await
            .unwrap();

        let view = admin_view(&pool, &m).await.unwrap();
        assert_eq!(view.values.get("upstream").map(String::as_str), Some("https://up.example"));
        assert!(!view.values.contains_key("token"), "a secret is never returned");
        assert_eq!(view.secrets_set.get("token"), Some(&true));

        let public = public_values(&pool, &m).await.unwrap();
        assert_eq!(public.keys().collect::<Vec<_>>(), vec!["upstream"]);

        let all = resolved(&pool, &m).await.unwrap();
        assert_eq!(all.get("token").map(String::as_str), Some("s3cr3t"));

        let raw: serde_json::Value =
            sqlx::query_scalar("SELECT setting_values FROM plugin_settings WHERE plugin_id = $1")
                .bind(&m.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(raw["token"].as_str(), Some("s3cr3t"), "the secret is sealed at rest");

        write(&pool, &m, &patch(&[("upstream", "https://other.example")])).await.unwrap();
        let all = resolved(&pool, &m).await.unwrap();
        assert_eq!(all.get("upstream").map(String::as_str), Some("https://other.example"));
        assert_eq!(all.get("token").map(String::as_str), Some("s3cr3t"), "a patch merges");

        write(&pool, &m, &patch(&[("token", "")])).await.unwrap();
        assert_eq!(admin_view(&pool, &m).await.unwrap().secrets_set.get("token"), Some(&false));

        assert!(write(&pool, &m, &patch(&[("nope", "x")])).await.is_err());
        assert!(write(&pool, &m, &patch(&[("upstream", "not-a-url")])).await.is_err());
        assert!(write(&pool, &m, &patch(&[("token", &"x".repeat(2049))])).await.is_err());

        delete(&pool, &m.id).await.unwrap();
        assert!(resolved(&pool, &m).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_proxy_secret_is_minted_once_and_rotates_to_a_new_value() {
        let Some(url) = crate::routes::gateway::test_db_url("plugin_proxy_secret") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        delete(&pool, "secret-test").await.unwrap();
        assert!(proxy_secret(&pool, "secret-test").await.unwrap().is_none());

        let (first, fresh) = ensure_proxy_secret(&pool, "secret-test").await.unwrap();
        assert!(fresh);
        assert_eq!(first.len(), 64);
        let (again, fresh) = ensure_proxy_secret(&pool, "secret-test").await.unwrap();
        assert_eq!(again, first);
        assert!(!fresh);

        let rotated = rotate_proxy_secret(&pool, "secret-test").await.unwrap();
        assert_ne!(rotated, first);
        assert_eq!(proxy_secret(&pool, "secret-test").await.unwrap().as_deref(), Some(&*rotated));
        delete(&pool, "secret-test").await.unwrap();
    }

    #[tokio::test]
    async fn settings_and_the_secret_share_one_row_without_clobbering_each_other() {
        let Some(url) = crate::routes::gateway::test_db_url("plugin_settings_coexist") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let m = manifest();
        delete(&pool, &m.id).await.unwrap();
        let (secret, _) = ensure_proxy_secret(&pool, &m.id).await.unwrap();
        write(&pool, &m, &patch(&[("upstream", "https://up.example")])).await.unwrap();
        assert_eq!(proxy_secret(&pool, &m.id).await.unwrap().as_deref(), Some(&*secret));
        assert_eq!(
            resolved(&pool, &m).await.unwrap().get("upstream").map(String::as_str),
            Some("https://up.example")
        );
        delete(&pool, &m.id).await.unwrap();
    }
}
