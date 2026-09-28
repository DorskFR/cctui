use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("invalid skill name")]
    InvalidName,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct SkillStats {
    pub sha256: String,
    pub size_bytes: u64,
}

const BUNDLE_SUFFIX: &str = ".tar.zst";

/// Everything `skill_registry` still points at, for [`SkillStore::sweep_orphans`].
#[derive(Debug, Default)]
pub struct SkillRefs {
    /// `(owner, name)` per row. Keyed by the parsed owner rather than a path, so
    /// no directory-name normalisation can make a live bundle look unreferenced.
    pub bundles: HashSet<(Uuid, String)>,
    /// Every referenced name, whatever its owner. A root-level bundle whose name
    /// is still registered may yet be claimed by [`SkillStore::adopt_legacy`], so
    /// it is not an orphan.
    pub names: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct SkillStore {
    root: PathBuf,
}

impl SkillStore {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn ensure_root(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root).await
    }

    /// Path to the active bundle for `owner`'s skill. One bundle per owner and
    /// name, overwritten on each upload.
    #[must_use]
    pub fn path_of(&self, owner: Uuid, name: &str) -> PathBuf {
        self.root.join(owner.to_string()).join(format!("{name}{BUNDLE_SUFFIX}"))
    }

    fn legacy_path_of(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}{BUNDLE_SUFFIX}"))
    }

    /// Delete bundles no `skill_registry` row can reach — migration 133 dropped
    /// the ownerless rows but left their files on disk. Idempotent; returns how
    /// many it removed.
    ///
    /// Deliberately narrow: it deletes only a regular file named `*.tar.zst` that
    /// `refs` does not account for. A referenced bundle, an in-flight `.partial`,
    /// a symlink, a non-UUID directory and anything nested deeper all survive.
    ///
    /// `refs` MUST come from a query that succeeded — a short set reads as
    /// "unreferenced" and would delete live bundles.
    pub async fn sweep_orphans(&self, refs: &SkillRefs) -> std::io::Result<usize> {
        let mut entries = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let raw = entry.file_name();
            let Some(name) = raw.to_str() else { continue };
            if file_type.is_dir() {
                // Only an owner directory; anything else is not ours to touch.
                if let Ok(owner) = Uuid::parse_str(name) {
                    removed += Self::sweep_owner_dir(owner, &entry.path(), refs).await?;
                }
            } else if file_type.is_file() {
                // Root level is the pre-133 ownerless layout.
                let Some(stem) = name.strip_suffix(BUNDLE_SUFFIX) else { continue };
                if refs.names.contains(stem) {
                    continue;
                }
                removed += usize::from(remove_orphan(&entry.path()).await);
            }
        }
        if removed > 0 {
            tracing::info!(root = %self.root.display(), removed, "swept ownerless skill bundles");
        }
        Ok(removed)
    }

    async fn sweep_owner_dir(owner: Uuid, dir: &Path, refs: &SkillRefs) -> std::io::Result<usize> {
        let mut entries = match fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            let raw = entry.file_name();
            let Some(name) = raw.to_str() else { continue };
            let Some(stem) = name.strip_suffix(BUNDLE_SUFFIX) else { continue };
            if refs.bundles.contains(&(owner, stem.to_string())) {
                continue;
            }
            removed += usize::from(remove_orphan(&entry.path()).await);
        }
        Ok(removed)
    }

    /// Move a bundle from the ownerless layout into `owner`'s directory, but
    /// only when its bytes hash to the owner's recorded `sha256`, so a file
    /// another account overwrote is never handed to the owner.
    pub async fn adopt_legacy(
        &self,
        owner: Uuid,
        name: &str,
        sha256: &str,
    ) -> Result<bool, SkillError> {
        validate_name(name)?;
        let legacy = self.legacy_path_of(name);
        let mut file = match fs::File::open(&legacy).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        drop(file);
        if hex::encode(hasher.finalize()) != sha256 {
            return Ok(false);
        }
        let target = self.path_of(owner, name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(&legacy, &target).await?;
        Ok(true)
    }

    pub async fn write<R: AsyncRead + Unpin>(
        &self,
        owner: Uuid,
        name: &str,
        mut body: R,
    ) -> Result<SkillStats, SkillError> {
        validate_name(name)?;

        let final_path = self.path_of(owner, name);
        let partial_path = final_path.with_extension(format!("zst.{}.partial", Uuid::new_v4()));
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let outcome = async {
            let mut file = fs::File::create(&partial_path).await?;
            let mut hasher = Sha256::new();
            let mut size: u64 = 0;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = body.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                size += n as u64;
                file.write_all(&buf[..n]).await?;
            }
            file.flush().await?;
            drop(file);
            Ok::<_, std::io::Error>(SkillStats {
                sha256: hex::encode(hasher.finalize()),
                size_bytes: size,
            })
        }
        .await;

        match outcome {
            Ok(stats) => {
                fs::rename(&partial_path, &final_path).await?;
                Ok(stats)
            }
            Err(e) => {
                let _ = fs::remove_file(&partial_path).await;
                Err(e.into())
            }
        }
    }
}

