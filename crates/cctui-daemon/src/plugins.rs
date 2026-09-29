//! Local mirror of a session's enabled plugin skills, laid out as Claude Code
//! plugins for `--plugin-dir`.
//!
//! Nothing is written into the user's repo. `<cache>/<id>/<skills_hash>/` holds `.claude-plugin/plugin.json` plus the
//! `skills/` tree fetched from the server's public `/plugins/<id>/skills/<file>`
//! route; a `.ready` marker makes a finished mirror reusable across launches.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use cctui_proto::api::SessionPlugin;

use crate::client::ServerClient;

const READY_MARKER: &str = ".ready";

/// Codex budgets its own skill list at a small fraction of the context, so a
/// long description is cut rather than allowed to crowd the prompt.
const MAX_DESCRIPTION: usize = 300;

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
    let html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    anyhow::ensure!(!html, "GET {url} returned HTML: /plugins is not routed to the server");
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

/// A mirrored skill as an agent without directory-based skill discovery needs
/// it: the frontmatter identity plus the absolute `SKILL.md` to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroredSkill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// The YAML frontmatter block of a `SKILL.md`, without its `---` fences.
fn frontmatter(text: &str) -> Option<&str> {
    let rest = text.trim_start_matches('\u{feff}').strip_prefix("---")?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

fn frontmatter_field(text: &str, field: &str) -> Option<String> {
    frontmatter(text)?.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == field).then(|| unquote(value))
    })
}

fn truncated(mut value: String) -> String {
    if value.chars().count() > MAX_DESCRIPTION {
        let cut = value.char_indices().nth(MAX_DESCRIPTION).map_or(value.len(), |(i, _)| i);
        value.truncate(cut);
        value.push('…');
    }
    value
}

/// Read one mirrored `skills/<folder>/SKILL.md`. Anything that is not a
/// top-level `SKILL.md` is reference material the skill itself pulls in.
async fn mirrored_skill(dir: &Path, file: &str) -> Option<MirroredSkill> {
    let rel = Path::new(file);
    if rel.file_name().and_then(std::ffi::OsStr::to_str) != Some("SKILL.md") {
        return None;
    }
    let folder = rel.parent()?.file_name()?.to_str()?.to_owned();
    let path = dir.join("skills").join(rel);
    let text = tokio::fs::read_to_string(&path).await.ok()?;
    let name = frontmatter_field(&text, "name").filter(|n| !n.is_empty()).unwrap_or(folder);
    let description = truncated(frontmatter_field(&text, "description").unwrap_or_default());
    Some(MirroredSkill { name, description, path })
}

/// The skill catalog for an agent that cannot be pointed at a skills
/// directory: name, description and the `SKILL.md` to read, the same
/// progressive disclosure a native skill list gives.
#[must_use]
pub fn codex_catalog(skills: &[MirroredSkill]) -> Option<String> {
    use std::fmt::Write as _;

    if skills.is_empty() {
        return None;
    }
    let mut out = String::from(
        "<cctui_skills>\nThe following skills are available to this session. When a task matches \
         a description, read the SKILL.md at the path before acting and follow it.\n",
    );
    for skill in skills {
        let _ = writeln!(
            out,
            "- {}: {} (path: {})",
            skill.name,
            skill.description,
            skill.path.display()
        );
    }
    out.push_str("</cctui_skills>");
    Some(out)
}

/// What one session's enabled plugins contribute to its agent, resolved once
/// at launch: `env` for every adapter, `roots` for an agent that discovers
/// skills from directories, `catalog` for one that does not.
#[derive(Debug, Default, Clone)]
pub struct SessionSkills {
    pub env: BTreeMap<String, String>,
    pub roots: Vec<PathBuf>,
    pub catalog: Option<String>,
}

impl SessionSkills {
    #[must_use]
    pub const fn none() -> Self {
        Self { env: BTreeMap::new(), roots: Vec::new(), catalog: None }
    }
}

