//! Where blob bytes live. `daemon_blobs.file_stored = true` rows keep only
//! metadata; their bytes are in the external store configured at boot.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cctui_aws::AwsCredentials;
use cctui_aws::s3::S3Client;
use sha2::{Digest, Sha256};

use crate::routes::is_sha256_hex;

const DEFAULT_REGION: &str = "us-east-1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Config {
    Postgres,
    File(PathBuf),
    S3(S3Config),
}

#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub prefix: String,
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let var = |k: &str| get(k).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        match (var("CCTUI_BLOB_S3_BUCKET"), var("CCTUI_BLOB_PATH")) {
            (Some(_), Some(_)) => anyhow::bail!(
                "CCTUI_BLOB_S3_BUCKET and CCTUI_BLOB_PATH are both set: configure one blob store"
            ),
            (None, Some(path)) => Ok(Self::File(path.into())),
            (None, None) => Ok(Self::Postgres),
            (Some(bucket), None) => {
                let required = |k: &str| {
                    var(k).ok_or_else(|| {
                        anyhow::anyhow!("{k} is required when CCTUI_BLOB_S3_BUCKET is set")
                    })
                };
                Ok(Self::S3(S3Config {
                    endpoint: required("CCTUI_BLOB_S3_ENDPOINT")?,
                    bucket,
                    region: var("CCTUI_BLOB_S3_REGION")
                        .unwrap_or_else(|| DEFAULT_REGION.to_owned()),
                    access_key_id: required("CCTUI_BLOB_S3_ACCESS_KEY_ID")?,
                    secret_access_key: required("CCTUI_BLOB_S3_SECRET_ACCESS_KEY")?,
                    prefix: var("CCTUI_BLOB_S3_PREFIX").unwrap_or_default(),
                }))
            }
        }
    }
}

#[derive(Debug)]
pub enum BlobStore {
    Postgres,
    File(PathBuf),
    S3 { client: S3Client, prefix: String },
}

impl BlobStore {
    pub fn from_config(config: Config) -> anyhow::Result<Self> {
        Ok(match config {
            Config::Postgres => Self::Postgres,
            Config::File(root) => Self::File(root),
            Config::S3(s3) => Self::S3 {
                client: S3Client::new(
                    &s3.endpoint,
                    &s3.bucket,
                    &s3.region,
                    AwsCredentials::new(s3.access_key_id, s3.secret_access_key),
                )?,
                prefix: s3.prefix,
            },
        })
    }

    #[must_use]
    pub const fn is_external(&self) -> bool {
        !matches!(self, Self::Postgres)
    }

    fn describe(&self) -> String {
        match self {
            Self::Postgres => "postgres".to_owned(),
            Self::File(root) => format!("file {}", root.display()),
            Self::S3 { client, prefix } => format!("s3 {client:?} prefix {prefix:?}"),
        }
    }

    pub async fn write_external(&self, hash: &str, bytes: &[u8]) -> sqlx::Result<()> {
        if !is_sha256_hex(hash) {
            return Err(protocol("invalid blob key"));
        }
        if hex::encode(Sha256::digest(bytes)) != hash {
            return Err(protocol("blob bytes do not match their hash"));
        }
        match self {
            Self::Postgres => Err(protocol("no external blob store is configured")),
            Self::File(root) => write_file(root, hash, bytes).await,
            Self::S3 { client, prefix } => {
                let key = format!("{prefix}{hash}");
                let present = client.head_object(&key).await.map_err(|e| s3_error(&e))?;
                if present == Some(bytes.len() as u64) {
                    return Ok(());
                }
                client.put_object(&key, bytes, hash).await.map_err(|e| s3_error(&e))
            }
        }
    }

    pub async fn read_external(&self, hash: &str, byte_len: i64) -> sqlx::Result<Vec<u8>> {
        if !is_sha256_hex(hash) {
            return Err(protocol("invalid blob key"));
        }
        let bytes = match self {
            Self::Postgres => {
                return Err(protocol(
                    "an externally stored blob needs CCTUI_BLOB_PATH or CCTUI_BLOB_S3_BUCKET",
                ));
            }
            Self::File(root) => tokio::fs::read(root.join(hash)).await?,
            Self::S3 { client, prefix } => client
                .get_object(&format!("{prefix}{hash}"))
                .await
                .map_err(|e| s3_error(&e))?
                .ok_or_else(|| protocol("blob object is missing"))?,
        };
        if i64::try_from(bytes.len()).ok() != Some(byte_len)
            || hex::encode(Sha256::digest(&bytes)) != hash
        {
            return Err(protocol("blob file hash mismatch"));
        }
        Ok(bytes)
    }

