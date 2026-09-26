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
    match load_all(pool).await {
        Ok(plugins) => {
            tracing::info!(installed = plugins.len(), "installed plugins loaded");
            registry.set_installed(plugins);
        }
        Err(e) => tracing::error!("installed plugins not loaded: {e}"),
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
    registry.remove_installed(id);
    Ok(true)
}

/// Download an archive from an https URL through the SSRF-guarded client.
pub async fn fetch_archive(url: &str) -> Result<Vec<u8>, InstallError> {
    if !url.starts_with("https://") {
        return Err(InstallError::Url(crate::outbound::OutboundUrlError::NotHttps));
    }
    let allow = crate::outbound::upstream_allowlist();
    crate::outbound::validate_outbound_url(url, &allow).await.map_err(InstallError::Url)?;
    let resp = crate::outbound::upstream_client()
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| InstallError::Fetch(e.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::{fetch_archive, install, load_all, set_enabled, uninstall};
    use crate::plugin_archive::test_support::demo_tgz;
    use crate::plugins::PluginRegistry;

    #[tokio::test]
    async fn install_upgrade_toggle_and_uninstall_round_trip() {
        let Some(url) = crate::routes::gateway::test_db_url("plugin_store_round_trip") else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let registry = PluginRegistry::disabled();
        sqlx::query("DELETE FROM plugins WHERE id = 'demo'").execute(&pool).await.unwrap();

        let v1 = demo_tgz(None, "1.0.0");
        let plugin = install(&pool, &registry, &v1, None).await.unwrap();
        assert_eq!(plugin.manifest.version, "1.0.0");
        assert!(!plugin.instance_enabled);
        assert!(registry.get("demo").is_none(), "disabled plugins are hidden from users");
        assert_eq!(registry.all_admin().len(), 1);

        assert!(set_enabled(&pool, &registry, "demo", true).await.unwrap());
        assert!(registry.get("demo").is_some());
        assert!(!set_enabled(&pool, &registry, "nope", true).await.unwrap());

        let upgraded =
            install(&pool, &registry, &demo_tgz(Some("demo"), "1.1.0"), None).await.unwrap();
        assert_eq!(upgraded.manifest.version, "1.1.0");
        assert!(upgraded.instance_enabled, "upgrade keeps the instance toggle");
        assert_eq!(registry.get("demo").unwrap().manifest.version, "1.1.0");

        let fresh = PluginRegistry::disabled();
        fresh.set_installed(load_all(&pool).await.unwrap());
        let loaded = fresh.get("demo").unwrap();
        assert_eq!(loaded.manifest.version, "1.1.0");
        assert_eq!(
            crate::plugins::resolve_static(&loaded, "skills/demo/SKILL.md").unwrap(),
            b"# demo"
        );

        let (hash,): (String,) =
            sqlx::query_as("SELECT archive_hash FROM plugins WHERE id = 'demo'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(uninstall(&pool, &registry, "demo").await.unwrap());
        assert!(!uninstall(&pool, &registry, "demo").await.unwrap());
        assert!(registry.all_admin().is_empty());
        let (blobs,): (i64,) = sqlx::query_as("SELECT count(*) FROM daemon_blobs WHERE hash = $1")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(blobs, 0);
    }

    #[tokio::test]
    async fn fetch_refuses_plain_http_and_internal_hosts() {
        let err = fetch_archive("http://example.com/p.tgz").await.unwrap_err();
        assert!(err.to_string().contains("https"), "{err}");
        let err = fetch_archive("https://127.0.0.1/p.tgz").await.unwrap_err();
        assert!(err.to_string().contains("private or loopback"), "{err}");
    }
}
