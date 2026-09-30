//! The read-only slice of the server's user settings the TUI obeys.

use serde_json::Value;

use super::ThemeChoice;

/// Settings the webui already stores per user that also change TUI behaviour.
/// Read-only: the TUI never writes this blob back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerPrefs {
    pub theme: Option<ThemeChoice>,
    pub compact_rows: Option<bool>,
    pub notifications: Option<bool>,
}

impl ServerPrefs {
    pub fn from_settings(data: &Value) -> Self {
        let display = data.get("display");
        let session_list = data.get("sessionList");
        Self {
            theme: display
                .and_then(|d| d.get("themeMode"))
                .and_then(Value::as_str)
                .and_then(ThemeChoice::parse),
            compact_rows: session_list
                .and_then(|s| s.get("density"))
                .and_then(Value::as_str)
                .map(|d| d == "compact"),
            notifications: display.and_then(|d| d.get("notifyEnabled")).and_then(Value::as_bool),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ServerPrefs, ThemeChoice};

    #[test]
    fn the_webui_blob_maps_onto_the_tui_knobs() {
        let prefs = ServerPrefs::from_settings(&json!({
            "display": {"themeMode": "light", "notifyEnabled": true},
            "sessionList": {"density": "compact"},
        }));
        assert_eq!(prefs.theme, Some(ThemeChoice::Light));
        assert_eq!(prefs.compact_rows, Some(true));
        assert_eq!(prefs.notifications, Some(true));
    }

    #[test]
    fn an_unknown_or_empty_blob_pins_nothing() {
        assert_eq!(ServerPrefs::from_settings(&json!({})), ServerPrefs::default());
        let odd = ServerPrefs::from_settings(&json!({
            "display": {"themeMode": "solarized"},
            "sessionList": {"density": "normal"},
        }));
        assert_eq!(odd.theme, None);
        assert_eq!(odd.compact_rows, Some(false));
    }
}