    pub async fn delete_external(&self, hash: &str) {
        if !is_sha256_hex(hash) {
            return;
        }
        let result = match self {
            Self::Postgres => Ok(()),
            Self::File(root) => tokio::fs::remove_file(root.join(hash)).await.or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(anyhow::Error::new(e))
                }
            }),
            Self::S3 { client, prefix } => client.delete_object(&format!("{prefix}{hash}")).await,
        };
        if let Err(e) = result {
            tracing::warn!(hash, "external blob delete failed: {e:#}");
        }
    }
}

static STORE: OnceLock<Result<BlobStore, String>> = OnceLock::new();

fn load() -> Result<BlobStore, String> {
    Config::from_env().and_then(BlobStore::from_config).map_err(|e| format!("{e:#}"))
}

pub fn init() -> anyhow::Result<()> {
    let store = STORE.get_or_init(load).as_ref().map_err(|e| anyhow::anyhow!("blob store: {e}"))?;
    tracing::info!(backend = %store.describe(), "blob store");
    Ok(())
}

pub fn current() -> sqlx::Result<&'static BlobStore> {
    STORE.get_or_init(load).as_ref().map_err(|e| protocol(&format!("blob store: {e}")))
}

/// Removes the blob's row and its external bytes once nothing references it.
pub async fn delete_unreferenced(pool: &sqlx::PgPool, hash: &str) {
    let store = match current() {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(hash, "blob delete skipped: {e}");
            return;
        }
    };
    delete_unreferenced_with(pool, store, hash).await;
}

pub async fn delete_unreferenced_with(pool: &sqlx::PgPool, store: &BlobStore, hash: &str) {
    if let Err(e) = collect_unreferenced(pool, store, hash).await {
        tracing::warn!(hash, "blob delete skipped: {e}");
    }
}

/// Every table that points at a `daemon_blobs` row.
const REFERENCED_SQL: &str = "SELECT EXISTS (SELECT 1 FROM plugins WHERE archive_hash = $1) \
     OR EXISTS (SELECT 1 FROM session_attachments WHERE hash = $1) \
     OR EXISTS (SELECT 1 FROM session_blob_links WHERE hash = $1)";

/// The row lock comes first so the reference check runs on a snapshot taken
/// after any concurrent store or referencing write committed, and the
/// external bytes go while the row is still locked.
async fn collect_unreferenced(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    hash: &str,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let row: Option<bool> =
        sqlx::query_scalar("SELECT file_stored FROM daemon_blobs WHERE hash = $1 FOR UPDATE")
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(file_stored) = row else { return Ok(false) };
    let referenced: bool =
        sqlx::query_scalar(REFERENCED_SQL).bind(hash).fetch_one(&mut *tx).await?;
    if referenced {
        return Ok(false);
    }
    sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1").bind(hash).execute(&mut *tx).await?;
    if file_stored {
        store.delete_external(hash).await;
    }
    tx.commit().await?;
    Ok(true)
}

fn protocol(msg: &str) -> sqlx::Error {
    sqlx::Error::Protocol(msg.to_owned())
}

fn s3_error(e: &anyhow::Error) -> sqlx::Error {
    sqlx::Error::Protocol(format!("{e:#}"))
}

