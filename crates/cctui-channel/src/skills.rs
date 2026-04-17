//! Skill sync: diff local `.cctui-version` against server index, pull changed
//! bundles, extract with `tar --zstd`. Port of `channel/src/skills.ts`.

use std::path::{Path, PathBuf};
use std::process::Command;

use cctui_proto::api::SkillIndexEntry;

use crate::bridge::Bridge;

const VERSION_FILE: &str = ".cctui-version";

fn skills_root() -> PathBuf {
    if let Ok(dir) = std::env::var("CCTUI_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".claude").join("skills")
}

fn read_local_version(root: &Path, name: &str) -> Option<String> {
    let p = root.join(name).join(VERSION_FILE);
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn write_local_version(root: &Path, name: &str, sha: &str) {
    let p = root.join(name).join(VERSION_FILE);
    if let Err(err) = std::fs::write(&p, format!("{sha}\n")) {
        tracing::error!(path = %p.display(), error = %err, "write {VERSION_FILE} failed");
    }
}

pub async fn sync(bridge: &Bridge) {
    let root = skills_root();
    if let Err(err) = std::fs::create_dir_all(&root) {
        tracing::error!(path = %root.display(), error = %err, "mkdir skills root failed");
        return;
    }
    let index = match bridge.get_skill_index().await {
        Ok(i) => i,
        Err(err) => {
            tracing::error!(error = %err, "skill index fetch failed");
            return;
        }
    };
    for entry in index {
        let local = read_local_version(&root, &entry.name);
        if local.as_deref() == Some(entry.sha256.as_str()) {
            continue;
        }
        match pull_one(bridge, &root, &entry).await {
            Ok(()) => {
                write_local_version(&root, &entry.name, &entry.sha256);
                tracing::info!(
                    name = %entry.name,
                    sha = %&entry.sha256[..entry.sha256.len().min(12)],
                    "skill synced"
                );
            }
            Err(err) => {
                tracing::error!(name = %entry.name, error = %err, "skill pull failed");
            }
        }
    }
}

async fn pull_one(bridge: &Bridge, root: &Path, entry: &SkillIndexEntry) -> anyhow::Result<()> {
    let bytes = bridge.get_skill_bundle(&entry.name).await?;
    let pid = std::process::id();
    let now = chrono::Utc::now().timestamp_millis();
    let tmp = root.join(format!(".cctui-skill-{}-{pid}-{now}.tar.zst", entry.name));
    std::fs::write(&tmp, &bytes)?;

    let dest = root.join(&entry.name);
    let backup = if dest.exists() {
        Some(root.join(format!("{}.cctui-old-{pid}-{now}", entry.name)))
    } else {
        None
    };

    let result = (|| -> anyhow::Result<()> {
        if let Some(b) = &backup {
            std::fs::rename(&dest, b)?;
        }
        let status =
            Command::new("tar").arg("--zstd").arg("-C").arg(root).arg("-xf").arg(&tmp).status()?;
        if !status.success() {
            anyhow::bail!("tar --zstd -x exited {status}");
        }
        if let Some(b) = &backup {
            let _ = std::fs::remove_dir_all(b);
        }
        Ok(())
    })();

    if let Err(err) = &result {
        if let Some(b) = &backup
            && b.exists()
        {
            let _ = std::fs::remove_dir_all(&dest);
            let _ = std::fs::rename(b, &dest);
        }
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!("{err}"));
    }

    let _ = std::fs::remove_file(&tmp);
    Ok(())
}
