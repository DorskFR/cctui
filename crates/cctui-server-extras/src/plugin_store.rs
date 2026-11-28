//! Admin-installed plugins: the `plugins` table (manifest + instance toggle)
//! with the archive bytes in the blob store, and the https fetch of an archive.

use std::collections::BTreeMap;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::hooks::BlobsHooks;
use crate::plugin_archive::{ArchiveError, MAX_ARCHIVE_BYTES, load_archive};
use crate::plugins::{Plugin, PluginRegistry};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const ARCHIVE_MEDIA_TYPE: &str = "application/gzip";
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Archive(#[from] ArchiveError),
    #[error("url {0}")]
    Url(cctui_server_kernel::outbound::OutboundUrlError),
    #[error("download failed: {0}")]
    Fetch(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Every stored plugin, extracted; rows whose archive no longer validates are
/// logged and skipped.
pub async fn load_all(
    pool: &PgPool,
    blobs: &impl BlobsHooks,
) -> sqlx::Result<BTreeMap<String, Plugin>> {
    let rows: Vec<(String, bool, String)> = sqlx::query_as(
        "SELECT p.id, p.enabled, b.hash FROM plugins p \
         JOIN daemon_blobs b ON b.hash = p.archive_hash ORDER BY p.id",
    )
    .fetch_all(pool)
    .await?;
    let mut out = BTreeMap::new();
    let mut conn = pool.acquire().await?;
    for (id, enabled, hash) in rows {
        let Some(bytes) = blobs.read_blob_in(&mut conn, &hash).await? else {
            continue;
        };
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
pub async fn init(pool: &PgPool, registry: &PluginRegistry, blobs: &impl BlobsHooks) {
    if let Err(e) = sync(pool, registry, blobs).await {
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
pub async fn sync(
    pool: &PgPool,
    registry: &PluginRegistry,
    blobs: &impl BlobsHooks,
) -> sqlx::Result<()> {
    let current = fingerprint(pool).await?;
    if registry.loaded_from().as_deref() == Some(current.as_str()) {
        return Ok(());
    }
    let plugins = load_all(pool, blobs).await?;
    tracing::info!(installed = plugins.len(), "installed plugins loaded");
    registry.set_installed_from(plugins, current);
    Ok(())
}

/// [`sync`] for request paths: a failure keeps serving the current registry.
pub async fn sync_or_warn(pool: &PgPool, registry: &PluginRegistry, blobs: &impl BlobsHooks) {
    if let Err(e) = sync(pool, registry, blobs).await {
        tracing::warn!("plugin registry sync failed: {e}");
    }
}

/// Validate `bytes`, store them, upsert the row and register the plugin. An
/// existing id is upgraded in place and keeps its instance toggle; its
/// previous archive is collected once nothing else references it.
pub async fn install(
    pool: &PgPool,
    registry: &PluginRegistry,
    bytes: &[u8],
    installed_by: Option<Uuid>,
    blobs: &impl BlobsHooks,
) -> Result<Plugin, InstallError> {
    let mut plugin = load_archive(bytes, false)?;
    let manifest = serde_json::to_value(&plugin.manifest).unwrap_or_default();
    for _ in 0..STORE_ATTEMPTS {
        let blob = blobs.store_blob(pool, bytes, Some(ARCHIVE_MEDIA_TYPE)).await?;
        let mut tx = pool.begin().await?;
        if !hold_blob(&mut tx, &blob.hash).await? {
            continue;
        }
        let previous: Option<String> =
            sqlx::query_scalar("SELECT archive_hash FROM plugins WHERE id = $1 FOR UPDATE")
                .bind(&plugin.manifest.id)
                .fetch_optional(&mut *tx)
                .await?;
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
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        plugin.instance_enabled = enabled;
        registry.upsert_installed(plugin.clone());
        if let Some(old) = previous.filter(|old| *old != blob.hash) {
            blobs.delete_unreferenced(pool, &old).await;
        }
        return Ok(plugin);
    }
    Err(InstallError::Db(sqlx::Error::Protocol(
        "the plugin archive was removed while it was being installed".to_owned(),
    )))
}

/// A stored archive can be collected before the plugin row references it;
/// the store is then retried.
const STORE_ATTEMPTS: usize = 3;

/// Share-locks the archive's blob row until the transaction ends, so the blob
/// collector, which locks the row before checking references, sees the
/// plugin row this transaction writes; `false` when the row is already gone.
async fn hold_blob(conn: &mut sqlx::PgConnection, hash: &str) -> sqlx::Result<bool> {
    let held: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM daemon_blobs WHERE hash = $1 FOR KEY SHARE")
            .bind(hash)
            .fetch_optional(conn)
            .await?;
    Ok(held.is_some())
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

/// Remove the row, its settings and the registry entry, then the archive blob
/// when nothing else references it; `Ok(false)` when `id` is not an installed
/// plugin.
pub async fn uninstall(
    pool: &PgPool,
    registry: &PluginRegistry,
    id: &str,
    blobs: &impl BlobsHooks,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let hash: Option<String> =
        sqlx::query_scalar("DELETE FROM plugins WHERE id = $1 RETURNING archive_hash")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(hash) = hash else { return Ok(false) };
    sqlx::query("DELETE FROM plugin_settings WHERE plugin_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    registry.remove_installed(id);
    blobs.delete_unreferenced(pool, &hash).await;
    Ok(true)
}

/// Download an archive from an https URL through the SSRF-guarded client,
/// following redirects (release hosts like GitHub serve assets via a 302) and
/// re-validating every hop.
pub async fn fetch_archive(url: &str) -> Result<Vec<u8>, InstallError> {
    let allow = cctui_server_kernel::outbound::upstream_allowlist();
    let mut url = url.to_owned();
    let mut hops = 0;
    let resp = loop {
        if !url.starts_with("https://") {
            return Err(InstallError::Url(
                cctui_server_kernel::outbound::OutboundUrlError::NotHttps,
            ));
        }
        cctui_server_kernel::outbound::validate_outbound_url(&url, &allow)
            .await
            .map_err(InstallError::Url)?;
        let resp = cctui_server_kernel::outbound::upstream_client()
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
    let base = reqwest::Url::parse(from).map_err(|_| {
        InstallError::Url(cctui_server_kernel::outbound::OutboundUrlError::Malformed)
    })?;
    base.join(location)
        .map(String::from)
        .map_err(|_| InstallError::Url(cctui_server_kernel::outbound::OutboundUrlError::Malformed))
}

#[cfg(test)]
mod tests {
    use super::fetch_archive;
    use crate::plugin_archive::test_support::plugin_tgz;
    use crate::plugins::PluginRegistry;

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

    /// Stores archives as plain rows and records what it is asked to collect;
    /// with `steal` set, the first stored row is deleted before install holds it.
    #[derive(Default)]
    struct Recording {
        collected: std::sync::Mutex<Vec<String>>,
        steal: std::sync::atomic::AtomicBool,
    }

    impl crate::hooks::BlobsHooks for Recording {
        async fn store_blob(
            &self,
            pool: &sqlx::PgPool,
            bytes: &[u8],
            media_type: Option<&str>,
        ) -> Result<cctui_server_kernel::blobs_types::StoredBlob, sqlx::Error> {
            use sha2::Digest;
            let hash = hex::encode(sha2::Sha256::digest(bytes));
            sqlx::query(
                "INSERT INTO daemon_blobs (hash, media_type, byte_len, bytes) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (hash) DO NOTHING",
            )
            .bind(&hash)
            .bind(media_type)
            .bind(i64::try_from(bytes.len()).unwrap())
            .bind(bytes)
            .execute(pool)
            .await?;
            if self.steal.swap(false, std::sync::atomic::Ordering::Relaxed) {
                sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
                    .bind(&hash)
                    .execute(pool)
                    .await?;
            }
            Ok(cctui_server_kernel::blobs_types::StoredBlob { hash, created: true })
        }

        async fn read_blob_in(
            &self,
            _conn: &mut sqlx::PgConnection,
            _hash: &str,
        ) -> sqlx::Result<Option<Vec<u8>>> {
            Ok(None)
        }

        async fn delete_unreferenced(&self, _pool: &sqlx::PgPool, hash: &str) {
            self.collected.lock().unwrap().push(hash.to_owned());
        }
    }

    async fn store_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = cctui_server_kernel::test_support::db_url(name)?;
        Some(sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap())
    }

    async fn archive_hash(pool: &sqlx::PgPool, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT archive_hash FROM plugins WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    async fn forget(pool: &sqlx::PgPool, id: &str, hashes: &[String]) {
        sqlx::query("DELETE FROM plugins WHERE id = $1").bind(id).execute(pool).await.unwrap();
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = ANY($1)")
            .bind(hashes)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_upgrade_collects_the_previous_archive_and_uninstall_the_last() {
        const ID: &str = "plugin-store-collects-archives";
        let Some(pool) = store_pool("plugin_store_collects_archives").await else { return };
        forget(&pool, ID, &[]).await;
        let (registry, blobs) = (PluginRegistry::disabled(), Recording::default());
        let v1 = plugin_tgz(ID, None, "1.0.0");

        super::install(&pool, &registry, &v1, None, &blobs).await.unwrap();
        let first = archive_hash(&pool, ID).await.unwrap();
        super::install(&pool, &registry, &v1, None, &blobs).await.unwrap();
        assert!(blobs.collected.lock().unwrap().is_empty(), "the same archive stays");

        super::install(&pool, &registry, &plugin_tgz(ID, None, "1.1.0"), None, &blobs)
            .await
            .unwrap();
        let second = archive_hash(&pool, ID).await.unwrap();
        assert_ne!(first, second);
        assert_eq!(*blobs.collected.lock().unwrap(), vec![first.clone()]);

        sqlx::query("INSERT INTO plugin_settings (plugin_id) VALUES ($1)")
            .bind(ID)
            .execute(&pool)
            .await
            .unwrap();
        assert!(super::uninstall(&pool, &registry, ID, &blobs).await.unwrap());
        assert_eq!(*blobs.collected.lock().unwrap(), vec![first.clone(), second.clone()]);
        assert_eq!(archive_hash(&pool, ID).await, None);
        let settings: i64 =
            sqlx::query_scalar("SELECT count(*) FROM plugin_settings WHERE plugin_id = $1")
                .bind(ID)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(settings, 0);
        assert!(!registry.all_admin().iter().any(|p| p.manifest.id == ID));
        assert!(!super::uninstall(&pool, &registry, ID, &blobs).await.unwrap());
        forget(&pool, ID, &[first, second]).await;
    }

    #[tokio::test]
    async fn install_stores_again_when_its_archive_is_collected_first() {
        const ID: &str = "plugin-store-archive-collected";
        let Some(pool) = store_pool("plugin_store_archive_collected").await else { return };
        forget(&pool, ID, &[]).await;
        let (registry, blobs) = (PluginRegistry::disabled(), Recording::default());
        blobs.steal.store(true, std::sync::atomic::Ordering::Relaxed);

        super::install(&pool, &registry, &plugin_tgz(ID, None, "1.0.0"), None, &blobs)
            .await
            .unwrap();
        let hash = archive_hash(&pool, ID).await.unwrap();
        let stored: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM daemon_blobs WHERE hash = $1)")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(stored, "the plugin row never points at a missing archive");
        forget(&pool, ID, &[hash]).await;
    }
}
