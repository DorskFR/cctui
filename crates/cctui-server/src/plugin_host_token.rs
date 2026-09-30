//! Per-user cctui credentials the host mints on a plugin's behalf: enabling a
//! plugin that declares `hostToken` mints a `read`-scoped user token labelled
//! `plugin:<id>`, and the token dies with the enablement.
//!
//! The plaintext is needed again on every session spawn, so it is sealed with
//! the vault key into the owning user's `user_settings.data` under
//! `plugins.hostTokens`. That sub-object is server-owned: a settings write
//! cannot set it and a settings read never returns it.

use std::collections::BTreeSet;

use serde_json::{Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{
    AuthConfig, NewKey, Scope, mint_secret, register_key, sha256_hex, token_preview, user_token,
};
use crate::plugins::PluginRegistry;

/// The only scope a host token ever carries. A plugin's skill reaches cctui
/// through one route — the backend proxy at `/api/v1/plugins/<id>/backend/*` —
/// and that route asks for `read`, the narrowest scope the system has.
const GRANT: Scope = Scope::Read;

const KIND: &str = "plugin";

/// The server-owned sub-object of the user's `plugins` settings block.
const BLOCK: &str = "hostTokens";

#[must_use]
pub fn label(plugin_id: &str) -> String {
    format!("plugin:{plugin_id}")
}

fn stored(data: Option<&Value>) -> Option<&Map<String, Value>> {
    data?.get("plugins")?.get(BLOCK)?.as_object()
}

/// The sealed token stored for `plugin_id`, as written.
#[must_use]
pub fn sealed(data: Option<&Value>, plugin_id: &str) -> Option<String> {
    stored(data)?.get(plugin_id)?.as_str().map(str::to_owned)
}

/// The usable token for `plugin_id`; `None` when absent or unsealable (a vault
/// key rotation leaves a value nothing can read — a re-enable re-mints it).
#[must_use]
pub fn unsealed(data: Option<&Value>, plugin_id: &str) -> Option<String> {
    crate::crypto::decrypt(&sealed(data, plugin_id)?, &crate::crypto::vault_key())
}

/// Drop the whole block, for anything that hands `user_settings.data` back to
/// a client.
pub fn strip(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    let Some(mut plugins) = obj.get("plugins").and_then(Value::as_object).cloned() else { return };
    if plugins.remove(BLOCK).is_none() {
        return;
    }
    put_plugins(obj, plugins);
}

fn put_plugins(obj: &mut Map<String, Value>, plugins: Map<String, Value>) {
    if plugins.is_empty() {
        obj.remove("plugins");
    } else {
        obj.insert("plugins".to_owned(), Value::Object(plugins));
    }
}

fn write(data: &mut Value, plugin_id: &str, value: Option<String>) {
    let Some(obj) = data.as_object_mut() else { return };
    let mut plugins = obj.get("plugins").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut block =
        plugins.get(BLOCK).and_then(Value::as_object).cloned().unwrap_or_default();
    match value {
        Some(v) => block.insert(plugin_id.to_owned(), Value::String(v)),
        None => block.remove(plugin_id),
    };
    if block.is_empty() {
        plugins.remove(BLOCK);
    } else {
        plugins.insert(BLOCK.to_owned(), Value::Object(block));
    }
    put_plugins(obj, plugins);
}

/// Mint a fresh host token for `user_id`/`plugin_id` and return the plaintext.
/// The grant is `read` intersected with the user's own ceiling, so a user who
/// may not read cannot gain it through a plugin.
async fn mint(pool: &PgPool, user_id: Uuid, plugin_id: &str) -> sqlx::Result<String> {
    let ceiling = crate::store::acls::user_ceiling(pool, user_id).await?;
    let grant: BTreeSet<Scope> = ceiling.into_iter().filter(|scope| *scope == GRANT).collect();
    let token = user_token(&mint_secret());
    let hash = sha256_hex(&token);
    let preview = token_preview(&token);
    let label = label(plugin_id);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, token_preview) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(&label)
    .bind(&preview)
    .execute(&mut *tx)
    .await?;
    register_key(
        &mut *tx,
        NewKey {
            user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: Some(&label),
            kind: KIND,
            machine_id: None,
            dispatcher_id: None,
            expires_at: None,
            passkey_id: None,
        },
        grant,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(%user_id, plugin_id, "plugin host token minted");
    Ok(token)
}

/// Destroy every host token `user_id` holds for `plugin_id`. Deleting the rows
/// rather than flagging them keeps the surface at exactly one live token per
/// (user, plugin); the auth cache is purged so the secret stops resolving now.
async fn revoke(
    pool: &PgPool,
    auth: &AuthConfig,
    user_id: Uuid,
    plugin_id: &str,
) -> sqlx::Result<usize> {
    let hashes: Vec<String> = sqlx::query_scalar(
        "DELETE FROM user_tokens WHERE user_id = $1 AND label = $2 RETURNING token_hash",
    )
    .bind(user_id)
    .bind(label(plugin_id))
    .fetch_all(pool)
    .await?;
    purge(pool, auth, &hashes).await?;
    if !hashes.is_empty() {
        tracing::info!(%user_id, plugin_id, count = hashes.len(), "plugin host token revoked");
    }
    Ok(hashes.len())
}

async fn purge(pool: &PgPool, auth: &AuthConfig, hashes: &[String]) -> sqlx::Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }
    sqlx::query("DELETE FROM auth_keys WHERE key_hash = ANY($1)")
        .bind(hashes)
        .execute(pool)
        .await?;
    for hash in hashes {
        auth.purge(hash);
    }
    Ok(())
}

