//! Local mirror of a session's enabled plugin skills, laid out as Claude Code
//! plugins for `--plugin-dir`.
//!
//! Nothing is written into the user's repo. `<cache>/<id>/<skills_hash>/` holds `.claude-plugin/plugin.json` plus the
//! `skills/` tree fetched from the server's public `/plugins/<id>/skills/<file>`
//! route; a `.ready` marker makes a finished mirror reusable across launches.

use std::path::{Path, PathBuf};

use anyhow::Context;
use cctui_proto::api::SessionPlugin;

use crate::client::ServerClient;

const READY_MARKER: &str = ".ready";

/// `CCTUI_PLUGIN_CACHE_DIR`, else `$XDG_CONFIG_HOME/cctui/plugins`
/// (`~/.config/cctui/plugins`).
#[must_use]
pub fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CCTUI_PLUGIN_CACHE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cctui").join("plugins"))
}

/// A hash or id is safe to use as a folder name: plain `[a-z0-9-]` only.
fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A server-supplied skill file path: relative, no `..`/hidden segments.
fn safe_file(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path).components().all(|c| match c {
            std::path::Component::Normal(seg) => seg.to_str().is_some_and(|s| !s.starts_with('.')),
            _ => false,
        })
}

fn plugin_dir(root: &Path, plugin: &SessionPlugin) -> Option<PathBuf> {
    (safe_segment(&plugin.id) && safe_segment(&plugin.skills_hash))
        .then(|| root.join(&plugin.id).join(&plugin.skills_hash))
}

/// The manifest Claude Code expects at `.claude-plugin/plugin.json`.
fn claude_manifest(plugin: &SessionPlugin) -> String {
    serde_json::json!({
        "name": plugin.id,
        "version": plugin.version,
        "description": format!("cctui runtime plugin {}", plugin.id),
    })
    .to_string()
}

/// The URL a skill file is fetched from.
fn file_url(base_url: &str, plugin: &SessionPlugin, file: &str) -> String {
    format!("{}/plugins/{}/skills/{file}", base_url.trim_end_matches('/'), plugin.id)
}

async fn fetch_file(server: &ServerClient, url: &str) -> anyhow::Result<Vec<u8>> {
    let resp = server.http().get(url).send().await?;
    let status = resp.status();
    anyhow::ensure!(status.is_success(), "GET {url} -> {status}");
    Ok(resp.bytes().await?.to_vec())
}

/// Ensure `plugin` is mirrored under `root`; returns its plugin dir.
pub async fn mirror_plugin(
    server: &ServerClient,
    root: &Path,
    plugin: &SessionPlugin,
) -> anyhow::Result<PathBuf> {
    let dir = plugin_dir(root, plugin)
        .ok_or_else(|| anyhow::anyhow!("plugin `{}`: unsafe id or hash", plugin.id))?;
    if dir.join(READY_MARKER).is_file() {
        return Ok(dir);
    }
    let staging = root.join(&plugin.id).join(format!(".staging-{}", plugin.skills_hash));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(staging.join(".claude-plugin")).await?;
    tokio::fs::write(staging.join(".claude-plugin/plugin.json"), claude_manifest(plugin)).await?;
    for file in &plugin.files {
        anyhow::ensure!(safe_file(file), "plugin `{}`: unsafe skill path `{file}`", plugin.id);
        let bytes = fetch_file(server, &file_url(server.base_url(), plugin, file))
            .await
            .with_context(|| format!("plugin `{}`: skill file {file}", plugin.id))?;
        let target = staging.join("skills").join(file);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&target, bytes).await?;
    }
    tokio::fs::write(staging.join(READY_MARKER), plugin.skills_hash.as_bytes()).await?;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::rename(&staging, &dir).await?;
    Ok(dir)
}

/// Export every plugin's setting env into `env`.
///
/// Names are re-validated here (the server already did) and never override a
/// key the launch env holds, so a plugin cannot shadow gateway routing or
/// cctui's own vars.
pub fn export_env(env: &mut std::collections::BTreeMap<String, String>, plugins: &[SessionPlugin]) {
    for plugin in plugins {
        for (name, value) in &plugin.env {
            if value.is_empty() || !cctui_proto::worker_env::valid_plugin_env_name(name) {
                tracing::warn!(id = %plugin.id, %name, "plugin env var skipped");
                continue;
            }
            env.entry(name.clone()).or_insert_with(|| value.clone());
        }
    }
}

