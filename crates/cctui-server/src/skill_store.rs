use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

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

    /// Path to the active bundle for a skill. One bundle per name;
    /// overwritten on each upload (last-write-wins per ticket scope).
    #[must_use]
    pub fn path_of(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.tar.zst"))
    }

    pub async fn write<R: AsyncRead + Unpin>(
        &self,
        name: &str,
        mut body: R,
    ) -> Result<SkillStats, SkillError> {
        validate_name(name)?;

        let final_path = self.path_of(name);
        let partial_path = final_path.with_extension("zst.partial");
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
        let body: &[u8] = b"bundle-bytes";
        let stats = store.write("my-skill", body).await.unwrap();
        assert_eq!(stats.sha256, hex::encode(Sha256::digest(body)));
        assert_eq!(stats.size_bytes, body.len() as u64);
        assert!(store.path_of("my-skill").exists());
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
        store.write("s", b"v1".as_slice()).await.unwrap();
        let s2 = store.write("s", b"version-two".as_slice()).await.unwrap();
        assert_eq!(s2.size_bytes, 11);
        let read = tokio::fs::read(store.path_of("s")).await.unwrap();
        assert_eq!(read, b"version-two");
    }
}
