//! Runtime plugin registry. A plugin holds a `plugin.json` manifest, an
//! optional `web/` ES module the webui imports, and optional `skills/<name>/`
//! folders handed to agent sessions. Plugins come from two sources: folders
//! under `CCTUI_PLUGINS_DIR` (read-only, always instance-enabled) and archives
//! an admin installed, kept in the blob store and extracted into memory.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The major of the host/plugin contract this server speaks.
pub const CCTUI_API: u32 = 1;
const MAX_ID_LEN: usize = 40;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    pub cctui_api: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    /// Per-user settings the plugin asks for; values are exported into agent
    /// sessions as `env`.
    #[serde(default)]
    pub settings: Vec<PluginSetting>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetting {
    pub key: String,
    pub label: String,
    pub env: String,
    #[serde(rename = "type")]
    pub kind: String,
}

pub const MAX_SETTING_VALUE_CHARS: usize = 512;
const MAX_SETTINGS: usize = 32;

fn valid_setting_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && key.len() <= 40
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum PluginSource {
    Directory,
    Installed,
}

/// Where a plugin's files live: a folder on disk, or the extracted archive.
#[derive(Debug, Clone)]
pub enum PluginFiles {
    Dir(PathBuf),
    Memory(Arc<BTreeMap<String, Vec<u8>>>),
}

impl PluginFiles {
    fn exists(&self, rel: &str) -> bool {
        match self {
            Self::Dir(dir) => dir.join(rel).is_file(),
            Self::Memory(files) => files.contains_key(rel),
        }
    }

    /// Bytes of the safe relative `rel`, `None` when absent or outside the plugin.
    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        if !safe_relative(rel) {
            return None;
        }
        match self {
            Self::Dir(dir) => std::fs::read(resolve_in_dir(dir, rel)?).ok(),
            Self::Memory(files) => files.get(rel).cloned(),
        }
    }

    fn list(&self, rel: &Path) -> Vec<String> {
        let mut out = Vec::new();
        match self {
            Self::Dir(dir) => collect_files(&dir.join("skills"), rel, &mut out),
            Self::Memory(files) => {
                let prefix = format!("skills/{}/", rel.to_string_lossy());
                out.extend(
                    files
                        .keys()
                        .filter_map(|k| k.strip_prefix(&prefix))
                        .map(|k| format!("{}/{k}", rel.to_string_lossy())),
                );
            }
        }
        out
    }
}

/// One installed plugin, validated at load time.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub files: PluginFiles,
    pub source: PluginSource,
    /// Directory plugins are always on; installed ones follow the admin toggle.
    pub instance_enabled: bool,
    /// Short content hash of the `web` entry, for the `?v=` cache-buster.
    pub web_hash: Option<String>,
    /// Files under `skills/`, relative to that folder, in a stable order.
    pub skill_files: Vec<String>,
    /// Hash over every skill file's path + content; changes when any does.
    pub skills_hash: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("plugin id `{0}` must match [a-z0-9-]{{1,40}}")]
    BadId(String),
    #[error("plugin id `{id}` does not match its folder `{folder}`")]
    IdFolderMismatch { id: String, folder: String },
    #[error("plugin `{0}` targets cctuiApi {1}, this server speaks {CCTUI_API}")]
    UnsupportedApi(String, u32),
    #[error("plugin `{0}`: `{1}` is empty")]
    EmptyField(String, &'static str),
    #[error("plugin `{0}`: path `{1}` must be relative and stay inside the plugin folder")]
    BadPath(String, String),
    #[error("plugin `{0}`: `{1}` does not exist")]
    Missing(String, String),
    #[error("plugin `{0}`: setting `{1}` is invalid: {2}")]
    BadSetting(String, String, &'static str),
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A relative path with plain components only: no root, no `..`, no `.`,
/// no hidden segments, no backslashes.
pub fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') || path.contains('\0') {
        return false;
    }
    Path::new(path).components().all(|c| match c {
        Component::Normal(seg) => seg.to_str().is_some_and(|s| !s.starts_with('.')),
        _ => false,
    })
}

