use std::path::PathBuf;

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
        self.root.join(owner.to_string()).join(format!("{name}.tar.zst"))
    }

    fn legacy_path_of(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.tar.zst"))
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
