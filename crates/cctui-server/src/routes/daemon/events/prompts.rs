/// auto-approve is scoped to tool-use permissions. `ExitPlanMode` and
/// `AskUserQuestion` are user decision points that must always be answered by
/// the user, so they are excluded from the auto-approve short-circuit even when
/// the session flag is set.
pub(super) fn is_auto_approve_excluded(tool: &str) -> bool {
    tool == "ExitPlanMode" || tool == "AskUserQuestion"
}

pub(super) fn should_auto_approve(tool: &str, auto_approve_enabled: bool) -> bool {
    auto_approve_enabled && !is_auto_approve_excluded(tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_excludes_plan_and_ask_but_allows_tools() {
        assert!(should_auto_approve("Bash", true));
        assert!(should_auto_approve("Edit", true));
        assert!(should_auto_approve("Write", true));
        assert!(!should_auto_approve("ExitPlanMode", true));
        assert!(!should_auto_approve("AskUserQuestion", true));
    }

    #[test]
    fn auto_approve_off_never_approves() {
        assert!(!should_auto_approve("Bash", false));
        assert!(!should_auto_approve("ExitPlanMode", false));
    }
}