/// Destroy every user's host token for `plugin_id` and forget the sealed copies.
/// Called when the instance toggle goes off and when the plugin is uninstalled:
/// a plugin nobody can use holds no credentials.
pub async fn revoke_for_all(
    pool: &PgPool,
    auth: &AuthConfig,
    plugin_id: &str,
) -> sqlx::Result<usize> {
    let hashes: Vec<String> =
        sqlx::query_scalar("DELETE FROM user_tokens WHERE label = $1 RETURNING token_hash")
            .bind(label(plugin_id))
            .fetch_all(pool)
            .await?;
    purge(pool, auth, &hashes).await?;
    sqlx::query(
        "UPDATE user_settings SET data = data #- $1::text[], updated_at = now() \
         WHERE (data #> $1::text[]) IS NOT NULL",
    )
    .bind(vec!["plugins".to_owned(), BLOCK.to_owned(), plugin_id.to_owned()])
    .execute(pool)
    .await?;
    if !hashes.is_empty() {
        tracing::info!(plugin_id, count = hashes.len(), "plugin host tokens revoked for every user");
    }
    Ok(hashes.len())
}

/// Which of the `enabled` ids are installed, instance-enabled and declare a
/// `hostToken`.
fn wanted(registry: &PluginRegistry, enabled: &[String]) -> Vec<String> {
    enabled
        .iter()
        .filter_map(|id| registry.get(id))
        .filter(|p| p.manifest.host_token.is_some())
        .map(|p| p.manifest.id)
        .collect()
}

/// What reconciling one settings write requires. `carry` is what makes a
/// re-enable idempotent: a token already sealed in `prev` is reused, so the
/// user never accumulates two live credentials for one plugin.
#[derive(Debug, Default, PartialEq, Eq)]
struct Plan {
    mint: Vec<String>,
    carry: Vec<(String, String)>,
    revoke: Vec<String>,
}

fn plan(wanted: &[String], prev: Option<&Value>) -> Plan {
    let mut out = Plan::default();
    for id in wanted {
        match sealed(prev, id) {
            Some(value) => out.carry.push((id.clone(), value)),
            None => out.mint.push(id.clone()),
        }
    }
    out.revoke = stored(prev)
        .map(|block| block.keys().filter(|id| !wanted.contains(*id)).cloned().collect())
        .unwrap_or_default();
    out
}