/// A failed unlink is logged and skipped: a sweep must never abort startup.
async fn remove_orphan(path: &Path) -> bool {
    match fs::remove_file(path).await {
        Ok(()) => {
            tracing::info!(path = %path.display(), "removed ownerless skill bundle");
            true
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), "could not remove ownerless skill bundle: {e}");
            false
        }
    }
}

/// Read what the registry references, then sweep. Any read failure leaves the
/// disk untouched: deleting on a partial view of the registry is the one
/// outcome worse than leaking a bundle.
pub async fn sweep_orphans_or_warn(pool: &sqlx::PgPool, store: &SkillStore) {
    let rows: Vec<(Uuid, String)> =
        match sqlx::query_as("SELECT uploaded_by_user, name FROM skill_registry")
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("skill bundle sweep skipped, registry read failed: {e}");
                return;
            }
        };
    let mut refs = SkillRefs::default();
    for (owner, name) in rows {
        refs.bundles.insert((owner, name.clone()));
        refs.names.insert(name);
    }
    if let Err(e) = store.sweep_orphans(&refs).await {
        tracing::warn!("skill bundle sweep failed: {e}");
    }
}

pub fn validate_name(s: &str) -> Result<(), SkillError> {
    if cctui_proto::util::is_valid_skill_name(s) { Ok(()) } else { Err(SkillError::InvalidName) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn write_roundtrip_hashes_and_renames() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let owner = Uuid::new_v4();
        let body: &[u8] = b"bundle-bytes";
        let stats = store.write(owner, "my-skill", body).await.unwrap();
        assert_eq!(stats.sha256, hex::encode(Sha256::digest(body)));
        assert_eq!(stats.size_bytes, body.len() as u64);
        assert!(store.path_of(owner, "my-skill").exists());
    }

    fn refs_for(rows: &[(Uuid, &str)]) -> SkillRefs {
        let mut refs = SkillRefs::default();
        for (owner, name) in rows {
            refs.bundles.insert((*owner, (*name).to_string()));
            refs.names.insert((*name).to_string());
        }
        refs
    }

    /// The migration-133 leftover: a root-level bundle whose name no row carries.
    #[tokio::test]
    async fn sweeps_ownerless_legacy_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        tokio::fs::write(store.legacy_path_of("dropped"), b"orphan").await.unwrap();

        assert_eq!(store.sweep_orphans(&SkillRefs::default()).await.unwrap(), 1);
        assert!(!store.legacy_path_of("dropped").exists());
        // Idempotent: a second pass finds nothing left to do.
        assert_eq!(store.sweep_orphans(&SkillRefs::default()).await.unwrap(), 0);
    }

    /// A legacy bundle whose name is still registered is adoptable, so the sweep
    /// must leave it for `adopt_legacy`.
    #[tokio::test]
    async fn keeps_a_legacy_bundle_whose_name_is_still_registered() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let owner = Uuid::new_v4();
        tokio::fs::write(store.legacy_path_of("shared"), b"adoptable").await.unwrap();

        assert_eq!(store.sweep_orphans(&refs_for(&[(owner, "shared")])).await.unwrap(), 0);
        assert!(store.legacy_path_of("shared").exists());

        let hash = hex::encode(Sha256::digest(b"adoptable"));
        assert!(store.adopt_legacy(owner, "shared", &hash).await.unwrap());
    }

    /// The invariant that matters: a referenced bundle is never removed, while an
    /// unreferenced one in the same directory is.
    #[tokio::test]
    async fn never_removes_a_referenced_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        store.write(a, "kept", b"keep".as_slice()).await.unwrap();
        store.write(a, "gone", b"drop".as_slice()).await.unwrap();
        store.write(b, "kept", b"other-owner".as_slice()).await.unwrap();

        let refs = refs_for(&[(a, "kept"), (b, "kept")]);
        assert_eq!(store.sweep_orphans(&refs).await.unwrap(), 1);
        assert_eq!(tokio::fs::read(store.path_of(a, "kept")).await.unwrap(), b"keep");
        assert_eq!(tokio::fs::read(store.path_of(b, "kept")).await.unwrap(), b"other-owner");
        assert!(!store.path_of(a, "gone").exists());
    }

    /// Same name, different owners: one row must not license the other's file.
    #[tokio::test]
    async fn owner_scoping_decides_not_just_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
        store.write(mine, "s", b"mine".as_slice()).await.unwrap();
        store.write(theirs, "s", b"theirs".as_slice()).await.unwrap();

        assert_eq!(store.sweep_orphans(&refs_for(&[(mine, "s")])).await.unwrap(), 1);
        assert!(store.path_of(mine, "s").exists());
        assert!(!store.path_of(theirs, "s").exists());
    }

    /// Everything that is not an owned `*.tar.zst` is left alone: in-flight
    /// uploads, foreign directories and unrelated files.
    #[tokio::test]
    async fn leaves_partials_and_unknown_entries_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let owner = Uuid::new_v4();
        store.write(owner, "live", b"v".as_slice()).await.unwrap();

        let partial = store.path_of(owner, "live").with_extension("zst.abc.partial");
        tokio::fs::write(&partial, b"in flight").await.unwrap();
        let readme = dir.path().join("README.md");
        tokio::fs::write(&readme, b"notes").await.unwrap();
        let foreign = dir.path().join("not-a-uuid");
        tokio::fs::create_dir_all(&foreign).await.unwrap();
        let nested = foreign.join("looks-like.tar.zst");
        tokio::fs::write(&nested, b"someone else's").await.unwrap();

        assert_eq!(store.sweep_orphans(&refs_for(&[(owner, "live")])).await.unwrap(), 0);
        for kept in [&partial, &readme, &nested] {
            assert!(kept.exists(), "{} was removed", kept.display());
        }
    }

    #[tokio::test]
    async fn sweep_on_a_missing_root_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().join("never-created"));
        assert_eq!(store.sweep_orphans(&SkillRefs::default()).await.unwrap(), 0);
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("ok").is_ok());
        assert!(validate_name("ok-name_1.2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name("a b").is_err());
        assert!(validate_name("a\0b").is_err());
    }

    #[tokio::test]
    async fn overwrites_existing_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let owner = Uuid::new_v4();
        store.write(owner, "s", b"v1".as_slice()).await.unwrap();
        let s2 = store.write(owner, "s", b"version-two".as_slice()).await.unwrap();
        assert_eq!(s2.size_bytes, 11);
        let read = tokio::fs::read(store.path_of(owner, "s")).await.unwrap();
        assert_eq!(read, b"version-two");
    }

    #[tokio::test]
    async fn owners_never_share_a_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        store.write(a, "s", b"alice".as_slice()).await.unwrap();
        store.write(b, "s", b"mallory".as_slice()).await.unwrap();
        assert_eq!(tokio::fs::read(store.path_of(a, "s")).await.unwrap(), b"alice");
        assert_eq!(tokio::fs::read(store.path_of(b, "s")).await.unwrap(), b"mallory");
    }

    #[tokio::test]
    async fn legacy_bundle_is_adopted_only_when_its_hash_matches() {
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let owner = Uuid::new_v4();
        tokio::fs::write(store.legacy_path_of("s"), b"old").await.unwrap();

        let wrong = hex::encode(Sha256::digest(b"other"));
        assert!(!store.adopt_legacy(owner, "s", &wrong).await.unwrap());
        assert!(store.legacy_path_of("s").exists());
        assert!(!store.path_of(owner, "s").exists());

        let right = hex::encode(Sha256::digest(b"old"));
        assert!(store.adopt_legacy(owner, "s", &right).await.unwrap());
        assert_eq!(tokio::fs::read(store.path_of(owner, "s")).await.unwrap(), b"old");
        assert!(!store.legacy_path_of("s").exists());
        assert!(!store.adopt_legacy(owner, "s", &right).await.unwrap());
    }
}
