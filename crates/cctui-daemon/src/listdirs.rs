//! List sub-directories of a path for the spawn dialog's working-directory
//! autocomplete. Kept dumb on purpose: the daemon resolves `~`, lists ONE
//! directory level, and returns sorted entry names — splitting the user's
//! input into (parent, prefix) and filtering is the web UI's job.

use std::path::PathBuf;

/// Hard cap on returned entries so a pathological directory can't bloat the
/// WS frame. The UI filters by typed prefix anyway.
const MAX_ENTRIES: usize = 500;

/// Expand a leading `~` / `~/…` to `$HOME`. `~user` forms pass through
/// untouched (rare, and resolving them portably isn't worth it).
fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Names of the sub-directories of `path` (directories only, dotdirs
/// included — display filtering is client-side), sorted case-insensitively,
/// capped at [`MAX_ENTRIES`].
pub fn list_dirs(path: &str) -> anyhow::Result<Vec<String>> {
    let dir = expand_tilde(path);
    let read = std::fs::read_dir(&dir)
        .map_err(|err| anyhow::anyhow!("cannot read {}: {err}", dir.display()))?;
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        // metadata() follows symlinks so a symlinked dir counts; entries we
        // can't stat are skipped rather than failing the whole listing.
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.truncate(MAX_ENTRIES);
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_directories_sorted_case_insensitively() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("beta")).unwrap();
        std::fs::create_dir(tmp.path().join("Alpha")).unwrap();
        std::fs::write(tmp.path().join("file.txt"), b"x").unwrap();
        let dirs = list_dirs(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(dirs, vec!["Alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn includes_dotdirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
        let dirs = list_dirs(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(dirs, vec![".hidden".to_string()]);
    }

    #[test]
    fn missing_path_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert!(list_dirs(missing.to_str().unwrap()).is_err());
    }

    #[test]
    fn expands_tilde_against_home() {
        // Don't set env vars in tests (racy across threads) — derive the
        // expectation from the HOME the test process already has.
        let home = std::env::var("HOME").expect("HOME set in test env");
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        assert_eq!(expand_tilde("~/x"), std::path::Path::new(&home).join("x"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
    }
}
