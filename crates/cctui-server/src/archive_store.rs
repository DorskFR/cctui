use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("invalid name")]
    InvalidName,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub sha256: String,
    pub size_bytes: u64,
    pub line_count: u32,
}

#[derive(Debug, Clone)]
pub struct ArchiveStore {
    root: PathBuf,
}

impl ArchiveStore {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn ensure_root(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root).await
    }

    #[must_use]
    pub fn path_of(&self, machine_id: Uuid, project_dir: &str, session_id: &str) -> PathBuf {
        self.root
            .join(machine_id.to_string())
            .join("projects")
            .join(project_dir)
            .join(format!("{session_id}.jsonl"))
    }

    pub async fn write<R: AsyncRead + Unpin>(
        &self,
        machine_id: Uuid,
        project_dir: &str,
        session_id: &str,
        mut body: R,
    ) -> Result<Stats, ArchiveError> {
        validate_name(project_dir)?;
        validate_name(session_id)?;

        let final_path = self.path_of(machine_id, project_dir, session_id);
        let partial_path = final_path.with_extension("jsonl.partial");
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let outcome = async {
            let mut file = fs::File::create(&partial_path).await?;
            let mut hasher = Sha256::new();
            let mut size: u64 = 0;
            let mut line_count: u32 = 0;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = body.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                let chunk = &buf[..n];
                hasher.update(chunk);
                size += n as u64;
                #[allow(clippy::naive_bytecount)]
                let nl = chunk.iter().filter(|&&b| b == b'\n').count();
                line_count = line_count.saturating_add(u32::try_from(nl).unwrap_or(u32::MAX));
                file.write_all(chunk).await?;
            }
            file.flush().await?;
            drop(file);
            Ok::<_, std::io::Error>(Stats {
                sha256: hex::encode(hasher.finalize()),
                size_bytes: size,
                line_count,
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

fn validate_name(s: &str) -> Result<(), ArchiveError> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.contains('/')
        || s.contains('\\')
        || s.contains('\0')
    {
        return Err(ArchiveError::InvalidName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn write_roundtrip_hashes_and_renames() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        let body: &[u8] = b"{\"a\":1}\n{\"b\":2}\n";

        let stats =
            store.write(machine, "-home-user-proj", "abc-123", body).await.expect("write ok");

        let expected = hex::encode(Sha256::digest(body));
        assert_eq!(stats.sha256, expected);
        assert_eq!(stats.size_bytes, body.len() as u64);
        assert_eq!(stats.line_count, 2);
        let path = store.path_of(machine, "-home-user-proj", "abc-123");
        assert!(path.exists());
        assert!(!path.with_extension("jsonl.partial").exists());
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        let err = store.write(machine, "..", "abc", b"".as_slice()).await.unwrap_err();
        assert!(matches!(err, ArchiveError::InvalidName));
    }

    #[tokio::test]
    async fn rejects_slashes_and_null_in_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        assert!(store.write(machine, "a/b", "abc", b"".as_slice()).await.is_err());
        assert!(store.write(machine, "ok", "a/b", b"".as_slice()).await.is_err());
        assert!(store.write(machine, "ok", "a\0b", b"".as_slice()).await.is_err());
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        store.write(machine, "-home-x", "s", b"first\n".as_slice()).await.unwrap();
        let s2 = store.write(machine, "-home-x", "s", b"second\nthird\n".as_slice()).await.unwrap();
        assert_eq!(s2.line_count, 2);
        let read = tokio::fs::read(store.path_of(machine, "-home-x", "s")).await.unwrap();
        assert_eq!(read, b"second\nthird\n");
    }
}
