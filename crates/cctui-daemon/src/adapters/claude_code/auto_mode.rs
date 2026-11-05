//! Warns when a session launched in `auto` first reports another mode.

use std::collections::HashSet;

use cctui_proto::adapter::{AdapterEvent, PermissionMode};

#[derive(Debug, Default)]
pub(super) struct AutoModeWatch {
    due: HashSet<String>,
}

impl AutoModeWatch {
    pub(super) fn arm(&mut self, short: &str) {
        self.due.insert(short.to_owned());
    }

    pub(super) fn forget(&mut self, short: &str) {
        self.due.remove(short);
    }

    /// Only the first reported mode is judged; later ones are user switches.
    pub(super) fn observe(&mut self, short: &str, evt: &AdapterEvent) -> Option<String> {
        let AdapterEvent::Status { permission_mode: Some(mode), .. } = evt else { return None };
        if !self.due.remove(short) || *mode == PermissionMode::Auto {
            return None;
        }
        Some(fallback_notice(*mode))
    }
}

pub(super) fn marker_payload(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "system_marker", "marker": "cctui/auto-mode", "text": text })
}

fn fallback_notice(reported: PermissionMode) -> String {
    let flag = reported.claude_flag();
    format!(
        "Auto mode unavailable: cctui launched this session with `--permission-mode auto`, but \
         Claude Code is running it in `{flag}`. The account plan, the Claude Code version or \
         the model may not support auto mode; check /permissions in the session. Actions now \
         follow `{flag}` rules."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_status(mode: Option<PermissionMode>) -> AdapterEvent {
        AdapterEvent::Status {
            local_id: "s".into(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: None,
            intent: None,
            model: None,
            effort: None,
            permission_mode: mode,
            children: Vec::new(),
        }
    }

    #[test]
    fn an_auto_launch_that_reports_another_mode_posts_one_notice() {
        let mut watch = AutoModeWatch::default();
        watch.arm("abcd1234");
        assert!(watch.observe("abcd1234", &mode_status(None)).is_none(), "name-only status");
        let notice = watch.observe("abcd1234", &mode_status(Some(PermissionMode::Ask))).unwrap();
        assert!(notice.contains("Auto mode unavailable"), "{notice}");
        assert!(notice.contains("`default`"), "{notice}");
        assert!(
            watch.observe("abcd1234", &mode_status(Some(PermissionMode::Ask))).is_none(),
            "only the first reported mode is judged"
        );
    }

    #[test]
    fn an_auto_launch_that_reports_auto_stays_quiet() {
        let mut watch = AutoModeWatch::default();
        watch.arm("abcd1234");
        assert!(watch.observe("abcd1234", &mode_status(Some(PermissionMode::Auto))).is_none());
        assert!(
            watch.observe("abcd1234", &mode_status(Some(PermissionMode::Ask))).is_none(),
            "a later user switch is not a fallback"
        );
    }

    #[test]
    fn sessions_not_launched_in_auto_are_never_flagged() {
        let mut watch = AutoModeWatch::default();
        assert!(watch.observe("other", &mode_status(Some(PermissionMode::Ask))).is_none());
        watch.arm("abcd1234");
        watch.forget("abcd1234");
        assert!(watch.observe("abcd1234", &mode_status(Some(PermissionMode::Ask))).is_none());
    }
}