/// Mirror every plugin and return the `--plugin-dir` values, in order. A
/// plugin that fails to mirror is logged and skipped: a missing skill must
/// never block the launch.
pub async fn plugin_dirs(server: &ServerClient, plugins: &[SessionPlugin]) -> Vec<String> {
    if plugins.is_empty() {
        return Vec::new();
    }
    let Some(root) = cache_root() else {
        tracing::warn!("no plugin cache dir resolvable; skipping plugin skills");
        return Vec::new();
    };
    let mut out = Vec::new();
    for plugin in plugins {
        match mirror_plugin(server, &root, plugin).await {
            Ok(dir) => out.push(dir.to_string_lossy().into_owned()),
            Err(e) => tracing::warn!(id = %plugin.id, "plugin skills unavailable: {e:#}"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        cache_root, claude_manifest, export_env, file_url, mirror_plugin, plugin_dir, safe_file,
    };
    use crate::client::ServerClient;
    use cctui_proto::api::SessionPlugin;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn plugin(files: &[&str]) -> SessionPlugin {
        SessionPlugin {
            id: "yubisashi".into(),
            version: "0.3.0".into(),
            skills_hash: "abcdef0123456789".into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
            env: std::collections::BTreeMap::new(),
        }
    }

    /// A one-thread HTTP server that answers every `GET` under `/plugins/`
    /// with the request path as body, `404` elsewhere.
    fn serve(requests: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..requests {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, body) = if path.starts_with("/plugins/") {
                    ("200 OK", path.clone())
                } else {
                    ("404 Not Found", String::new())
                };
                let _ = write!(
                    sock,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn layout_and_safety() {
        let root = std::path::Path::new("/c");
        assert_eq!(
            plugin_dir(root, &plugin(&[])).unwrap(),
            root.join("yubisashi").join("abcdef0123456789")
        );
        let mut bad = plugin(&[]);
        bad.skills_hash = "../x".into();
        assert!(plugin_dir(root, &bad).is_none());
        assert!(safe_file("yubisashi/SKILL.md"));
        assert!(!safe_file("../SKILL.md"));
        assert!(!safe_file("/abs"));
        assert!(!safe_file(".hidden/SKILL.md"));
        assert_eq!(
            file_url("http://s/", &plugin(&[]), "yubisashi/SKILL.md"),
            "http://s/plugins/yubisashi/skills/yubisashi/SKILL.md"
        );
        let m: serde_json::Value = serde_json::from_str(&claude_manifest(&plugin(&[]))).unwrap();
        assert_eq!(m["name"], "yubisashi");
        assert_eq!(m["version"], "0.3.0");
    }

    #[test]
    fn export_env_adds_valid_names_without_overriding() {
        let mut p = plugin(&[]);
        p.env = std::collections::BTreeMap::from([
            ("YUBI_HOST".to_owned(), "10.0.0.5".to_owned()),
            ("YUBI_EMPTY".to_owned(), String::new()),
            ("PATH".to_owned(), "/evil".to_owned()),
            ("ANTHROPIC_BASE_URL".to_owned(), "http://evil".to_owned()),
            ("CCTUI_WEB_ORIGIN".to_owned(), "http://evil".to_owned()),
            ("lower".to_owned(), "x".to_owned()),
            ("PRESET".to_owned(), "plugin".to_owned()),
        ]);
        let mut env = std::collections::BTreeMap::from([
            ("PRESET".to_owned(), "launch".to_owned()),
            ("ANTHROPIC_BASE_URL".to_owned(), "http://gateway".to_owned()),
        ]);
        export_env(&mut env, &[p]);
        assert_eq!(env["YUBI_HOST"], "10.0.0.5");
        assert_eq!(env["PRESET"], "launch");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://gateway");
        assert_eq!(env.len(), 3, "{env:?}");
    }

    #[test]
    fn cache_root_resolves() {
        assert!(cache_root().is_some());
    }

    #[tokio::test]
    async fn mirrors_once_and_reuses_the_ready_dir() {
        let base = serve(2);
        let server = ServerClient::new(base);
        let root = tempfile::tempdir().unwrap();
        let p = plugin(&["yubisashi/SKILL.md", "yubisashi/ref/notes.md"]);

        let dir = mirror_plugin(&server, root.path(), &p).await.unwrap();
        assert_eq!(dir, root.path().join("yubisashi/abcdef0123456789"));
        assert!(dir.join(".claude-plugin/plugin.json").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.join("skills/yubisashi/SKILL.md")).unwrap(),
            "/plugins/yubisashi/skills/yubisashi/SKILL.md"
        );
        assert!(dir.join("skills/yubisashi/ref/notes.md").is_file());
        assert!(dir.join(".ready").is_file());

        let again = mirror_plugin(&server, root.path(), &p).await.unwrap();
        assert_eq!(again, dir);
    }

    #[tokio::test]
    async fn unsafe_paths_are_refused_and_leave_no_ready_dir() {
        let server = ServerClient::new("http://127.0.0.1:9");
        let root = tempfile::tempdir().unwrap();
        let err = mirror_plugin(&server, root.path(), &plugin(&["../escape"])).await.unwrap_err();
        assert!(err.to_string().contains("unsafe skill path"), "{err:#}");
        assert!(!root.path().join("yubisashi/abcdef0123456789").exists());

        let err = mirror_plugin(&server, root.path(), &plugin(&["yubisashi/SKILL.md"])).await;
        assert!(err.is_err(), "unreachable server -> error");
        assert!(!root.path().join("yubisashi/abcdef0123456789").exists());
    }
}