/// Validate `manifest` as the content of `<folder>/plugin.json`; `exists`
/// answers whether a relative path is a file of the plugin.
pub fn validate_manifest(
    manifest: &PluginManifest,
    folder: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Result<(), ManifestError> {
    let id = manifest.id.clone();
    if !valid_id(&id) {
        return Err(ManifestError::BadId(id));
    }
    if id != folder {
        return Err(ManifestError::IdFolderMismatch { id, folder: folder.to_owned() });
    }
    if manifest.cctui_api != CCTUI_API {
        return Err(ManifestError::UnsupportedApi(id, manifest.cctui_api));
    }
    if manifest.name.trim().is_empty() {
        return Err(ManifestError::EmptyField(id, "name"));
    }
    if manifest.version.trim().is_empty() {
        return Err(ManifestError::EmptyField(id, "version"));
    }
    if let Some(web) = &manifest.web {
        if !safe_relative(web) {
            return Err(ManifestError::BadPath(id, web.clone()));
        }
        if !exists(web) {
            return Err(ManifestError::Missing(id, web.clone()));
        }
    }
    for skill in &manifest.skills {
        if !safe_relative(skill) || skill.contains('/') {
            return Err(ManifestError::BadPath(id, skill.clone()));
        }
        let skill_md = format!("skills/{skill}/SKILL.md");
        if !exists(&skill_md) {
            return Err(ManifestError::Missing(id, skill_md));
        }
    }
    if manifest.settings.len() > MAX_SETTINGS {
        return Err(ManifestError::BadSetting(id, String::new(), "too many settings"));
    }
    let mut keys = std::collections::HashSet::new();
    let mut envs = std::collections::HashSet::new();
    for setting in &manifest.settings {
        let bad = |why| ManifestError::BadSetting(id.clone(), setting.key.clone(), why);
        if !valid_setting_key(&setting.key) {
            return Err(bad("key must match [a-zA-Z][a-zA-Z0-9_-]{0,39}"));
        }
        if setting.label.trim().is_empty() {
            return Err(bad("label is empty"));
        }
        if setting.kind != "string" {
            return Err(bad("type must be \"string\""));
        }
        if !cctui_proto::worker_env::valid_plugin_env_name(&setting.env) {
            return Err(bad("env must match ^[A-Z][A-Z0-9_]{0,63}$ and not be reserved"));
        }
        if !keys.insert(setting.key.as_str()) || !envs.insert(setting.env.as_str()) {
            return Err(bad("duplicate key or env"));
        }
    }
    Ok(())
}

fn short_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..8].to_owned()
}

fn collect_files(root: &Path, rel: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let child = rel.join(name);
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            collect_files(root, &child, out);
        } else if kind.is_file() {
            out.push(child.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Load and validate `<dir>/plugin.json` for the plugin in `dir`.
pub fn load_plugin(dir: &Path) -> anyhow::Result<Plugin> {
    let folder = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("unreadable folder name {}", dir.display()))?;
    let manifest_path = dir.join("plugin.json");
    let meta = std::fs::metadata(&manifest_path)?;
    anyhow::ensure!(meta.len() <= MAX_MANIFEST_BYTES, "plugin.json larger than 64 KiB");
    let raw = std::fs::read(&manifest_path)?;
    build_plugin(&raw, folder, PluginFiles::Dir(dir.to_path_buf()), PluginSource::Directory)
}

/// Build a plugin from extracted archive files keyed by relative path; the
/// manifest is `plugin.json` and `folder` is the archive's top-level folder
/// (or the manifest id when the archive has none).
pub fn load_from_memory(
    files: BTreeMap<String, Vec<u8>>,
    folder: Option<&str>,
    enabled: bool,
) -> anyhow::Result<Plugin> {
    let raw = files.get("plugin.json").ok_or_else(|| anyhow::anyhow!("plugin.json missing"))?;
    anyhow::ensure!(raw.len() as u64 <= MAX_MANIFEST_BYTES, "plugin.json larger than 64 KiB");
    let id = serde_json::from_slice::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("id")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let folder = folder.map_or(id, str::to_owned);
    let raw = raw.clone();
    let mut plugin =
        build_plugin(&raw, &folder, PluginFiles::Memory(Arc::new(files)), PluginSource::Installed)?;
    plugin.instance_enabled = enabled;
    Ok(plugin)
}

fn build_plugin(
    raw: &[u8],
    folder: &str,
    files: PluginFiles,
    source: PluginSource,
) -> anyhow::Result<Plugin> {
    let manifest: PluginManifest = serde_json::from_slice(raw)?;
    validate_manifest(&manifest, folder, &|rel| files.exists(rel))?;
    let web_hash = match &manifest.web {
        Some(web) => {
            Some(short_hash(&files.read(web).ok_or_else(|| anyhow::anyhow!("`{web}` unreadable"))?))
        }
        None => None,
    };
    let mut skill_files = Vec::new();
    for skill in &manifest.skills {
        skill_files.extend(files.list(Path::new(skill)));
    }
    let mut hasher = Sha256::new();
    for file in &skill_files {
        hasher.update(file.as_bytes());
        hasher.update([0]);
        hasher.update(
            files
                .read(&format!("skills/{file}"))
                .ok_or_else(|| anyhow::anyhow!("skill file `{file}` unreadable"))?,
        );
        hasher.update([0]);
    }
    let skills_hash = hex::encode(hasher.finalize())[..16].to_owned();
    Ok(Plugin {
        manifest,
        files,
        source,
        instance_enabled: true,
        web_hash,
        skill_files,
        skills_hash,
    })
}

/// Scan every child folder of `root`; invalid plugins are logged and skipped.
pub fn scan_dir(root: &Path) -> BTreeMap<String, Plugin> {
    let mut out = BTreeMap::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(dir = %root.display(), "plugins dir unreadable: {e}");
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("plugin.json").is_file() {
            continue;
        }
        match load_plugin(&path) {
            Ok(plugin) => {
                tracing::info!(
                    id = %plugin.manifest.id,
                    version = %plugin.manifest.version,
                    "plugin loaded"
                );
                out.insert(plugin.manifest.id.clone(), plugin);
            }
            Err(e) => tracing::warn!(dir = %path.display(), "plugin skipped: {e:#}"),
        }
    }
    out
}

