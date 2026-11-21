//! Off unless `CCTUI_BLOB_MOVE=1`; procedure in `docs/image-blobs.md`.

use std::collections::HashSet;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool};

use crate::blob_store::{self, BlobStore};

const DEFAULT_BATCH: i64 = 20;
const DEFAULT_PAUSE: Duration = Duration::from_millis(500);
const ERROR_BACKOFF: Duration = Duration::from_secs(30);
const PROGRESS_EVERY: u64 = 100;
const LOCK_KEY: i64 = 0x6363_7475_695f_626d;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub batch: i64,
    pub pause: Duration,
    pub budget_bytes: Option<u64>,
}

impl Config {
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let on = get("CCTUI_BLOB_MOVE")
            .map(|v| v.trim().to_ascii_lowercase())
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "on" | "yes"));
        if !on {
            return None;
        }
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<u64>().ok());
        Some(Self {
            batch: num("CCTUI_BLOB_MOVE_BATCH")
                .filter(|n| *n > 0)
                .and_then(|n| i64::try_from(n).ok())
                .unwrap_or(DEFAULT_BATCH),
            pause: num("CCTUI_BLOB_MOVE_PAUSE_MS").map_or(DEFAULT_PAUSE, Duration::from_millis),
            budget_bytes: num("CCTUI_BLOB_MOVE_BUDGET_MB")
                .filter(|n| *n > 0)
                .map(|n| n.saturating_mul(1024 * 1024)),
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub moved: u64,
    pub bytes: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    More,
    Done,
    Budget,
}

struct Mover<'a> {
    store: &'a BlobStore,
    batch: i64,
    budget_bytes: Option<u64>,
    scope: Option<Vec<String>>,
    skip: HashSet<String>,
    progress: Progress,
}

const BATCH_SQL: &str = "SELECT hash, byte_len, bytes FROM daemon_blobs \
     WHERE NOT file_stored AND octet_length(bytes) > 0 AND hash <> ALL($2) \
       AND ($3::text[] IS NULL OR hash = ANY($3)) \
     ORDER BY hash LIMIT $1";

const REMAINING_SQL: &str = "SELECT count(*) FROM daemon_blobs \
     WHERE NOT file_stored AND octet_length(bytes) > 0 \
       AND ($1::text[] IS NULL OR hash = ANY($1))";

impl<'a> Mover<'a> {
    fn new(store: &'a BlobStore, config: &Config) -> Self {
        Self {
            store,
            batch: config.batch.max(1),
            budget_bytes: config.budget_bytes,
            scope: None,
            skip: HashSet::new(),
            progress: Progress::default(),
        }
    }

    fn skip_blob(&mut self, hash: String, reason: &str) {
        tracing::warn!(hash = %hash, reason, "blob move skipped a blob");
        self.skip.insert(hash);
        self.progress.skipped += 1;
    }

    async fn remaining(&self, pool: &PgPool) -> sqlx::Result<i64> {
        sqlx::query_scalar(REMAINING_SQL).bind(self.scope.as_deref()).fetch_one(pool).await
    }

    async fn step(&mut self, pool: &PgPool) -> sqlx::Result<Step> {
        let skipped: Vec<String> = self.skip.iter().cloned().collect();
        let rows: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(BATCH_SQL)
            .bind(self.batch)
            .bind(&skipped)
            .bind(self.scope.as_deref())
            .fetch_all(pool)
            .await?;
        let full = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= self.batch;
        for (hash, byte_len, bytes) in rows {
            if self.budget_bytes.is_some_and(|b| self.progress.bytes >= b) {
                return Ok(Step::Budget);
            }
            if i64::try_from(bytes.len()).ok() != Some(byte_len) {
                self.skip_blob(hash, "length does not match byte_len");
                continue;
            }
            if hex::encode(Sha256::digest(&bytes)) != hash {
                self.skip_blob(hash, "bytes do not match their hash");
                continue;
            }
            self.store.write_external(&hash, &bytes).await?;
            match self.store.read_external(&hash, byte_len).await {
                Ok(back) if back == bytes => {}
                Ok(_) => {
                    self.skip_blob(hash, "read back different bytes");
                    continue;
                }
                Err(e) => {
                    self.skip_blob(hash, &format!("read back failed: {e}"));
                    continue;
                }
            }
            if mark_external(pool, &hash).await? {
                self.progress.moved += 1;
                self.progress.bytes += bytes.len() as u64;
                if self.progress.moved.is_multiple_of(PROGRESS_EVERY) {
                    self.log_progress(pool, "blob move progress").await;
                }
            }
        }
        Ok(if full { Step::More } else { Step::Done })
    }

