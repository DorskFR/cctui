//! Whether each enabled adapter's binary is installed. Reconcile records which
//! binaries to look for; each heartbeat and each spawn re-resolves them, so a
//! binary installed after the reconcile is picked up without one.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use cctui_proto::harness::AdapterBinary;

use crate::adapter_runtime::BinarySpec;

static WATCHED: LazyLock<Mutex<BTreeMap<String, BinarySpec>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub fn watch(specs: BTreeMap<String, BinarySpec>) {
    for (adapter_id, binary) in probe_all(&specs, &crate::childenv::child_path()) {
        if !binary.installed() {
            tracing::warn!(%adapter_id, bin = %binary.bin, "adapter binary not installed");
        }
    }
    *WATCHED.lock().unwrap_or_else(PoisonError::into_inner) = specs;
}

#[must_use]
pub fn report() -> BTreeMap<String, AdapterBinary> {
    let specs = WATCHED.lock().unwrap_or_else(PoisonError::into_inner).clone();
    probe_all(&specs, &crate::childenv::child_path())
}

#[must_use]
pub fn spawn_refusal(adapter_id: &str) -> Option<String> {
    let spec = WATCHED.lock().unwrap_or_else(PoisonError::into_inner).get(adapter_id).cloned()?;
    probe(&spec, &crate::childenv::child_path()).refusal(adapter_id)
}

fn probe_all(specs: &BTreeMap<String, BinarySpec>, path: &str) -> BTreeMap<String, AdapterBinary> {
    specs.iter().map(|(id, spec)| (id.clone(), probe(spec, path))).collect()
}

#[must_use]
pub fn probe(spec: &BinarySpec, path: &str) -> AdapterBinary {
    AdapterBinary {
        bin: spec.bin.clone(),
        config_key: spec.config_key.to_owned(),
        path: resolve(&spec.bin, path).map(|p| p.display().to_string()),
    }
}

#[must_use]
pub fn resolve(bin: &str, path: &str) -> Option<PathBuf> {
    let bin = bin.trim();
    if bin.is_empty() {
        return None;
    }
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return executable(&p).then_some(p);
    }
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(bin))
        .find(|p| executable(p))
}

fn executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(bin: &str) -> BinarySpec {
        BinarySpec { bin: bin.to_owned(), config_key: "bin" }
    }

    fn install(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    #[test]
    fn a_bare_name_resolves_on_the_first_path_dir_that_holds_an_executable() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        install(a.path(), "agent", 0o644);
        let want = install(b.path(), "agent", 0o755);
        let path = format!("{}::{}", a.path().display(), b.path().display());
        assert_eq!(resolve("agent", &path), Some(want));
        assert_eq!(resolve("nope", &path), None);
        assert_eq!(resolve("  ", &path), None);
    }

    #[test]
    fn a_path_override_is_checked_as_is_and_ignores_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = install(dir.path(), "opencode", 0o755);
        let shown = bin.display().to_string();
        assert_eq!(resolve(&shown, ""), Some(bin));
        let missing = dir.path().join("gone").display().to_string();
        assert_eq!(resolve(&missing, &dir.path().display().to_string()), None);
        assert_eq!(resolve(&dir.path().display().to_string(), ""), None, "a directory is no binary");
    }

    #[test]
    fn a_missing_binary_is_reported_with_the_key_that_overrides_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().display().to_string();
        let missing = probe(&BinarySpec { bin: "codex".to_owned(), config_key: "codex_bin" }, &path);
        assert!(!missing.installed());
        assert_eq!(missing.config_key, "codex_bin");
        assert!(missing.refusal("codex").is_some_and(|m| m.contains("`codex_bin`")));

        let bin = install(dir.path(), "claude", 0o755);
        let found = probe(&spec("claude"), &path);
        assert_eq!(found.path, Some(bin.display().to_string()));
        assert!(found.refusal("claude-code").is_none());
    }
}