/// Directory plugins plus admin-installed ones. An installed plugin shadows a
/// directory plugin with the same id. `dir == None` means no directory source.
#[derive(Debug)]
pub struct PluginRegistry {
    dir: Option<PathBuf>,
    dir_plugins: RwLock<BTreeMap<String, Plugin>>,
    installed: RwLock<BTreeMap<String, Plugin>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::disabled()
    }
}

impl PluginRegistry {
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            dir: None,
            dir_plugins: RwLock::new(BTreeMap::new()),
            installed: RwLock::new(BTreeMap::new()),
        }
    }

    /// Scan `dir` now and keep it for rescans.
    #[must_use]
    pub fn from_dir(dir: PathBuf) -> Self {
        let plugins = scan_dir(&dir);
        Self {
            dir: Some(dir),
            dir_plugins: RwLock::new(plugins),
            installed: RwLock::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Re-read the plugins dir; returns how many directory plugins there are.
    pub fn rescan(&self) -> usize {
        let Some(dir) = &self.dir else { return 0 };
        let fresh = scan_dir(dir);
        let n = fresh.len();
        *self.dir_plugins.write().unwrap_or_else(std::sync::PoisonError::into_inner) = fresh;
        n
    }

    /// Replace every installed plugin (startup load).
    pub fn set_installed(&self, plugins: BTreeMap<String, Plugin>) {
        *self.installed.write().unwrap_or_else(std::sync::PoisonError::into_inner) = plugins;
    }

    pub fn upsert_installed(&self, plugin: Plugin) {
        self.installed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(plugin.manifest.id.clone(), plugin);
    }

    pub fn remove_installed(&self, id: &str) -> bool {
        self.installed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id)
            .is_some()
    }

    /// Flip the instance toggle of an installed plugin; `false` when unknown.
    pub fn set_installed_enabled(&self, id: &str, enabled: bool) -> bool {
        self.installed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(id)
            .map(|plugin| plugin.instance_enabled = enabled)
            .is_some()
    }

    fn merged(&self) -> BTreeMap<String, Plugin> {
        let mut out =
            self.dir_plugins.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        for (id, plugin) in
            self.installed.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter()
        {
            out.insert(id.clone(), plugin.clone());
        }
        out
    }

    /// The instance-enabled plugin `id`, what users and sessions may use.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Plugin> {
        self.merged().remove(id).filter(|p| p.instance_enabled)
    }

    /// Every instance-enabled plugin.
    #[must_use]
    pub fn all(&self) -> Vec<Plugin> {
        self.merged().into_values().filter(|p| p.instance_enabled).collect()
    }

    /// Every plugin regardless of the instance toggle (admin listing).
    #[must_use]
    pub fn all_admin(&self) -> Vec<Plugin> {
        self.merged().into_values().collect()
    }
}

