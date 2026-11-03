//! Claude Code ignores `--permission-mode bypassPermissions` unless the USER
//! settings file sets `skipDangerousModePermissionPrompt`; `--settings` cannot.

use std::path::{Path, PathBuf};

const KEY: &str = "skipDangerousModePermissionPrompt";

fn user_settings_path() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
        .map(|dir| dir.join("settings.json"))
}

pub fn ensure_for(mode: Option<cctui_proto::adapter::PermissionMode>) {
    if mode.is_none_or(|m| m.claude_flag() != "bypassPermissions") {
        return;
    }
    let Some(path) = user_settings_path() else { return };
    if let Err(err) = ensure_in(&path) {
        tracing::warn!(%err, path = %path.display(), "cannot accept bypassPermissions in user settings");
    }
}

/// Text insertion, not a reserialize: keeps the user's key order and formatting.
fn ensure_in(path: &Path) -> std::io::Result<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, format!("{{\n  \"{KEY}\": true\n}}\n"))?;
            return Ok(true);
        }
        Err(e) => return Err(e),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(std::io::Error::other)?;
    let Some(obj) = parsed.as_object() else {
        return Err(std::io::Error::other("user settings is not a JSON object"));
    };
    if obj.contains_key(KEY) {
        return Ok(false);
    }
    let brace = text.find('{').ok_or_else(|| std::io::Error::other("no opening brace"))?;
    let (head, tail) = text.split_at(brace + 1);
    let sep = if obj.is_empty() { "\n" } else { "," };
    let updated = format!("{head}\n  \"{KEY}\": true{sep}{tail}");
    serde_json::from_str::<serde_json::Value>(&updated).map_err(std::io::Error::other)?;
    std::fs::write(path, updated)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn creates_the_file_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/settings.json");
        assert!(ensure_in(&path).unwrap());
        assert_eq!(read(&path)[KEY], serde_json::json!(true));
    }

    #[test]
    fn adds_the_key_keeping_existing_order_and_formatting() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let original = "{\n    \"zeta\": 1,\n    \"alpha\": {\"b\": 2}\n}\n";
        std::fs::write(&path, original).unwrap();
        assert!(ensure_in(&path).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("    \"zeta\": 1,\n    \"alpha\": {\"b\": 2}\n}\n"), "{text}");
        let v = read(&path);
        assert_eq!(v[KEY], serde_json::json!(true));
        assert_eq!(v["alpha"]["b"], serde_json::json!(2));
        assert!(!ensure_in(&path).unwrap());
    }

    #[test]
    fn handles_an_empty_object() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(ensure_in(&path).unwrap());
        assert_eq!(read(&path)[KEY], serde_json::json!(true));
    }

    #[test]
    fn respects_an_explicit_false() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, format!("{{\"{KEY}\": false}}")).unwrap();
        assert!(!ensure_in(&path).unwrap());
        assert_eq!(read(&path)[KEY], serde_json::json!(false));
    }

    #[test]
    fn leaves_an_unparseable_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(ensure_in(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn writes_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("dotfiles.json");
        std::fs::write(&real, "{\"a\": 1}").unwrap();
        let link = tmp.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(ensure_in(&link).unwrap());
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(read(&real)[KEY], serde_json::json!(true));
    }
}