    async fn log_progress(&self, pool: &PgPool, msg: &str) {
        let remaining = self.remaining(pool).await.ok();
        let Progress { moved, bytes, skipped } = self.progress;
        tracing::info!(moved, bytes, skipped, remaining, "{msg}");
    }
}

async fn mark_external(pool: &PgPool, hash: &str) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE daemon_blobs SET file_stored = true, bytes = ''::bytea \
         WHERE hash = $1 AND NOT file_stored",
    )
    .bind(hash)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(updated == 1)
}

/// A detached connection, so the session lock goes with it if the task dies.
async fn try_lock(pool: &PgPool, key: i64) -> sqlx::Result<Option<PgConnection>> {
    let mut conn = pool.acquire().await?.detach();
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(&mut conn)
        .await?;
    if locked {
        Ok(Some(conn))
    } else {
        conn.close().await?;
        Ok(None)
    }
}

pub async fn task(pool: PgPool, config: Config) {
    let store = match blob_store::current() {
        Ok(store) if store.is_external() => store,
        Ok(_) => {
            tracing::warn!("blob move: no external blob store is configured, nothing to do");
            return;
        }
        Err(e) => {
            tracing::warn!("blob move: {e}");
            return;
        }
    };
    let lock = match try_lock(&pool, LOCK_KEY).await {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            tracing::info!("blob move: another replica holds the lock");
            return;
        }
        Err(e) => {
            tracing::warn!("blob move: lock failed: {e}");
            return;
        }
    };
    tracing::info!(budget_bytes = config.budget_bytes, "blob move enabled");
    let mut mover = Mover::new(store, &config);
    loop {
        match mover.step(&pool).await {
            Ok(Step::More) => tokio::time::sleep(config.pause).await,
            Ok(Step::Done) => break,
            Ok(Step::Budget) => {
                tracing::warn!("blob move stopped: byte budget reached");
                break;
            }
            Err(e) => {
                tracing::warn!("blob move batch failed: {e}");
                tokio::time::sleep(ERROR_BACKOFF).await;
            }
        }
    }
    mover.log_progress(&pool, "blob move finished").await;
    let _ = lock.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(vars: &[(&str, &str)]) -> Option<Config> {
        let vars: HashMap<String, String> =
            vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        Config::from_lookup(|k| vars.get(k).cloned())
    }

    #[test]
    fn blob_move_config_is_off_by_default_with_knobs() {
        assert_eq!(config(&[]), None);
        assert_eq!(config(&[("CCTUI_BLOB_MOVE", "0")]), None);
        assert_eq!(
            config(&[("CCTUI_BLOB_MOVE", "1")]),
            Some(Config { batch: 20, pause: Duration::from_millis(500), budget_bytes: None })
        );
        assert_eq!(
            config(&[
                ("CCTUI_BLOB_MOVE", "on"),
                ("CCTUI_BLOB_MOVE_BATCH", "5"),
                ("CCTUI_BLOB_MOVE_PAUSE_MS", "0"),
                ("CCTUI_BLOB_MOVE_BUDGET_MB", "2"),
            ]),
            Some(Config { batch: 5, pause: Duration::ZERO, budget_bytes: Some(2 * 1024 * 1024) })
        );
    }

    async fn test_pool(name: &str) -> Option<PgPool> {
        let url = cctui_server_kernel::test_support::db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(3)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn insert_raw(pool: &PgPool, hash: &str, bytes: &[u8]) {
        sqlx::query(
            "INSERT INTO daemon_blobs (hash, media_type, byte_len, bytes) \
             VALUES ($1, NULL, $2, $3)",
        )
        .bind(hash)
        .bind(i64::try_from(bytes.len()).unwrap())
        .bind(bytes)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn row(pool: &PgPool, hash: &str) -> (bool, Vec<u8>) {
        sqlx::query_as("SELECT file_stored, bytes FROM daemon_blobs WHERE hash = $1")
            .bind(hash)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn drain(mover: &mut Mover<'_>, pool: &PgPool) -> Step {
        loop {
            match mover.step(pool).await.unwrap() {
                Step::More => {}
                stop => return stop,
            }
        }
    }

    fn mover<'a>(store: &'a BlobStore, scope: &[String], config: &Config) -> Mover<'a> {
        let mut m = Mover::new(store, config);
        m.scope = Some(scope.to_vec());
        m
    }

    const TEST_CONFIG: Config = Config { batch: 2, pause: Duration::ZERO, budget_bytes: None };

    async fn seed_session(pool: &PgPool, sid: &str, hashes: &[String]) -> (uuid::Uuid, uuid::Uuid) {
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'blob-move', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("kh-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(sid)
        .bind(machine)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_blob_links (session_id, hash) \
             SELECT $1, * FROM UNNEST($2::text[])",
        )
        .bind(sid)
        .bind(hashes)
        .execute(pool)
        .await
        .unwrap();
        (uid, machine)
    }

    async fn cleanup(pool: &PgPool, sid: &str, owner: (uuid::Uuid, uuid::Uuid), hashes: &[String]) {
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(pool).await.unwrap();
        sqlx::query("DELETE FROM machines WHERE id = $1")
            .bind(owner.1)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(owner.0).execute(pool).await.unwrap();
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = ANY($1)")
            .bind(hashes)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn blob_move_moves_verified_blobs_and_skips_corrupt_ones() {
        let Some(pool) = test_pool("blob_move_moves_verified_blobs").await else { return };
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::File(dir.path().to_owned());
        let run = uuid::Uuid::new_v4();
        let blobs: Vec<Vec<u8>> =
            (0..3).map(|i| format!("blob move {run} #{i}").into_bytes()).collect();
        let mut hashes: Vec<String> =
            blobs.iter().map(|b| hex::encode(Sha256::digest(b))).collect();
        for (hash, bytes) in hashes.iter().zip(&blobs) {
            insert_raw(&pool, hash, bytes).await;
        }
        let corrupt_hash = hex::encode(Sha256::digest(format!("claimed {run}")));
        let corrupt_bytes = format!("actual {run}").into_bytes();
        insert_raw(&pool, &corrupt_hash, &corrupt_bytes).await;
        hashes.push(corrupt_hash.clone());
        let sid = format!("blob-move-{run}");
        let owner = seed_session(&pool, &sid, &hashes).await;

        let mut first = mover(&store, &hashes, &TEST_CONFIG);
        assert_eq!(drain(&mut first, &pool).await, Step::Done);
        let total: u64 = blobs.iter().map(|b| b.len() as u64).sum();
        assert_eq!(first.progress, Progress { moved: 3, bytes: total, skipped: 1 });

        let mut conn = pool.acquire().await.unwrap();
        for (hash, bytes) in hashes.iter().zip(&blobs) {
            assert_eq!(row(&pool, hash).await, (true, Vec::new()), "bytes cleared");
            assert_eq!(std::fs::read(dir.path().join(hash)).unwrap(), *bytes);
            let read = crate::routes::read_blob_with(&mut conn, &store, hash).await;
            assert_eq!(read.unwrap().as_deref(), Some(bytes.as_slice()));
            let served = crate::routes::fetch_session_blob_with(&pool, &store, &sid, hash).await;
            assert_eq!(served.unwrap().map(|(_, b)| b).as_deref(), Some(bytes.as_slice()));
        }
        drop(conn);
        assert_eq!(row(&pool, &corrupt_hash).await, (false, corrupt_bytes), "left untouched");
        assert!(!dir.path().join(&corrupt_hash).exists());

        let mut second = mover(&store, &hashes, &TEST_CONFIG);
        assert_eq!(drain(&mut second, &pool).await, Step::Done);
        assert_eq!(second.progress, Progress { moved: 0, bytes: 0, skipped: 1 });
        assert_eq!(second.remaining(&pool).await.unwrap(), 1, "only the corrupt row is left");

        cleanup(&pool, &sid, owner, &hashes).await;
    }

    #[tokio::test]
    async fn blob_move_stops_at_its_byte_budget() {
        let Some(pool) = test_pool("blob_move_stops_at_its_byte_budget").await else { return };
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::File(dir.path().to_owned());
        let run = uuid::Uuid::new_v4();
        let mut hashes = Vec::new();
        for i in 0..3 {
            let bytes = format!("budget {run} #{i}").into_bytes();
            let hash = hex::encode(Sha256::digest(&bytes));
            insert_raw(&pool, &hash, &bytes).await;
            hashes.push(hash);
        }
        let config = Config { budget_bytes: Some(1), ..TEST_CONFIG };
        let mut budgeted = mover(&store, &hashes, &config);
        assert_eq!(drain(&mut budgeted, &pool).await, Step::Budget);
        assert_eq!(budgeted.progress.moved, 1);
        assert_eq!(budgeted.remaining(&pool).await.unwrap(), 2);
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = ANY($1)")
            .bind(&hashes)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn blob_move_lock_admits_one_replica() {
        let Some(pool) = test_pool("blob_move_lock_admits_one_replica").await else { return };
        let key = LOCK_KEY ^ i64::from(rand_key());
        let held = try_lock(&pool, key).await.unwrap().expect("first replica locks");
        assert!(try_lock(&pool, key).await.unwrap().is_none(), "second replica is refused");
        held.close().await.unwrap();
        let again = try_lock(&pool, key).await.unwrap().expect("released with its connection");
        again.close().await.unwrap();
    }

    fn rand_key() -> u32 {
        uuid::Uuid::new_v4().as_u128() as u32 | 1
    }
}
