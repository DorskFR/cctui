//! The agent table: one row per ACP agent, all driven by one adapter.
//!
//! Rows are compiled in because `AdapterFactory::id()` is `&'static str`. A
//! row only becomes a factory once the harness table in `cctui-proto` lists
//! its id, so adding an agent is a row here plus a harness row there.

use super::modes::{GEMINI_STYLE, ModeTable};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRow {
    /// Adapter id, the harness-table key.
    pub id: &'static str,
    pub label: &'static str,
    /// Binary looked up on `PATH` unless `config.bin` overrides it.
    pub bin: &'static str,
    /// Arguments that put the binary into ACP mode.
    pub args: &'static [&'static str],
    /// Full argv that updates the agent in place, when it has one.
    pub update: Option<&'static [&'static str]>,
    pub modes: ModeTable,
    /// Advertises models through the legacy `models` / `session/set_model`
    /// pair rather than config options.
    pub legacy_models: bool,
}

pub const GEMINI: AgentRow = AgentRow {
    id: "gemini",
    label: "Gemini CLI",
    bin: "gemini",
    args: &["--acp"],
    update: Some(&["npm", "install", "-g", "@google/gemini-cli@latest"]),
    modes: GEMINI_STYLE,
    legacy_models: true,
};

/// Every known agent, in picker order.
pub const ROWS: &[AgentRow] = &[GEMINI];

#[must_use]
pub fn row(id: &str) -> Option<&'static AgentRow> {
    ROWS.iter().find(|r| r.id == id)
}

/// The rows the daemon registers as factories: those the harness table
/// knows. A row the table does not list is data only.
#[must_use]
pub fn registered() -> Vec<&'static AgentRow> {
    ROWS.iter().filter(|r| cctui_proto::adapter::harness(r.id).is_some()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_has_an_update_command_that_is_not_claude() {
        for r in ROWS {
            let update = r.update.expect(r.id);
            assert!(!update.is_empty(), "{}", r.id);
            assert_ne!(update[0], "claude", "{}", r.id);
        }
    }

    #[test]
    fn a_registered_row_is_never_enabled_by_default() {
        for r in registered() {
            assert!(!cctui_proto::adapter::is_default_enabled(r.id), "{}", r.id);
        }
    }

    #[test]
    fn rows_are_found_by_id() {
        assert_eq!(row("gemini").map(|r| r.bin), Some("gemini"));
        assert_eq!(row("opencode"), None, "opencode stays native");
    }
}
