//! The agent table: one row per ACP agent, all driven by one adapter.
//!
//! Rows are compiled in because `AdapterFactory::id()` is `&'static str`. A
//! row only becomes a factory once the harness table in `cctui-proto` lists
//! its id, so adding an agent is a row here plus a harness row there.

use super::modes::{GEMINI_STYLE, GOOSE_STYLE, ModeTable, QWEN_STYLE};

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
    /// The project file the agent reads its own instructions from.
    pub context_file: &'static str,
    /// Env the agent authenticates from on its own login; an unbound launch
    /// with none of these set is flagged, not refused.
    pub auth_env: &'static [&'static str],
    /// The env var that names the launch model, for an agent that reads it
    /// before any config option can be set.
    pub model_env: Option<&'static str>,
}

pub const GEMINI: AgentRow = AgentRow {
    id: "gemini",
    label: "Gemini CLI",
    bin: "gemini",
    args: &["--acp"],
    update: Some(&["npm", "install", "-g", "@google/gemini-cli@latest"]),
    modes: GEMINI_STYLE,
    legacy_models: true,
    context_file: "GEMINI.md",
    auth_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS"],
    model_env: None,
};

/// qwen-code: a gemini-cli fork on the OpenAI wire. Qwen OAuth was
/// discontinued, so its only auth is `openai`: the OpenAI SDK triple, which
/// the gateway binding mints.
pub const QWEN: AgentRow = AgentRow {
    id: "qwen",
    label: "Qwen Code",
    bin: "qwen",
    args: &["--acp"],
    update: Some(&["npm", "install", "-g", "@qwen-code/qwen-code@latest"]),
    modes: QWEN_STYLE,
    legacy_models: true,
    context_file: "QWEN.md",
    auth_env: &["OPENAI_API_KEY"],
    model_env: Some("OPENAI_MODEL"),
};

/// goose: its ACP server picks the provider from `GOOSE_PROVIDER`; the gateway
/// binding selects `openai`. It advertises `session/load`, not `session/resume`.
pub const GOOSE: AgentRow = AgentRow {
    id: "goose",
    label: "Goose",
    bin: "goose",
    args: &["acp"],
    update: Some(&["goose", "update"]),
    modes: GOOSE_STYLE,
    legacy_models: false,
    context_file: ".goosehints",
    auth_env: &["GOOSE_PROVIDER", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"],
    model_env: Some("GOOSE_MODEL"),
};

/// Every known agent, in picker order.
pub const ROWS: &[AgentRow] = &[GEMINI, QWEN, GOOSE];

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
        assert_eq!(row("qwen").map(|r| r.args), Some(&["--acp"][..]));
        assert_eq!(row("goose").map(|r| r.args), Some(&["acp"][..]));
        assert_eq!(row("opencode"), None, "opencode stays native");
    }

    /// The OpenAI-wire agents are the registered ones: the harness table lists
    /// them, default-off, and the launch model travels in the env each reads.
    #[test]
    fn the_openai_wire_agents_are_registered_and_name_their_model_env() {
        let ids: Vec<&str> = registered().iter().map(|r| r.id).collect();
        assert_eq!(ids, ["qwen", "goose"]);
        assert_eq!(QWEN.model_env, Some("OPENAI_MODEL"));
        assert_eq!(GOOSE.model_env, Some("GOOSE_MODEL"));
        for r in registered() {
            assert!(
                !crate::adapters::gateway_env::required_gateway_keys(r.id).is_empty(),
                "{} is bound through a gateway family and must fail closed",
                r.id
            );
            assert!(!r.auth_env.is_empty(), "{}", r.id);
            assert!(!r.context_file.is_empty(), "{}", r.id);
        }
    }
}