async fn write_file(root: &Path, hash: &str, bytes: &[u8]) -> sqlx::Result<()> {
    tokio::fs::create_dir_all(root).await?;
    let path = root.join(hash);
    if let Ok(existing) = tokio::fs::read(&path).await {
        if existing == bytes {
            return Ok(());
        }
        return Err(protocol("blob file hash mismatch"));
    }
    // A unique temporary inode and atomic publication keep concurrent uploads
    // and interrupted writes from exposing partial content.
    let tmp = root.join(format!(".{}.{}", hash, uuid::Uuid::new_v4()));
    let result = async {
        use tokio::io::AsyncWriteExt;
        let mut file =
            tokio::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&tmp, &path).await?;
        // Persist the directory entry before a database commit references it.
        let dir = tokio::fs::File::open(root).await?;
        dir.sync_all().await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(vars: &[(&str, &str)]) -> anyhow::Result<Config> {
        let vars: HashMap<String, String> =
            vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        Config::from_lookup(|k| vars.get(k).cloned())
    }

    const S3_VARS: [(&str, &str); 4] = [
        ("CCTUI_BLOB_S3_BUCKET", "cctui-1"),
        ("CCTUI_BLOB_S3_ENDPOINT", "http://versitygw.storage.svc:7070"),
        ("CCTUI_BLOB_S3_ACCESS_KEY_ID", "AKID"),
        ("CCTUI_BLOB_S3_SECRET_ACCESS_KEY", "s3-secret"),
    ];

    #[test]
    fn blob_store_config_selects_s3() {
        let Config::S3(s3) = lookup(&S3_VARS).unwrap() else { panic!("expected s3") };
        assert_eq!(
            s3,
            S3Config {
                endpoint: "http://versitygw.storage.svc:7070".to_owned(),
                bucket: "cctui-1".to_owned(),
                region: "us-east-1".to_owned(),
                access_key_id: "AKID".to_owned(),
                secret_access_key: "s3-secret".to_owned(),
                prefix: String::new(),
            }
        );
        assert!(!format!("{s3:?}").contains("s3-secret"));
        let mut vars = S3_VARS.to_vec();
        vars.extend([("CCTUI_BLOB_S3_REGION", "eu-west-3"), ("CCTUI_BLOB_S3_PREFIX", "blobs/")]);
        let Config::S3(s3) = lookup(&vars).unwrap() else { panic!("expected s3") };
        assert_eq!((s3.region.as_str(), s3.prefix.as_str()), ("eu-west-3", "blobs/"));
    }

    #[test]
    fn blob_store_config_selects_the_file_backend() {
        assert_eq!(
            lookup(&[("CCTUI_BLOB_PATH", "/data/blobs")]).unwrap(),
            Config::File("/data/blobs".into())
        );
    }

    #[test]
    fn blob_store_config_refuses_both_backends() {
        let mut vars = S3_VARS.to_vec();
        vars.push(("CCTUI_BLOB_PATH", "/data/blobs"));
        let err = lookup(&vars).unwrap_err().to_string();
        assert!(err.contains("CCTUI_BLOB_S3_BUCKET") && err.contains("CCTUI_BLOB_PATH"), "{err}");
    }

    #[test]
    fn blob_store_config_requires_every_s3_field() {
        for missing in [
            "CCTUI_BLOB_S3_ENDPOINT",
            "CCTUI_BLOB_S3_ACCESS_KEY_ID",
            "CCTUI_BLOB_S3_SECRET_ACCESS_KEY",
        ] {
            let vars: Vec<_> = S3_VARS.iter().copied().filter(|(k, _)| *k != missing).collect();
            let err = lookup(&vars).unwrap_err().to_string();
            assert!(err.contains(missing), "{err}");
            assert!(!err.contains("s3-secret"), "{err}");
        }
    }

    #[test]
    fn blob_store_config_defaults_to_postgres() {
        assert_eq!(lookup(&[]).unwrap(), Config::Postgres);
        assert_eq!(
            lookup(&[("CCTUI_BLOB_PATH", " "), ("CCTUI_BLOB_S3_BUCKET", "")]).unwrap(),
            Config::Postgres
        );
        assert_eq!(
            lookup(&[("CCTUI_BLOB_S3_ENDPOINT", "http://unused:7070")]).unwrap(),
            Config::Postgres
        );
    }

    #[tokio::test]
    async fn file_backend_publishes_verified_bytes_and_refuses_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::File(dir.path().to_owned());
        let bytes = b"durable attachment";
        let hash = hex::encode(Sha256::digest(bytes));
        let len = i64::try_from(bytes.len()).unwrap();
        store.write_external(&hash, bytes).await.unwrap();
        store.write_external(&hash, bytes).await.unwrap();
        assert_eq!(store.read_external(&hash, len).await.unwrap(), bytes);
        assert!(store.read_external("../private", 1).await.is_err());
        assert!(store.read_external(&hash, 1).await.is_err());
        assert!(store.write_external(&hash, b"other bytes").await.is_err(), "hash checked");
        tokio::fs::write(dir.path().join(&hash), b"damaged").await.unwrap();
        assert!(store.read_external(&hash, len).await.is_err());
        assert!(store.write_external(&hash, bytes).await.is_err());
        store.delete_external(&hash).await;
        store.delete_external(&hash).await;
        assert!(!dir.path().join(&hash).exists());
    }

    #[tokio::test]
    async fn concurrent_file_uploads_are_complete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("blobs");
        let bytes = vec![7; 128 * 1024];
        let hash = hex::encode(Sha256::digest(&bytes));
        let (a, b) =
            tokio::join!(write_file(&root, &hash, &bytes), write_file(&root, &hash, &bytes));
        a.unwrap();
        b.unwrap();
        let store = BlobStore::File(root.clone());
        let len = i64::try_from(bytes.len()).unwrap();
        assert_eq!(store.read_external(&hash, len).await.unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn postgres_backend_refuses_external_io() {
        let hash = hex::encode(Sha256::digest(b"x"));
        assert!(BlobStore::Postgres.write_external(&hash, b"x").await.is_err());
        assert!(BlobStore::Postgres.read_external(&hash, 1).await.is_err());
    }

    async fn gc_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = cctui_server_kernel::test_support::db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(3)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn store(pool: &sqlx::PgPool, store: &BlobStore, tag: &str) -> String {
        let bytes = format!("{tag} {}", uuid::Uuid::new_v4()).into_bytes();
        let mut conn = pool.acquire().await.unwrap();
        crate::routes::store_blob_with(&mut conn, store, &bytes, None).await.unwrap().hash
    }

    async fn row_exists(pool: &sqlx::PgPool, hash: &str) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM daemon_blobs WHERE hash = $1)")
            .bind(hash)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn drop_session(pool: &sqlx::PgPool, uid: uuid::Uuid, hash: &str) {
        for sql in [
            "DELETE FROM sessions WHERE user_id = $1",
            "DELETE FROM machines WHERE user_id = $1",
            "DELETE FROM users WHERE id = $1",
        ] {
            sqlx::query(sql).bind(uid).execute(pool).await.unwrap();
        }
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
            .bind(hash)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_unreferenced_blob_is_collected_with_its_bytes() {
        let Some(pool) = gc_pool("blob_gc_unreferenced").await else { return };
        let dir = tempfile::tempdir().unwrap();
        let store_ = BlobStore::File(dir.path().to_owned());
        let hash = store(&pool, &store_, "unreferenced").await;
        assert!(dir.path().join(&hash).exists());

        assert!(collect_unreferenced(&pool, &store_, &hash).await.unwrap());
        assert!(!row_exists(&pool, &hash).await);
        assert!(!dir.path().join(&hash).exists());
        assert!(!collect_unreferenced(&pool, &store_, &hash).await.unwrap(), "already gone");
    }

    #[tokio::test]
    async fn a_blob_linked_to_a_session_is_not_collected() {
        let name = "blob_gc_session_link";
        let Some(pool) = gc_pool(name).await else { return };
        let dir = tempfile::tempdir().unwrap();
        let store_ = BlobStore::File(dir.path().to_owned());
        let (uid, sid) = cctui_server_kernel::test_support::seed_session(&pool, name).await;
        let hash = store(&pool, &store_, "linked").await;
        sqlx::query("INSERT INTO session_blob_links (session_id, hash) VALUES ($1, $2)")
            .bind(&sid)
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();

        delete_unreferenced_with(&pool, &store_, &hash).await;
        assert!(row_exists(&pool, &hash).await);
        assert!(dir.path().join(&hash).exists(), "a linked blob keeps its bytes");
        drop_session(&pool, uid, &hash).await;
    }

    #[tokio::test]
    async fn collection_waits_for_a_concurrent_store_and_its_reference() {
        let name = "blob_gc_races_store";
        let Some(pool) = gc_pool(name).await else { return };
        let dir = tempfile::tempdir().unwrap();
        let store_ = BlobStore::File(dir.path().to_owned());
        let (uid, sid) = cctui_server_kernel::test_support::seed_session(&pool, name).await;
        let hash = store(&pool, &store_, "restored").await;
        let bytes = tokio::fs::read(dir.path().join(&hash)).await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let again = crate::routes::store_blob_with(&mut tx, &store_, &bytes, None).await.unwrap();
        assert!(!again.created);
        let gc = tokio::spawn({
            let (pool, root, hash) = (pool.clone(), dir.path().to_owned(), hash.clone());
            async move { collect_unreferenced(&pool, &BlobStore::File(root), &hash).await.unwrap() }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!gc.is_finished(), "the collection waits for the storing transaction");
        sqlx::query("INSERT INTO session_blob_links (session_id, hash) VALUES ($1, $2)")
            .bind(&sid)
            .bind(&hash)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(!gc.await.unwrap(), "the reference committed with the store keeps the blob");
        assert!(row_exists(&pool, &hash).await);
        assert!(dir.path().join(&hash).exists());
        drop_session(&pool, uid, &hash).await;
    }
}