/// Mirror `plugins` and build the per-session skill/env contribution.
///
/// Never fails: a plugin whose skills cannot be fetched is logged and skipped,
/// so the env (`CCTUI_SESSION_ID`, `CCTUI_WEB_ORIGIN`) still reaches the agent.
pub async fn resolve_session_skills(
    server: Option<&ServerClient>,
    session_id: &str,
    plugins: &[SessionPlugin],
) -> SessionSkills {
    let mut out = SessionSkills::default();
    if let Some(server) = server {
        crate::childenv::with_web_origin(&mut out.env, server.base_url());
    }
    crate::childenv::with_session_id(&mut out.env, session_id);
    export_env(&mut out.env, plugins);
    if plugins.is_empty() {
        return out;
    }
    let (Some(server), Some(root)) = (server, cache_root()) else {
        tracing::warn!("no server or plugin cache dir; skipping plugin skills");
        return out;
    };
    mirror_into(&mut out, server, &root, plugins).await;
    out
}

async fn mirror_into(
    out: &mut SessionSkills,
    server: &ServerClient,
    root: &Path,
    plugins: &[SessionPlugin],
) {
    let mut skills = Vec::new();
    for plugin in plugins {
        match mirror_plugin(server, root, plugin).await {
            Ok(dir) => {
                out.roots.push(dir.join("skills"));
                for file in &plugin.files {
                    if let Some(skill) = mirrored_skill(&dir, file).await {
                        skills.push(skill);
                    }
                }
            }
            Err(e) => tracing::warn!(id = %plugin.id, "plugin skills unavailable: {e:#}"),
        }
    }
    out.catalog = codex_catalog(&skills);
}

static BY_SESSION: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, SessionSkills>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Remember what a launch resolved, keyed by the id the agent is really known
/// by. A hibernated session resumes without a fresh gateway-env pull, and the
/// skills are not persisted anywhere, so this is all a resume has.
pub fn remember_skills(session_id: &str, skills: &SessionSkills) {
    if let Ok(mut map) = BY_SESSION.lock() {
        map.insert(session_id.to_owned(), skills.clone());
    }
}

#[must_use]
pub fn recall_skills(session_id: &str) -> Option<SessionSkills> {
    BY_SESSION.lock().ok().and_then(|map| map.get(session_id).cloned())
}

#[cfg(test)]
mod tests {
    use super::{
        MirroredSkill, SessionSkills, cache_root, claude_manifest, codex_catalog, export_env,
        file_url, frontmatter_field, mirror_into, mirror_plugin, mirrored_skill, plugin_dir,
        recall_skills, remember_skills, resolve_session_skills, safe_file, truncated,
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
    async fn an_html_answer_is_refused_and_leaves_no_ready_dir() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let _ = sock.read(&mut [0u8; 4096]).unwrap();
            let body = "<!doctype html>";
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        let server = ServerClient::new(format!("http://{addr}"));
        let root = tempfile::tempdir().unwrap();
        let p = plugin(&["yubisashi/SKILL.md"]);
        let err = mirror_plugin(&server, root.path(), &p).await.unwrap_err();
        assert!(format!("{err:#}").contains("HTML"), "{err:#}");
        assert!(!plugin_dir(root.path(), &p).unwrap().exists());
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

    #[test]
    fn frontmatter_fields_are_read_quoted_or_bare() {
        let doc = "---\nname: yubisashi\ndescription: \"Use when the user wants to review\"\n---\n\n# Body\ndescription: not frontmatter\n";
        assert_eq!(frontmatter_field(doc, "name").as_deref(), Some("yubisashi"));
        assert_eq!(
            frontmatter_field(doc, "description").as_deref(),
            Some("Use when the user wants to review")
        );
        assert_eq!(frontmatter_field(doc, "license"), None);
        assert_eq!(frontmatter_field("# no frontmatter\nname: x\n", "name"), None);
        assert_eq!(
            frontmatter_field("---\r\nname: 'single'\r\n---\r\n", "name").as_deref(),
            Some("single")
        );
    }

    #[test]
    fn a_long_description_is_cut_on_a_char_boundary() {
        let short = "é".repeat(10);
        assert_eq!(truncated(short.clone()), short);
        let long = "é".repeat(400);
        let cut = truncated(long);
        assert_eq!(cut.chars().count(), 301, "300 chars plus the ellipsis");
        assert!(cut.ends_with('…'));
    }

    #[tokio::test]
    async fn a_mirrored_skill_reports_its_frontmatter_identity_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills/yubisashi");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("SKILL.md"),
            "---\nname: yubisashi\ndescription: Point at the UI\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(skills.join("notes.md"), "reference").unwrap();

        let skill = mirrored_skill(dir.path(), "yubisashi/SKILL.md").await.expect("skill");
        assert_eq!(skill.name, "yubisashi");
        assert_eq!(skill.description, "Point at the UI");
        assert_eq!(skill.path, skills.join("SKILL.md"));

        assert!(
            mirrored_skill(dir.path(), "yubisashi/notes.md").await.is_none(),
            "reference material is not a skill"
        );
        assert!(mirrored_skill(dir.path(), "SKILL.md").await.is_none(), "no folder, no skill");
        assert!(mirrored_skill(dir.path(), "absent/SKILL.md").await.is_none());
    }

