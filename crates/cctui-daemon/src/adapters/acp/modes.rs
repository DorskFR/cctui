//! How a cctui [`PermissionMode`] is applied to one ACP agent.
//!
//! Agents only call `session/request_permission` when their own policy says
//! so, so the posture has to be set on the agent at launch. Each agent row
//! carries a table from the four cctui modes to whatever the agent can
//! express; a mode with no entry is refused before the process is spawned.

use cctui_proto::adapter::PermissionMode;

/// One way of applying a mode to a running agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeApply {
    /// `session/set_mode` with this mode id.
    SessionMode(&'static str),
    /// `session/set_config_option` on the option with category `mode`.
    ConfigOption { id: &'static str, value: &'static str },
    /// A launch flag appended to the agent's argv.
    LaunchFlag(&'static str),
}

/// The mode table of one agent row. `None` means the agent cannot express
/// that posture, and a spawn asking for it is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModeTable {
    pub ask: Option<ModeApply>,
    pub auto: Option<ModeApply>,
    pub yolo: Option<ModeApply>,
    pub whip: Option<ModeApply>,
}

/// A spawn asked for a posture the agent has no equivalent of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inexpressible {
    pub mode: PermissionMode,
    pub agent: &'static str,
}

impl std::fmt::Display for Inexpressible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "permission mode `{}` cannot be expressed by {}: refusing to spawn rather than \
             widening it",
            label(self.mode),
            self.agent
        )
    }
}

impl std::error::Error for Inexpressible {}

const fn label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
        PermissionMode::Whip => "whip",
    }
}

impl ModeTable {
    #[must_use]
    pub const fn entry(&self, mode: PermissionMode) -> Option<ModeApply> {
        match mode {
            PermissionMode::Ask => self.ask,
            PermissionMode::Auto => self.auto,
            PermissionMode::Yolo => self.yolo,
            PermissionMode::Whip => self.whip,
        }
    }

    /// The agent-side application of `mode`, or the refusal to report.
    pub const fn resolve(
        &self,
        agent: &'static str,
        mode: PermissionMode,
    ) -> Result<ModeApply, Inexpressible> {
        match self.entry(mode) {
            Some(apply) => Ok(apply),
            None => Err(Inexpressible { mode, agent }),
        }
    }

    /// The modes this table can express, in picker order.
    #[must_use]
    pub fn expressible(&self) -> Vec<PermissionMode> {
        PermissionMode::ALL.into_iter().filter(|m| self.entry(*m).is_some()).collect()
    }

    /// The cctui posture an agent-reported mode id maps back to, when the
    /// table knows it.
    #[must_use]
    pub fn posture_of(&self, agent_mode_id: &str) -> Option<PermissionMode> {
        PermissionMode::ALL.into_iter().find(|m| match self.entry(*m) {
            Some(ModeApply::SessionMode(id)) => id == agent_mode_id,
            Some(ModeApply::ConfigOption { value, .. }) => value == agent_mode_id,
            Some(ModeApply::LaunchFlag(_)) | None => false,
        })
    }
}

/// The table for agents speaking legacy `modes` with the gemini vocabulary.
///
/// `default` / `autoEdit` / `yolo`; whip has no agent-side equivalent, being
/// yolo plus hooks the agent cannot run.
pub const GEMINI_STYLE: ModeTable = ModeTable {
    ask: Some(ModeApply::SessionMode("default")),
    auto: Some(ModeApply::SessionMode("autoEdit")),
    yolo: Some(ModeApply::SessionMode("yolo")),
    whip: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_style_refuses_whip_and_maps_the_rest() {
        assert_eq!(
            GEMINI_STYLE.resolve("gemini", PermissionMode::Ask),
            Ok(ModeApply::SessionMode("default"))
        );
        assert_eq!(
            GEMINI_STYLE.resolve("gemini", PermissionMode::Auto),
            Ok(ModeApply::SessionMode("autoEdit"))
        );
        assert_eq!(
            GEMINI_STYLE.resolve("gemini", PermissionMode::Yolo),
            Ok(ModeApply::SessionMode("yolo"))
        );
        let err = GEMINI_STYLE.resolve("gemini", PermissionMode::Whip).unwrap_err();
        assert_eq!(err, Inexpressible { mode: PermissionMode::Whip, agent: "gemini" });
        assert!(err.to_string().contains("`whip`"), "{err}");
        assert!(err.to_string().contains("refusing to spawn"), "{err}");
    }

    #[test]
    fn expressible_modes_follow_picker_order() {
        assert_eq!(
            GEMINI_STYLE.expressible(),
            [PermissionMode::Ask, PermissionMode::Auto, PermissionMode::Yolo]
        );
        assert!(ModeTable::default().expressible().is_empty());
    }

    #[test]
    fn an_agent_mode_id_maps_back_to_a_posture() {
        assert_eq!(GEMINI_STYLE.posture_of("autoEdit"), Some(PermissionMode::Auto));
        assert_eq!(GEMINI_STYLE.posture_of("plan"), None);
        let table = ModeTable {
            yolo: Some(ModeApply::ConfigOption { id: "approval", value: "never" }),
            ..ModeTable::default()
        };
        assert_eq!(table.posture_of("never"), Some(PermissionMode::Yolo));
    }
}