/// Which plugin ids a settings blob enables (`plugins.enabled[id] == true`).
pub fn enabled_ids(settings: Option<&serde_json::Value>) -> Vec<String> {
    settings
        .and_then(|v| v.get("plugins"))
        .and_then(|p| p.get("enabled"))
        .and_then(serde_json::Value::as_object)
        .map(|m| {
            m.iter().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k.clone()).collect()
        })
        .unwrap_or_default()
}

/// The user's stored values for plugin `id` (`plugins.config[id]`), limited
/// to declared keys and non-empty strings.
pub fn plugin_config(
    manifest: &PluginManifest,
    settings: Option<&serde_json::Value>,
) -> BTreeMap<String, String> {
    let stored = settings
        .and_then(|v| v.get("plugins"))
        .and_then(|p| p.get("config"))
        .and_then(|c| c.get(&manifest.id))
        .and_then(serde_json::Value::as_object);
    let Some(stored) = stored else { return BTreeMap::new() };
    manifest
        .settings
        .iter()
        .filter_map(|decl| {
            let value = stored.get(&decl.key)?.as_str()?;
            (!value.is_empty()).then(|| (decl.key.clone(), value.to_owned()))
        })
        .collect()
}

/// `env name -> value` for every declared setting the user filled in.
pub fn plugin_env(
    manifest: &PluginManifest,
    settings: Option<&serde_json::Value>,
) -> BTreeMap<String, String> {
    let config = plugin_config(manifest, settings);
    manifest
        .settings
        .iter()
        .filter(|decl| cctui_proto::worker_env::valid_plugin_env_name(&decl.env))
        .filter_map(|decl| config.get(&decl.key).map(|v| (decl.env.clone(), v.clone())))
        .collect()
}

