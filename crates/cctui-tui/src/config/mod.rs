//! `~/.config/cctui/tui.toml`: keymap, theme and local preferences.

pub mod chord;
pub mod keymap;
pub mod prefs;
pub mod server;
pub mod uistate;

use std::path::{Path, PathBuf};

use keymap::{Context, Keymap};
use prefs::Prefs;
use serde::Deserialize;

/// Which palette to paint; `Auto` reads the terminal's background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    Light,
    Dark,
    #[default]
    Auto,
}

impl ThemeChoice {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub theme: ThemeChoice,
    pub keys: Keymap,
    pub prefs: Prefs,
    /// Kept so the server's settings can be layered underneath the file's own
    /// entries when they arrive, after the file has already been read.
    file: prefs::PrefsFile,
    theme_pinned: bool,
}

impl Config {
    pub fn apply_server(&mut self, server: server::ServerPrefs) {
        if let Some(theme) = server.theme
            && !self.theme_pinned
        {
            self.theme = theme;
        }
        let mut prefs = Prefs::default();
        prefs.apply_server(server);
        prefs.apply_file(&self.file);
        self.prefs = prefs;
    }
}

/// A config load never fails: every problem is collected and surfaced in the
/// TUI, and the defaults stay in force for whatever could not be read.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub config: Config,
    pub problems: Vec<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    theme: Option<String>,
    #[serde(default)]
    preferences: prefs::PrefsFile,
    #[serde(default)]
    keys: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

/// `$CCTUI_TUI_CONFIG` wins so a test (or a second instance) can point elsewhere.
pub fn config_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CCTUI_TUI_CONFIG") {
        return Some(PathBuf::from(explicit));
    }
    Some(dirs::config_dir()?.join("cctui").join("tui.toml"))
}

pub fn load() -> Loaded {
    let Some(path) = config_path() else {
        return Loaded { config: defaults_with(&mut Vec::new()), ..Loaded::default() };
    };
    let mut loaded = load_from(&path);
    loaded.path = Some(path);
    loaded
}

pub fn load_from(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut problems = Vec::new();
            let config = defaults_with(&mut problems);
            Loaded { config, problems, path: None }
        }
        Err(err) => {
            let mut problems = vec![format!("{}: {err}", path.display())];
            let config = defaults_with(&mut problems);
            Loaded { config, problems, path: None }
        }
    }
}

fn defaults_with(problems: &mut Vec<String>) -> Config {
    Config {
        theme: ThemeChoice::default(),
        keys: Keymap::new(problems),
        prefs: Prefs::default(),
        file: prefs::PrefsFile::default(),
        theme_pinned: false,
    }
}

pub fn parse(text: &str) -> Loaded {
    let mut problems = Vec::new();
    let mut config = defaults_with(&mut problems);
    let file: FileConfig = match toml::from_str(text) {
        Ok(file) => file,
        Err(err) => {
            problems.push(format!("tui.toml is not valid TOML: {err}"));
            return Loaded { config, problems, path: None };
        }
    };

    if let Some(theme) = file.theme {
        match ThemeChoice::parse(&theme) {
            Some(choice) => {
                config.theme = choice;
                config.theme_pinned = true;
            }
            None => problems
                .push(format!("theme `{theme}` is not one of light, dark, auto — using auto")),
        }
    }
    config.file = file.preferences;
    config.prefs.apply_file(&config.file.clone());

    for (context_name, table) in &file.keys {
        let Some(context) = Context::parse(context_name) else {
            problems.push(format!("[keys.{context_name}] is not a known context"));
            continue;
        };
        for (keys, action) in table {
            if let Err(err) = config.keys.set(context, keys, action) {
                problems.push(format!("[keys.{context_name}] {keys}: {err}"));
            }
        }
    }
    Loaded { config, problems, path: None }
}

#[cfg(test)]
mod tests {
    use super::chord::Chord;
    use super::keymap::{ActionId, Context};
    use super::{Config, ThemeChoice, parse};

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("parses")
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        let loaded = parse("");
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn user_bindings_merge_over_the_defaults() {
        let loaded = parse(
            r#"
theme = "light"

[preferences]
timestamps = true

[keys.session-list]
"x" = "select-next"
"j" = "none"
"#,
        );
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded.config.theme, ThemeChoice::Light);
        assert!(loaded.config.prefs.timestamps);
        let keys = &loaded.config.keys;
        assert_eq!(keys.lookup(Context::SessionList, chord("x")), Some(ActionId::SelectNext));
        assert_eq!(keys.lookup(Context::SessionList, chord("j")), None);
        assert_eq!(keys.lookup(Context::SessionList, chord("k")), Some(ActionId::SelectPrev));
    }

    #[test]
    fn every_bad_entry_is_reported_and_the_rest_still_loads() {
        let loaded = parse(
            r#"
theme = "neon"

[keys.nowhere]
"x" = "quit"

[keys.global]
"hyper+x" = "quit"
"Q" = "no-such-action"
"ctrl+x" = "quit"
"#,
        );
        assert_eq!(loaded.problems.len(), 4, "{:?}", loaded.problems);
        assert_eq!(loaded.config.theme, ThemeChoice::Auto);
        assert_eq!(
            loaded.config.keys.lookup(Context::Global, chord("ctrl+x")),
            Some(ActionId::Quit)
        );
    }

    #[test]
    fn a_broken_file_keeps_the_defaults() {
        let loaded = parse("this is not toml");
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.config, Config::default());
    }
}
