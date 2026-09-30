//! Admin-installed plugins: the `plugins` table (manifest + instance toggle)
//! with the archive bytes in the blob store, and the https fetch of an archive.

use std::collections::BTreeMap;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::plugin_archive::{ArchiveError, MAX_ARCHIVE_BYTES, load_archive};
use crate::plugins::{Plugin, PluginRegistry};
use crate::routes::blobs::store_blob;

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const ARCHIVE_MEDIA_TYPE: &str = "application/gzip";
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Archive(#[from] ArchiveError),
    #[error("url {0}")]
    Url(crate::outbound::OutboundUrlError),
    #[error("download failed: {0}")]
    Fetch(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Every stored plugin, extracted; rows whose archive no longer validates are
/// logged and skipped.
pub async fn load_all(pool: &PgPool) -> sqlx::Result<BTreeMap<String, Plugin>> {
    let rows: Vec<(String, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT p.id, p.enabled, b.bytes FROM plugins p \
         JOIN daemon_blobs b ON b.hash = p.archive_hash ORDER BY p.id",
    )
    .fetch_all(pool)
    .await?;
    let mut out = BTreeMap::new();
    for (id, enabled, bytes) in rows {
        match load_archive(&bytes, enabled) {
            Ok(plugin) if plugin.manifest.id == id => {
                out.insert(id, plugin);
            }
            Ok(plugin) => {
                tracing::warn!(id, found = %plugin.manifest.id, "installed plugin id mismatch, skipped");
            }
            Err(e) => tracing::warn!(id, "installed plugin skipped: {e}"),
        }
    }
    Ok(out)
}

/// Load the table into `registry` at startup.
pub async fn init(pool: &PgPool, registry: &PluginRegistry) {
    if let Err(e) = sync(pool, registry).await {
        tracing::error!("installed plugins not loaded: {e}");
    }
}

/// A digest of every row that shapes the registry, so a pod can tell that
/// another replica installed, toggled or removed a plugin.
async fn fingerprint(pool: &PgPool) -> sqlx::Result<String> {
    sqlx::query_scalar(
        "SELECT coalesce(string_agg(id || ':' || archive_hash || ':' || enabled::text, ',' \
         ORDER BY id), '') FROM plugins",
    )
    .fetch_one(pool)
    .await
}

/// Reload `registry` when the table no longer matches what it was loaded from.
/// The fingerprint is read before the rows, so a concurrent change can only
/// cause one extra reload, never a stale registry.
pub async fn sync(pool: &PgPool, registry: &PluginRegistry) -> sqlx::Result<()> {
    let current = fingerprint(pool).await?;
    if registry.loaded_from().as_deref() == Some(current.as_str()) {
        return Ok(());
    }
    let plugins = load_all(pool).await?;
    tracing::info!(installed = plugins.len(), "installed plugins loaded");
    registry.set_installed_from(plugins, current);
    Ok(())
}

/// [`sync`] for request paths: a failure keeps serving the current registry.
pub async fn sync_or_warn(pool: &PgPool, registry: &PluginRegistry) {
    if let Err(e) = sync(pool, registry).await {
        tracing::warn!("plugin registry sync failed: {e}");
    }
}

/// Validate `bytes`, store them, upsert the row and register the plugin. An
/// existing id is upgraded in place and keeps its instance toggle.
pub async fn install(
    pool: &PgPool,
    registry: &PluginRegistry,
    bytes: &[u8],
    installed_by: Option<Uuid>,
) -> Result<Plugin, InstallError> {
    let mut plugin = load_archive(bytes, false)?;
    let blob = store_blob(pool, bytes, Some(ARCHIVE_MEDIA_TYPE)).await?;
    let manifest = serde_json::to_value(&plugin.manifest).unwrap_or_default();
    let (enabled,): (bool,) = sqlx::query_as(
        "INSERT INTO plugins (id, version, manifest, archive_hash, enabled, installed_by) \
         VALUES ($1, $2, $3, $4, false, $5) \
         ON CONFLICT (id) DO UPDATE SET version = EXCLUDED.version, \
           manifest = EXCLUDED.manifest, archive_hash = EXCLUDED.archive_hash, \
           installed_by = EXCLUDED.installed_by, updated_at = now() \
         RETURNING enabled",
    )
    .bind(&plugin.manifest.id)
    .bind(&plugin.manifest.version)
    .bind(&manifest)
    .bind(&blob.hash)
    .bind(installed_by)
    .fetch_one(pool)
    .await?;
    plugin.instance_enabled = enabled;
    registry.upsert_installed(plugin.clone());
    Ok(plugin)
}

/// Flip the instance toggle; `Ok(false)` when `id` is not an installed plugin.
pub async fn set_enabled(
    pool: &PgPool,
    registry: &PluginRegistry,
    id: &str,
    enabled: bool,
) -> sqlx::Result<bool> {
    let n = sqlx::query("UPDATE plugins SET enabled = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(enabled)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n == 1 && registry.set_installed_enabled(id, enabled))
}

/// Remove the row, the archive blob when nothing else references it, and the
/// registry entry; `Ok(false)` when `id` is not an installed plugin.
pub async fn uninstall(pool: &PgPool, registry: &PluginRegistry, id: &str) -> sqlx::Result<bool> {
    let hash: Option<String> =
        sqlx::query_scalar("DELETE FROM plugins WHERE id = $1 RETURNING archive_hash")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some(hash) = hash else { return Ok(false) };
    sqlx::query(
        "DELETE FROM daemon_blobs WHERE hash = $1 \
         AND NOT EXISTS (SELECT 1 FROM plugins WHERE archive_hash = $1) \
         AND NOT EXISTS (SELECT 1 FROM session_attachments WHERE hash = $1)",
    )
    .bind(&hash)
    .execute(pool)
    .await?;
    crate::plugin_settings::delete(pool, id).await?;
    registry.remove_installed(id);
    Ok(true)
}

/// Download an archive from an https URL through the SSRF-guarded client,
/// following redirects (release hosts like GitHub serve assets via a 302) and
/// re-validating every hop.
pub async fn fetch_archive(url: &str) -> Result<Vec<u8>, InstallError> {
    let allow = crate::outbound::upstream_allowlist();
    let mut url = url.to_owned();
    let mut hops = 0;
    let resp = loop {
        if !url.starts_with("https://") {
            return Err(InstallError::Url(crate::outbound::OutboundUrlError::NotHttps));
        }
        crate::outbound::validate_outbound_url(&url, &allow).await.map_err(InstallError::Url)?;
        let resp = crate::outbound::upstream_client()
            .get(&url)
            .timeout(FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|e| InstallError::Fetch(e.to_string()))?;
        if !resp.status().is_redirection() {
            break resp;
        }
        hops += 1;
        if hops > MAX_REDIRECTS {
            return Err(InstallError::Fetch("too many redirects".into()));
        }
        url = redirect_target(&url, resp.headers().get(reqwest::header::LOCATION))?;
    };
    if !resp.status().is_success() {
        return Err(InstallError::Fetch(format!("HTTP {}", resp.status())));
    }
    if resp.content_length().is_some_and(|n| n > MAX_ARCHIVE_BYTES as u64) {
        return Err(ArchiveError::TooLarge.into());
    }
    let mut body = Vec::new();
    let mut stream = resp;
    while let Some(chunk) = stream.chunk().await.map_err(|e| InstallError::Fetch(e.to_string()))? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_ARCHIVE_BYTES {
            return Err(ArchiveError::TooLarge.into());
        }
    }
    Ok(body)
}

fn redirect_target(
    from: &str,
    location: Option<&reqwest::header::HeaderValue>,
) -> Result<String, InstallError> {
    let location = location
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| InstallError::Fetch("redirect without a Location".into()))?;
    let base = reqwest::Url::parse(from)
        .map_err(|_| InstallError::Url(crate::outbound::OutboundUrlError::Malformed))?;
    base.join(location)
        .map(String::from)
        .map_err(|_| InstallError::Url(crate::outbound::OutboundUrlError::Malformed))
}

#[cfg(test)]
mod tests {
    use super::{fetch_archive, install, load_all, set_enabled, sync, uninstall};
    use crate::plugin_archive::test_support::plugin_tgz;
    use crate::plugins::PluginRegistry;

    /// Whether `registry` holds the plugin `id`, regardless of what other tests
    /// have left in the shared `plugins` table.
    fn knows(registry: &PluginRegistry, id: &str) -> bool {
        registry.all_admin().iter().any(|p| p.manifest.id == id)
    }

    #[tokio::test]
    async fn install_upgrade_toggle_and_uninstall_round_trip() {
        const ID: &str = "plugin-store-round-trip";
        let Some(url) = crate::routes::gateway::test_db_url("plugin_store_round_trip") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let registry = PluginRegistry::disabled();
        sqlx::query("DELETE FROM plugins WHERE id = $1").bind(ID).execute(&pool).await.unwrap();

        let v1 = plugin_tgz(ID, None, "1.0.0");
        let plugin = install(&pool, &registry, &v1, None).await.unwrap();
        assert_eq!(plugin.manifest.version, "1.0.0");
        assert!(!plugin.instance_enabled);
        assert!(registry.get(ID).is_none(), "disabled plugins are hidden from users");
        assert_eq!(registry.all_admin().iter().filter(|p| p.manifest.id == ID).count(), 1);

        assert!(set_enabled(&pool, &registry, ID, true).await.unwrap());
        assert!(registry.get(ID).is_some());
        assert!(!set_enabled(&pool, &registry, "nope", true).await.unwrap());

        let upgraded =
            install(&pool, &registry, &plugin_tgz(ID, Some(ID), "1.1.0"), None).await.unwrap();
        assert_eq!(upgraded.manifest.version, "1.1.0");
        assert!(upgraded.instance_enabled, "upgrade keeps the instance toggle");
        assert_eq!(registry.get(ID).unwrap().manifest.version, "1.1.0");

        let fresh = PluginRegistry::disabled();
        fresh.set_installed(load_all(&pool).await.unwrap());
        let loaded = fresh.get(ID).unwrap();
        assert_eq!(loaded.manifest.version, "1.1.0");
        assert_eq!(
            crate::plugins::resolve_static(&loaded, &format!("skills/{ID}/SKILL.md")).unwrap(),
            b"# demo"
        );

        let (hash,): (String,) =
            sqlx::query_as("SELECT archive_hash FROM plugins WHERE id = $1")
                .bind(ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(uninstall(&pool, &registry, ID).await.unwrap());
        assert!(!uninstall(&pool, &registry, ID).await.unwrap());
        assert!(!knows(&registry, ID));
        let (blobs,): (i64,) = sqlx::query_as("SELECT count(*) FROM daemon_blobs WHERE hash = $1")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(blobs, 0);
    }

    #[tokio::test]
    async fn a_replica_picks_up_changes_made_through_another() {
        const ID: &str = "plugin-store-replicas";
        let Some(url) = crate::routes::gateway::test_db_url("plugin_store_replicas") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        sqlx::query("DELETE FROM plugins WHERE id = $1").bind(ID).execute(&pool).await.unwrap();
        let (a, b) = (PluginRegistry::disabled(), PluginRegistry::disabled());
        sync(&pool, &b).await.unwrap();

        install(&pool, &a, &plugin_tgz(ID, None, "1.0.0"), None).await.unwrap();
        assert!(!knows(&b, ID), "b has not synced since the install");
        sync(&pool, &b).await.unwrap();
        assert!(knows(&b, ID));
        assert!(b.get(ID).is_none(), "still disabled instance-wide");

        set_enabled(&pool, &a, ID, true).await.unwrap();
        sync(&pool, &b).await.unwrap();
        assert!(b.get(ID).is_some());

        install(&pool, &a, &plugin_tgz(ID, Some(ID), "1.1.0"), None).await.unwrap();
        sync(&pool, &b).await.unwrap();
        assert_eq!(b.get(ID).unwrap().manifest.version, "1.1.0");

        uninstall(&pool, &a, ID).await.unwrap();
        sync(&pool, &b).await.unwrap();
        assert!(!knows(&b, ID));
    }

    #[tokio::test]
    async fn fetch_refuses_plain_http_and_internal_hosts() {
        let err = fetch_archive("http://example.com/p.tgz").await.unwrap_err();
        assert!(err.to_string().contains("https"), "{err}");
        let err = fetch_archive("https://127.0.0.1/p.tgz").await.unwrap_err();
        assert!(err.to_string().contains("private or loopback"), "{err}");
    }

    #[test]
    fn redirect_targets_resolve_against_the_current_hop() {
        use reqwest::header::HeaderValue;
        let from = "https://github.com/o/r/releases/download/v1/p.tgz";
        let abs = HeaderValue::from_static("https://release-assets.githubusercontent.com/x?sig=1");
        assert_eq!(
            super::redirect_target(from, Some(&abs)).unwrap(),
            "https://release-assets.githubusercontent.com/x?sig=1"
        );
        let rel = HeaderValue::from_static("/o/r/other.tgz");
        assert_eq!(
            super::redirect_target(from, Some(&rel)).unwrap(),
            "https://github.com/o/r/other.tgz"
        );
        assert!(super::redirect_target(from, None).is_err());
    }

    #[tokio::test]
    #[ignore = "needs network"]
    async fn fetch_follows_github_release_redirects() {
        let bytes = fetch_archive(
            "https://github.com/DorskFR/yubisashi/releases/download/v0.5.0/yubisashi-0.5.0.tgz",
        )
        .await
        .unwrap();
        crate::plugin_archive::load_archive(&bytes, true).unwrap();
    }
}