/// The absolute file for a safe relative `path` inside `dir`, `None` when it
/// escapes the folder or is not a regular file.
fn resolve_in_dir(dir: &Path, path: &str) -> Option<PathBuf> {
    let root = dir.canonicalize().ok()?;
    let file = root.join(path).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

/// Static-file lookup for `GET /plugins/{id}/{path}`.
pub fn resolve_static(plugin: &Plugin, path: &str) -> Option<Vec<u8>> {
    plugin.files.read(path)
}

pub fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("md" | "txt") => "text/plain; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
pub mod test_support {
    use std::path::Path;

    /// Write a minimal valid plugin folder with one web entry and one skill.
    pub fn write_plugin(root: &Path, id: &str, extra_manifest: &str) -> std::path::PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::create_dir_all(dir.join("skills").join(id)).unwrap();
        std::fs::write(dir.join("web/index.js"), b"export default { cctuiApi: 1 };").unwrap();
        std::fs::write(dir.join("web/style.css"), b"body{}").unwrap();
        std::fs::write(dir.join("skills").join(id).join("SKILL.md"), b"# skill").unwrap();
        std::fs::write(dir.join("skills").join(id).join("notes.txt"), b"n").unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            format!(
                r#"{{"id":"{id}","name":"Plug {id}","description":"d","version":"1.2.3","cctuiApi":1,"web":"web/index.js","skills":["{id}"]{extra_manifest}}}"#
            ),
        )
        .unwrap();
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::write_plugin;
    use super::{
        ManifestError, PluginManifest, PluginRegistry, PluginSetting, PluginSource, enabled_ids,
        load_plugin, mime_for, plugin_config, plugin_env, resolve_static, safe_relative, scan_dir,
        valid_id, validate_manifest,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    fn manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.to_owned(),
            name: "N".to_owned(),
            description: String::new(),
            version: "1".to_owned(),
            cctui_api: 1,
            icon: None,
            web: None,
            skills: vec![],
            settings: vec![],
        }
    }

    fn setting(key: &str, env: &str) -> PluginSetting {
        PluginSetting {
            key: key.to_owned(),
            label: "L".to_owned(),
            env: env.to_owned(),
            kind: "string".to_owned(),
        }
    }

    #[test]
    fn settings_declarations_are_validated() {
        let mut m = manifest("p");
        m.settings = vec![setting("host", "YUBI_HOST"), setting("cert", "YUBI_TLS_CERT")];
        assert_eq!(validate_manifest(&m, "p", &|_| false), Ok(()));
        for (key, env) in [
            ("host", "PATH"),
            ("host", "CCTUI_X"),
            ("host", "ANTHROPIC_API_KEY"),
            ("host", "lower"),
            ("1bad", "OK_ENV"),
            ("", "OK_ENV"),
        ] {
            let mut m = manifest("p");
            m.settings = vec![setting(key, env)];
            assert!(
                matches!(
                    validate_manifest(&m, "p", &|_| false),
                    Err(ManifestError::BadSetting(..))
                ),
                "{key}/{env}"
            );
        }
        let mut m = manifest("p");
        m.settings = vec![setting("a", "SAME"), setting("b", "SAME")];
        assert!(matches!(
            validate_manifest(&m, "p", &|_| false),
            Err(ManifestError::BadSetting(..))
        ));
        let mut m = manifest("p");
        let mut s = setting("a", "A");
        s.kind = "number".into();
        m.settings = vec![s];
        assert!(matches!(
            validate_manifest(&m, "p", &|_| false),
            Err(ManifestError::BadSetting(..))
        ));
    }

    #[test]
    fn plugin_env_exports_filled_declared_values_only() {
        let mut m = manifest("p");
        m.settings = vec![setting("host", "YUBI_HOST"), setting("cert", "YUBI_TLS_CERT")];
        let settings = json!({ "plugins": { "config": {
            "p": { "host": "10.0.0.5", "cert": "", "undeclared": "x", "n": 3 },
            "q": { "host": "other" }
        } } });
        let env = plugin_env(&m, Some(&settings));
        assert_eq!(env, BTreeMap::from([("YUBI_HOST".to_owned(), "10.0.0.5".to_owned())]));
        assert_eq!(plugin_config(&m, Some(&settings)).keys().collect::<Vec<_>>(), vec!["host"]);
        assert!(plugin_env(&m, None).is_empty());
    }

    #[test]
    fn id_regex_and_folder_match() {
        assert!(valid_id("yubisashi"));
        assert!(valid_id("a-1"));
        assert!(!valid_id(""));
        assert!(!valid_id("Upper"));
        assert!(!valid_id("has space"));
        assert!(!valid_id(&"x".repeat(41)));
        assert_eq!(
            validate_manifest(&manifest("Bad"), "Bad", &|_| false),
            Err(ManifestError::BadId("Bad".into()))
        );
        assert_eq!(
            validate_manifest(&manifest("a"), "b", &|_| false),
            Err(ManifestError::IdFolderMismatch { id: "a".into(), folder: "b".into() })
        );
    }

    #[test]
    fn refuses_other_api_majors_and_unsafe_paths() {
        let mut m = manifest("p");
        m.cctui_api = 2;
        assert_eq!(
            validate_manifest(&m, "p", &|_| false),
            Err(ManifestError::UnsupportedApi("p".into(), 2))
        );
        let mut m = manifest("p");
        m.web = Some("../evil.js".into());
        assert!(matches!(validate_manifest(&m, "p", &|_| false), Err(ManifestError::BadPath(..))));
        let mut m = manifest("p");
        m.web = Some("/abs.js".into());
        assert!(matches!(validate_manifest(&m, "p", &|_| false), Err(ManifestError::BadPath(..))));
        let mut m = manifest("p");
        m.web = Some("web/missing.js".into());
        assert!(matches!(validate_manifest(&m, "p", &|_| false), Err(ManifestError::Missing(..))));
        let mut m = manifest("p");
        m.skills = vec!["a/b".into()];
        assert!(matches!(validate_manifest(&m, "p", &|_| false), Err(ManifestError::BadPath(..))));
        assert!(safe_relative("web/index.js"));
        assert!(!safe_relative(".hidden/x"));
        assert!(!safe_relative("a/../b"));
        assert!(!safe_relative(""));
    }

    #[test]
    fn scan_loads_valid_plugins_and_skips_broken_ones() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "good", "");
        let bad = root.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("plugin.json"), b"{not json").unwrap();
        let mismatch = root.path().join("other");
        std::fs::create_dir_all(&mismatch).unwrap();
        std::fs::write(
            mismatch.join("plugin.json"),
            br#"{"id":"good","name":"x","version":"1","cctuiApi":1}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join("no-manifest")).unwrap();

        let found = scan_dir(root.path());
        assert_eq!(found.keys().collect::<Vec<_>>(), vec!["good"]);
        let good = &found["good"];
        assert_eq!(good.web_hash.as_deref().map(str::len), Some(8));
        assert_eq!(good.skill_files, vec!["good/SKILL.md", "good/notes.txt"]);
        assert_eq!(good.skills_hash.len(), 16);

        let again = load_plugin(&root.path().join("good")).unwrap();
        assert_eq!(again.skills_hash, good.skills_hash);
        std::fs::write(root.path().join("good/skills/good/SKILL.md"), b"# changed").unwrap();
        assert_ne!(load_plugin(&root.path().join("good")).unwrap().skills_hash, good.skills_hash);
    }

    #[test]
    fn registry_rescan_picks_up_new_folders() {
        let root = tempfile::tempdir().unwrap();
        let reg = PluginRegistry::from_dir(root.path().to_path_buf());
        assert!(reg.enabled());
        assert!(reg.all().is_empty());
        write_plugin(root.path(), "late", "");
        assert_eq!(reg.rescan(), 1);
        assert_eq!(reg.get("late").unwrap().manifest.name, "Plug late");
        assert!(!PluginRegistry::disabled().enabled());
        assert_eq!(PluginRegistry::disabled().rescan(), 0);
    }

    #[test]
    fn enabled_ids_reads_true_flags_only() {
        let s = json!({ "plugins": { "enabled": { "a": true, "b": false, "c": "yes" } } });
        assert_eq!(enabled_ids(Some(&s)), vec!["a"]);
        assert!(enabled_ids(None).is_empty());
        assert!(enabled_ids(Some(&json!({}))).is_empty());
    }

    #[test]
    fn installed_plugins_follow_the_instance_toggle_and_shadow_the_dir() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "p", "");
        let registry = PluginRegistry::from_dir(root.path().to_path_buf());
        let mut installed = load_plugin(&root.path().join("p")).unwrap();
        installed.manifest.version = "9.9.9".into();
        installed.source = PluginSource::Installed;
        installed.instance_enabled = false;
        let mut other = installed.clone();
        other.manifest.id = "q".into();
        registry.set_installed(BTreeMap::from([("q".to_owned(), other)]));
        registry.upsert_installed(installed);
        assert!(registry.get("q").is_none());
        assert_eq!(registry.get("p").map(|p| p.manifest.version), None, "installed shadows dir");
        assert!(registry.all().is_empty());
        assert_eq!(registry.all_admin().len(), 2);
        assert!(registry.set_installed_enabled("p", true));
        assert!(!registry.set_installed_enabled("zz", true));
        assert_eq!(registry.get("p").unwrap().manifest.version, "9.9.9");
        assert_eq!(registry.all().len(), 1);
        assert!(registry.remove_installed("p"));
        assert_eq!(registry.get("p").unwrap().source, PluginSource::Directory);
    }

    #[test]
    fn static_resolution_stays_inside_the_plugin() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "p", "");
        std::fs::write(root.path().join("secret.txt"), b"s").unwrap();
        let plugin = load_plugin(&root.path().join("p")).unwrap();
        assert!(resolve_static(&plugin, "web/index.js").is_some());
        assert!(resolve_static(&plugin, "skills/p/SKILL.md").is_some());
        assert!(resolve_static(&plugin, "../secret.txt").is_none());
        assert!(resolve_static(&plugin, "web/../../secret.txt").is_none());
        assert!(resolve_static(&plugin, "/etc/passwd").is_none());
        assert!(resolve_static(&plugin, "web").is_none());
        assert!(resolve_static(&plugin, "web/nope.js").is_none());
        assert_eq!(mime_for("web/index.js"), "text/javascript; charset=utf-8");
        assert_eq!(mime_for("a.MJS"), "text/javascript; charset=utf-8");
        assert_eq!(mime_for("s.css"), "text/css; charset=utf-8");
        assert_eq!(mime_for("f.woff2"), "font/woff2");
        assert_eq!(mime_for("i.svg"), "image/svg+xml");
        assert_eq!(mime_for("x.bin"), "application/octet-stream");
    }
}