    #[tokio::test]
    async fn a_skill_without_frontmatter_falls_back_to_its_folder_name() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills/yubisashi");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "no frontmatter here").unwrap();
        let skill = mirrored_skill(dir.path(), "yubisashi/SKILL.md").await.expect("skill");
        assert_eq!(skill.name, "yubisashi");
        assert_eq!(skill.description, "");
    }

    #[test]
    fn the_catalog_lists_one_line_per_skill_and_is_empty_for_none() {
        assert_eq!(codex_catalog(&[]), None);
        let catalog = codex_catalog(&[MirroredSkill {
            name: "yubisashi".to_owned(),
            description: "Point at the UI".to_owned(),
            path: std::path::PathBuf::from("/c/yubisashi/h/skills/yubisashi/SKILL.md"),
        }])
        .expect("catalog");
        assert!(catalog.starts_with("<cctui_skills>"));
        assert!(catalog.ends_with("</cctui_skills>"));
        assert!(catalog.contains(
            "- yubisashi: Point at the UI (path: /c/yubisashi/h/skills/yubisashi/SKILL.md)"
        ));
    }

    #[tokio::test]
    async fn session_skills_carry_the_env_the_roots_and_the_catalog() {
        let base = serve(1);
        let server = ServerClient::new(base.clone());
        let cache = tempfile::tempdir().unwrap();

        let mut p = plugin(&["yubisashi/SKILL.md"]);
        p.env = std::collections::BTreeMap::from([("YUBI_HOST".to_owned(), "10.0.0.5".to_owned())]);
        let mut skills = resolve_session_skills(Some(&server), "sess-1", &[]).await;
        mirror_into(&mut skills, &server, cache.path(), &[p.clone()]).await;
        export_env(&mut skills.env, &[p]);

        assert_eq!(
            skills.env.get(crate::preview::SESSION_ID_VAR).map(String::as_str),
            Some("sess-1")
        );
        assert_eq!(
            skills.env.get(crate::childenv::WEB_ORIGIN_VAR).map(String::as_str),
            Some(base.as_str())
        );
        assert_eq!(skills.env.get("YUBI_HOST").map(String::as_str), Some("10.0.0.5"));
        assert_eq!(skills.roots, vec![cache.path().join("yubisashi/abcdef0123456789/skills")]);
        let catalog = skills.catalog.expect("a mirrored skill yields a catalog");
        assert!(catalog.contains("yubisashi"));
    }

    #[tokio::test]
    async fn a_session_without_plugins_still_learns_its_own_id() {
        let skills = resolve_session_skills(None, "sess-2", &[]).await;
        assert_eq!(
            skills.env.get(crate::preview::SESSION_ID_VAR).map(String::as_str),
            Some("sess-2")
        );
        assert!(skills.roots.is_empty());
        assert_eq!(skills.catalog, None);
    }

    #[test]
    fn remembered_skills_are_what_a_resume_recalls() {
        assert_eq!(recall_skills("thread_0199absent"), None);
        let skills = SessionSkills {
            env: std::collections::BTreeMap::from([(
                "CCTUI_SESSION_ID".to_owned(),
                "launch-key-1".to_owned(),
            )]),
            roots: vec![std::path::PathBuf::from("/c/p/h/skills")],
            catalog: Some("<cctui_skills>x</cctui_skills>".to_owned()),
        };
        remember_skills("thread_0199remember", &skills);
        let got = recall_skills("thread_0199remember").expect("remembered");
        assert_eq!(got.env, skills.env);
        assert_eq!(got.roots, skills.roots);
        assert_eq!(got.catalog, skills.catalog);
    }
}