/// Bring `next` — the settings blob about to be persisted for `user_id` — in
/// line with what it enables, minting and revoking as the enablement changes.
pub async fn reconcile(
    pool: &PgPool,
    auth: &AuthConfig,
    registry: &PluginRegistry,
    user_id: Uuid,
    prev: Option<&Value>,
    next: &mut Value,
) -> sqlx::Result<()> {
    // Server-owned: whatever the client sent for this block is not evidence.
    strip(next);
    let plan = plan(&wanted(registry, &crate::plugins::enabled_ids(Some(next))), prev);
    for (id, value) in plan.carry {
        write(next, &id, Some(value));
    }
    for id in plan.mint {
        let token = mint(pool, user_id, &id).await?;
        write(next, &id, Some(crate::crypto::encrypt(&token, &crate::crypto::vault_key())));
    }
    for id in plan.revoke {
        revoke(pool, auth, user_id, &id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BLOCK, Plan, label, plan, sealed, strip, wanted, write};
    use crate::plugins::{PluginRegistry, test_support::write_plugin};
    use serde_json::{Value, json};

    fn with_token(id: &str) -> Value {
        json!({ "plugins": { BLOCK: { id: "v1:sealed" } } })
    }

    #[test]
    fn enabling_mints_once_and_a_re_enable_carries_the_same_token() {
        let fresh = plan(&["ghreview".to_owned()], None);
        assert_eq!(fresh, Plan { mint: vec!["ghreview".to_owned()], ..Plan::default() });

        let prev = with_token("ghreview");
        let again = plan(&["ghreview".to_owned()], Some(&prev));
        assert_eq!(
            again,
            Plan {
                carry: vec![("ghreview".to_owned(), "v1:sealed".to_owned())],
                ..Plan::default()
            },
            "a second enable must not mint a second credential"
        );
    }

    #[test]
    fn disabling_revokes_and_an_unrelated_write_keeps_the_token() {
        let prev = with_token("ghreview");
        assert_eq!(
            plan(&[], Some(&prev)),
            Plan { revoke: vec!["ghreview".to_owned()], ..Plan::default() }
        );

        let mut both = with_token("ghreview");
        both["plugins"][BLOCK]["other"] = json!("v1:other");
        let only_ghreview = plan(&["ghreview".to_owned()], Some(&both));
        assert_eq!(only_ghreview.carry, vec![("ghreview".to_owned(), "v1:sealed".to_owned())]);
        assert_eq!(
            only_ghreview.revoke,
            vec!["other".to_owned()],
            "a plugin that dropped out of the enabled set loses its token"
        );
    }

    #[test]
    fn a_plan_over_nothing_does_nothing() {
        assert_eq!(plan(&[], None), Plan::default());
        assert_eq!(plan(&[], Some(&json!({ "locale": "en" }))), Plan::default());
    }

    #[test]
    fn only_an_enabled_plugin_that_asked_for_a_token_wants_one() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "asks", r#","hostToken":{"env":"ASKS_TOKEN"}"#);
        write_plugin(root.path(), "quiet", "");
        let registry = PluginRegistry::from_dir(root.path().to_path_buf());

        let enabled =
            ["asks".to_owned(), "quiet".to_owned(), "not-installed".to_owned()];
        assert_eq!(wanted(&registry, &enabled), vec!["asks".to_owned()]);
        assert!(wanted(&registry, &["quiet".to_owned()]).is_empty());
        assert!(wanted(&registry, &[]).is_empty());
    }

    #[test]
    fn a_label_names_the_plugin() {
        assert_eq!(label("ghreview"), "plugin:ghreview");
    }

    #[test]
    fn the_block_is_written_read_and_stripped() {
        let mut data = json!({ "locale": "en" });
        write(&mut data, "ghreview", Some("v1:aa".to_owned()));
        assert_eq!(sealed(Some(&data), "ghreview").as_deref(), Some("v1:aa"));
        assert_eq!(data["plugins"][BLOCK]["ghreview"], "v1:aa");
        assert_eq!(data["locale"], "en");

        write(&mut data, "other", Some("v1:bb".to_owned()));
        assert_eq!(sealed(Some(&data), "other").as_deref(), Some("v1:bb"));

        write(&mut data, "ghreview", None);
        assert!(sealed(Some(&data), "ghreview").is_none());
        assert_eq!(sealed(Some(&data), "other").as_deref(), Some("v1:bb"));

        write(&mut data, "other", None);
        assert!(data.get("plugins").is_none(), "an empty block takes `plugins` with it");
        assert_eq!(data["locale"], "en");

        assert!(sealed(None, "ghreview").is_none());
        assert!(sealed(Some(&json!({ "plugins": {} })), "ghreview").is_none());
    }

    #[test]
    fn removing_an_absent_id_creates_nothing() {
        let mut data = json!({});
        write(&mut data, "ghreview", None);
        assert_eq!(data, json!({}), "a revoke must not materialise the block it clears");
    }

    #[test]
    fn strip_leaves_the_users_own_plugin_settings_alone() {
        let mut data = json!({
            "plugins": {
                "enabled": { "ghreview": true },
                "config": { "ghreview": { "host": "h" } },
                BLOCK: { "ghreview": "v1:aa" }
            }
        });
        strip(&mut data);
        assert!(data["plugins"].get(BLOCK).is_none());
        assert_eq!(data["plugins"]["enabled"]["ghreview"], true);
        assert_eq!(data["plugins"]["config"]["ghreview"]["host"], "h");

        let mut only = json!({ "plugins": { BLOCK: { "ghreview": "v1:aa" } } });
        strip(&mut only);
        assert_eq!(only, json!({}), "nothing else lived in `plugins`");

        let mut none: Value = json!({ "locale": "en" });
        strip(&mut none);
        assert_eq!(none, json!({ "locale": "en" }));
    }
}
