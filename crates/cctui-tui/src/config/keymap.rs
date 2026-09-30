//! The binding table: every key the TUI reacts to resolves through here.

use std::collections::HashMap;
use std::fmt;

use super::chord::Chord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Global,
    SessionList,
    Conversation,
    Composer,
    History,
    Terminal,
    Help,
    Permission,
    Diagnose,
    Ask,
    AskText,
    Plan,
    PlanText,
}

pub const CONTEXTS: &[Context] = &[
    Context::Global,
    Context::SessionList,
    Context::Conversation,
    Context::Composer,
    Context::History,
    Context::Terminal,
    Context::Help,
    Context::Permission,
    Context::Diagnose,
    Context::Ask,
    Context::AskText,
    Context::Plan,
    Context::PlanText,
];

impl Context {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::SessionList => "session-list",
            Self::Conversation => "conversation",
            Self::Composer => "composer",
            Self::History => "history",
            Self::Terminal => "terminal",
            Self::Help => "help",
            Self::Permission => "permission",
            Self::Diagnose => "diagnose",
            Self::Ask => "ask",
            Self::AskText => "ask-text",
            Self::Plan => "plan",
            Self::PlanText => "plan-text",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Global => "Anywhere",
            Self::SessionList => "Session list",
            Self::Conversation => "Conversation",
            Self::Composer => "Composer",
            Self::History => "Prompt history",
            Self::Terminal => "Terminal pane",
            Self::Help => "Help",
            Self::Permission => "Permission card",
            Self::Diagnose => "Diagnose / info",
            Self::Ask => "Question card",
            Self::AskText => "Question card — free text",
            Self::Plan => "Plan card",
            Self::PlanText => "Plan card — refine",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        CONTEXTS.iter().copied().find(|c| c.as_str() == text)
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

macro_rules! actions {
    ($($variant:ident => $name:literal, $desc:literal;)*) => {
        /// Every nameable binding target. Variants without a key in
        /// [`DEFAULT_BINDINGS`] are reserved for later waves.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ActionId {
            $($variant,)*
        }

        pub const ACTION_IDS: &[ActionId] = &[$(ActionId::$variant,)*];

        impl ActionId {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }

            pub const fn description(self) -> &'static str {
                match self {
                    $(Self::$variant => $desc,)*
                }
            }
        }
    };
}

actions! {
    Help => "help", "Toggle this cheat sheet";
    Quit => "quit", "Quit cctui";
    SwitchView => "switch-view", "Switch view";
    Search => "search", "Search the current view";
    SearchNext => "search-next", "Next search hit";
    SearchPrev => "search-prev", "Previous search hit";
    Diagnose => "diagnose", "Diagnose the selected session";
    Info => "info", "Session info";
    Refresh => "refresh", "Refresh from the server";

    SelectNext => "select-next", "Next session";
    SelectPrev => "select-prev", "Previous session";
    SelectFirst => "select-first", "First session";
    SelectLast => "select-last", "Last session";
    SelectIndex => "select-index", "Jump to session by number";
    OpenConversation => "open-conversation", "Open the conversation";
    ToggleCompactRows => "toggle-compact-rows", "Compact session rows";
    ToggleFold => "toggle-fold", "Fold or open the subagent group";
    ToggleFoldSection => "toggle-fold-section", "Fold or open the section";
    ToggleFoldAll => "toggle-fold-all", "Fold or open everything";
    TogglePin => "toggle-pin", "Pin the selected session";
    NewSession => "new-session", "Spawn a session";
    Archive => "archive", "Archive the selected session";
    Fork => "fork", "Fork the session";
    Resume => "resume", "Resume the session";

    LeaveConversation => "leave-conversation", "Back to the session list";
    ScrollDown => "scroll-down", "Scroll down";
    ScrollUp => "scroll-up", "Scroll up";
    PageDown => "page-down", "Page down";
    PageUp => "page-up", "Page up";
    ScrollToTop => "scroll-to-top", "Jump to the top";
    ScrollToBottom => "scroll-to-bottom", "Jump to the bottom";
    ToggleTimestamps => "toggle-timestamps", "Show timestamps";
    Interrupt => "interrupt", "Interrupt the turn";
    ToggleAutoApprove => "toggle-auto-approve", "Toggle auto-approve";
    LineCursor => "line-cursor", "Select transcript lines";
    ToggleExpand => "toggle-expand", "Expand the focused line";
    ToggleExpandAll => "toggle-expand-all", "Expand every thinking and result block";
    RetrySend => "retry-send", "Retry the undelivered message";
    EditSend => "edit-send", "Edit the undelivered message";
    DiscardSend => "discard-send", "Drop the undelivered message";
    CopyMessage => "copy-message", "Copy the selected message";
    OpenInEditor => "open-in-editor", "Compose in $EDITOR";
    TerminalOpen => "terminal-open", "Watch the live terminal";
    TerminalClose => "terminal-close", "Close the terminal pane";
    TerminalScrollDown => "terminal-scroll-down", "Scroll the terminal down";
    TerminalScrollUp => "terminal-scroll-up", "Scroll the terminal up";

    CancelInput => "cancel-input", "Close the composer";
    SubmitInput => "submit-input", "Send the message";
    InputNewline => "input-newline", "Insert a newline";
    HistoryPrev => "history-prev", "Recall an earlier prompt";
    HistoryNext => "history-next", "Recall a later prompt";
    HistoryOpen => "history-open", "Search sent prompts";

    HistoryClose => "history-close", "Close the prompt list";
    HistorySelectNext => "history-select-next", "Next prompt";
    HistorySelectPrev => "history-select-prev", "Previous prompt";
    HistoryRecall => "history-recall", "Put this prompt in the composer";

    CloseHelp => "close-help", "Close this cheat sheet";

    DiagnoseClose => "diagnose-close", "Close the panel";
    DiagnoseScrollDown => "diagnose-scroll-down", "Scroll the panel down";
    DiagnoseScrollUp => "diagnose-scroll-up", "Scroll the panel up";
    DiagnosePageDown => "diagnose-page-down", "Page the panel down";
    DiagnosePageUp => "diagnose-page-up", "Page the panel up";
    DiagnoseTop => "diagnose-top", "Jump to the top of the panel";
    DiagnoseRefresh => "diagnose-refresh", "Refresh the report";
    DiagnoseCopyId => "diagnose-copy-id", "Copy the session id";

    PermissionAllow => "permission-allow", "Allow";
    PermissionDeny => "permission-deny", "Deny";
    PermissionAllowAlways => "permission-allow-always", "Allow and auto-approve";
    JumpToPending => "jump-to-pending", "Jump to the next pending approval";

    FocusPrompt => "focus-prompt", "Answer the waiting prompt";
    PromptDefer => "prompt-defer", "Answer later";
    AskNextOption => "ask-next-option", "Next option";
    AskPrevOption => "ask-prev-option", "Previous option";
    AskPickIndex => "ask-pick-index", "Pick an option by number";
    AskToggleOption => "ask-toggle-option", "Toggle the focused option";
    AskNextQuestion => "ask-next-question", "Next question";
    AskPrevQuestion => "ask-prev-question", "Previous question";
    AskEditOther => "ask-edit-other", "Write a free-text answer";
    AskSubmit => "ask-submit", "Send the answer";
    PromptTextCommit => "prompt-text-commit", "Accept the text";
    PromptTextCancel => "prompt-text-cancel", "Discard the text";
    PlanApproveAuto => "plan-approve-auto", "Approve, auto-accept edits";
    PlanApproveManual => "plan-approve-manual", "Approve, manually approve edits";
    PlanKeepPlanning => "plan-keep-planning", "Keep planning";
    PlanRefine => "plan-refine", "Refine the plan";
    PlanScrollDown => "plan-scroll-down", "Scroll the plan down";
    PlanScrollUp => "plan-scroll-up", "Scroll the plan up";
}

impl ActionId {
    pub fn parse(text: &str) -> Option<Self> {
        ACTION_IDS.iter().copied().find(|a| a.as_str() == text)
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct BindingSpec {
    pub context: Context,
    /// Comma-separated chords; `1-9` expands to a range.
    pub keys: &'static str,
    pub action: ActionId,
}

const fn spec(context: Context, keys: &'static str, action: ActionId) -> BindingSpec {
    BindingSpec { context, keys, action }
}

const GLOBAL: &[BindingSpec] = &[
    spec(Context::Global, "?", ActionId::Help),
    spec(Context::Global, "q", ActionId::Quit),
    spec(Context::Global, "1-9", ActionId::SwitchView),
    spec(Context::Global, "/", ActionId::Search),
    spec(Context::Global, "n", ActionId::SearchNext),
    spec(Context::Global, "N", ActionId::SearchPrev),
    spec(Context::Global, "D", ActionId::Diagnose),
    spec(Context::Global, "i", ActionId::Info),
    spec(Context::Global, "ctrl+g", ActionId::JumpToPending),
];

const SESSION_LIST: &[BindingSpec] = &[
    spec(Context::SessionList, "j, down", ActionId::SelectNext),
    spec(Context::SessionList, "k, up", ActionId::SelectPrev),
    spec(Context::SessionList, "g", ActionId::SelectFirst),
    spec(Context::SessionList, "G", ActionId::SelectLast),
    spec(Context::SessionList, "enter", ActionId::OpenConversation),
    spec(Context::SessionList, "tab, z", ActionId::ToggleFold),
    spec(Context::SessionList, "S", ActionId::ToggleFoldSection),
    spec(Context::SessionList, "Z", ActionId::ToggleFoldAll),
];

const CONVERSATION: &[BindingSpec] = &[
    spec(Context::Conversation, "esc, q", ActionId::LeaveConversation),
    spec(Context::Conversation, "j, down", ActionId::ScrollDown),
    spec(Context::Conversation, "k, up", ActionId::ScrollUp),
    spec(Context::Conversation, "pagedown", ActionId::PageDown),
    spec(Context::Conversation, "pageup", ActionId::PageUp),
    spec(Context::Conversation, "g", ActionId::ScrollToTop),
    spec(Context::Conversation, "G", ActionId::ScrollToBottom),
    spec(Context::Conversation, "t", ActionId::ToggleTimestamps),
    spec(Context::Conversation, "v", ActionId::LineCursor),
    spec(Context::Conversation, "o", ActionId::ToggleExpand),
    spec(Context::Conversation, "z", ActionId::ToggleExpandAll),
    spec(Context::Conversation, "1-9", ActionId::SelectIndex),
    spec(Context::Conversation, "R", ActionId::RetrySend),
    spec(Context::Conversation, "e", ActionId::EditSend),
    spec(Context::Conversation, "x", ActionId::DiscardSend),
    spec(Context::Conversation, "ctrl+c", ActionId::Interrupt),
    spec(Context::Conversation, "ctrl+a", ActionId::ToggleAutoApprove),
    spec(Context::Conversation, "ctrl+r", ActionId::HistoryOpen),
    spec(Context::Conversation, "tab", ActionId::FocusPrompt),
    spec(Context::Conversation, "T", ActionId::TerminalOpen),
];

const TERMINAL: &[BindingSpec] = &[
    spec(Context::Terminal, "esc, q, T", ActionId::TerminalClose),
    spec(Context::Terminal, "j, down", ActionId::TerminalScrollDown),
    spec(Context::Terminal, "k, up", ActionId::TerminalScrollUp),
];

/// `shift+enter` is unreported by most terminals, so a newline also has a
/// modifier pair that always arrives.
const COMPOSER: &[BindingSpec] = &[
    spec(Context::Composer, "esc", ActionId::CancelInput),
    spec(Context::Composer, "enter", ActionId::SubmitInput),
    spec(Context::Composer, "shift+enter, alt+enter, ctrl+j", ActionId::InputNewline),
    spec(Context::Composer, "up", ActionId::HistoryPrev),
    spec(Context::Composer, "down", ActionId::HistoryNext),
    spec(Context::Composer, "ctrl+r", ActionId::HistoryOpen),
];

const HISTORY: &[BindingSpec] = &[
    spec(Context::History, "esc", ActionId::HistoryClose),
    spec(Context::History, "down, ctrl+n", ActionId::HistorySelectNext),
    spec(Context::History, "up, ctrl+p", ActionId::HistorySelectPrev),
    spec(Context::History, "enter", ActionId::HistoryRecall),
];

/// The panel is modal, so it claims its own scrolling rather than falling
/// through to the view underneath.
const DIAGNOSE: &[BindingSpec] = &[
    spec(Context::Diagnose, "esc, q", ActionId::DiagnoseClose),
    spec(Context::Diagnose, "j, down", ActionId::DiagnoseScrollDown),
    spec(Context::Diagnose, "k, up", ActionId::DiagnoseScrollUp),
    spec(Context::Diagnose, "pagedown", ActionId::DiagnosePageDown),
    spec(Context::Diagnose, "pageup", ActionId::DiagnosePageUp),
    spec(Context::Diagnose, "g", ActionId::DiagnoseTop),
    spec(Context::Diagnose, "r", ActionId::DiagnoseRefresh),
    spec(Context::Diagnose, "y", ActionId::DiagnoseCopyId),
    spec(Context::Diagnose, "D", ActionId::Diagnose),
    spec(Context::Diagnose, "i", ActionId::Info),
];

const HELP: &[BindingSpec] = &[
    spec(Context::Help, "esc, q, ?", ActionId::CloseHelp),
    spec(Context::Help, "j, down", ActionId::ScrollDown),
    spec(Context::Help, "k, up", ActionId::ScrollUp),
    spec(Context::Help, "pagedown", ActionId::PageDown),
    spec(Context::Help, "pageup", ActionId::PageUp),
];

/// The card is inline, not modal: only the answer keys live here and
/// everything else falls through to the conversation underneath.
const PERMISSION: &[BindingSpec] = &[
    spec(Context::Permission, "y", ActionId::PermissionAllow),
    spec(Context::Permission, "n", ActionId::PermissionDeny),
    spec(Context::Permission, "A", ActionId::PermissionAllowAlways),
];

const ASK: &[BindingSpec] = &[
    spec(Context::Ask, "j, down", ActionId::AskNextOption),
    spec(Context::Ask, "k, up", ActionId::AskPrevOption),
    spec(Context::Ask, "1-9", ActionId::AskPickIndex),
    spec(Context::Ask, "space", ActionId::AskToggleOption),
    spec(Context::Ask, "tab", ActionId::AskNextQuestion),
    spec(Context::Ask, "backtab, shift+backtab, shift+tab", ActionId::AskPrevQuestion),
    spec(Context::Ask, "o", ActionId::AskEditOther),
    spec(Context::Ask, "enter", ActionId::AskSubmit),
    spec(Context::Ask, "esc", ActionId::PromptDefer),
];

const ASK_TEXT: &[BindingSpec] = &[
    spec(Context::AskText, "enter", ActionId::PromptTextCommit),
    spec(Context::AskText, "esc", ActionId::PromptTextCancel),
];

const PLAN: &[BindingSpec] = &[
    spec(Context::Plan, "1", ActionId::PlanApproveAuto),
    spec(Context::Plan, "2", ActionId::PlanApproveManual),
    spec(Context::Plan, "3", ActionId::PlanKeepPlanning),
    spec(Context::Plan, "r", ActionId::PlanRefine),
    spec(Context::Plan, "j, down", ActionId::PlanScrollDown),
    spec(Context::Plan, "k, up", ActionId::PlanScrollUp),
    spec(Context::Plan, "esc", ActionId::PromptDefer),
];

const PLAN_TEXT: &[BindingSpec] = &[
    spec(Context::PlanText, "enter", ActionId::PromptTextCommit),
    spec(Context::PlanText, "esc", ActionId::PromptTextCancel),
];

/// Where a feature registers its default bindings: add one slice here.
pub const DEFAULT_BINDINGS: &[&[BindingSpec]] = &[
    GLOBAL,
    SESSION_LIST,
    CONVERSATION,
    COMPOSER,
    HISTORY,
    TERMINAL,
    HELP,
    PERMISSION,
    DIAGNOSE,
    ASK,
    ASK_TEXT,
    PLAN,
    PLAN_TEXT,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: HashMap<(Context, Chord), ActionId>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = Self { bindings: HashMap::new() };
        let mut problems = Vec::new();
        map.load_defaults(&mut problems);
        debug_assert!(problems.is_empty(), "built-in keymap: {problems:?}");
        map
    }
}

impl Keymap {
    pub fn new(problems: &mut Vec<String>) -> Self {
        let mut map = Self { bindings: HashMap::new() };
        map.load_defaults(problems);
        map
    }

    fn load_defaults(&mut self, problems: &mut Vec<String>) {
        for group in DEFAULT_BINDINGS {
            for spec in *group {
                let chords = match Chord::parse_list(spec.keys) {
                    Ok(chords) => chords,
                    Err(err) => {
                        problems.push(format!("default binding for `{}`: {err}", spec.action));
                        continue;
                    }
                };
                for chord in chords {
                    if let Some(existing) = self.bindings.get(&(spec.context, chord))
                        && *existing != spec.action
                    {
                        problems.push(format!(
                            "`{chord}` in [{}] is bound to both `{existing}` and `{}`",
                            spec.context, spec.action
                        ));
                        continue;
                    }
                    self.bindings.insert((spec.context, chord), spec.action);
                }
            }
        }
    }

    /// One user entry, merged over the defaults. `none` unbinds.
    pub fn set(&mut self, context: Context, keys: &str, action: &str) -> Result<(), String> {
        let chords = Chord::parse_list(keys)?;
        let action = if action == "none" {
            None
        } else {
            Some(
                ActionId::parse(action)
                    .ok_or_else(|| format!("`{action}` is not a known action"))?,
            )
        };
        for chord in chords {
            match action {
                Some(action) => self.bindings.insert((context, chord), action),
                None => self.bindings.remove(&(context, chord)),
            };
        }
        Ok(())
    }

    /// The context's own binding, then its fallbacks. A permission card sits
    /// inside a conversation, so it inherits both; the composer swallows typed
    /// characters and inherits nothing.
    pub fn lookup(&self, context: Context, chord: Chord) -> Option<ActionId> {
        std::iter::once(context)
            .chain(Self::fallbacks(context).iter().copied())
            .find_map(|c| self.bindings.get(&(c, chord)).copied())
    }

    const fn fallbacks(context: Context) -> &'static [Context] {
        match context {
            Context::Permission => &[Context::Conversation, Context::Global],
            Context::SessionList | Context::Conversation | Context::Help => &[Context::Global],
            _ => &[],
        }
    }

    /// Plain characters first, then named keys: a `HashMap` has no order of its
    /// own and the cheat sheet must not shuffle between runs.
    pub fn chords_for(&self, context: Context, action: ActionId) -> Vec<Chord> {
        let mut chords: Vec<Chord> = self
            .bindings
            .iter()
            .filter(|((c, _), a)| *c == context && **a == action)
            .map(|((_, chord), _)| *chord)
            .collect();
        chords.sort_by_key(|c| {
            let named = u8::from(!matches!(c.code, crossterm::event::KeyCode::Char(_)));
            (named, c.to_string())
        });
        chords
    }

    /// Bound actions of a context, in declaration order, each with its chords.
    pub fn entries(&self, context: Context) -> Vec<(ActionId, Vec<Chord>)> {
        ACTION_IDS
            .iter()
            .copied()
            .map(|action| (action, self.chords_for(context, action)))
            .filter(|(_, chords)| !chords.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::{ActionId, Chord, Context, Keymap};

    fn chord(text: &str) -> Chord {
        Chord::parse(text).expect("parses")
    }

    #[test]
    fn the_built_in_table_has_no_conflicts() {
        let mut problems = Vec::new();
        let _ = Keymap::new(&mut problems);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_context_binding_wins_over_the_global_one() {
        let map = Keymap::default();
        assert_eq!(
            map.lookup(Context::Conversation, chord("q")),
            Some(ActionId::LeaveConversation)
        );
        assert_eq!(map.lookup(Context::SessionList, chord("q")), Some(ActionId::Quit));
        assert_eq!(map.lookup(Context::Conversation, chord("1")), Some(ActionId::SelectIndex));
        assert_eq!(map.lookup(Context::SessionList, chord("1")), Some(ActionId::SwitchView));
    }

    #[test]
    fn a_permission_card_answers_first_and_inherits_the_conversation() {
        let map = Keymap::default();
        assert_eq!(map.lookup(Context::Permission, chord("y")), Some(ActionId::PermissionAllow));
        assert_eq!(map.lookup(Context::Permission, chord("n")), Some(ActionId::PermissionDeny));
        assert_eq!(
            map.lookup(Context::Permission, chord("A")),
            Some(ActionId::PermissionAllowAlways)
        );
        assert_eq!(map.lookup(Context::Permission, chord("j")), Some(ActionId::ScrollDown));
        let leave = Some(ActionId::LeaveConversation);
        assert_eq!(map.lookup(Context::Permission, chord("esc")), leave);
        assert_eq!(map.lookup(Context::Permission, chord("?")), Some(ActionId::Help));
    }

    #[test]
    fn the_composer_never_falls_through_to_a_global() {
        let map = Keymap::default();
        assert_eq!(map.lookup(Context::Composer, chord("?")), None);
        assert_eq!(map.lookup(Context::Composer, chord("esc")), Some(ActionId::CancelInput));
    }

    #[test]
    fn a_user_entry_overrides_and_can_unbind() {
        let mut map = Keymap::default();
        map.set(Context::SessionList, "x", "select-next").expect("valid");
        assert_eq!(map.lookup(Context::SessionList, chord("x")), Some(ActionId::SelectNext));
        map.set(Context::SessionList, "j", "none").expect("valid");
        assert_eq!(map.lookup(Context::SessionList, chord("j")), None);
    }

    #[test]
    fn an_unknown_action_or_key_is_an_error() {
        let mut map = Keymap::default();
        assert!(map.set(Context::Global, "x", "no-such-action").is_err());
        assert!(map.set(Context::Global, "hyper+x", "quit").is_err());
    }

    #[test]
    fn shift_is_dropped_from_character_chords_on_both_sides() {
        let map = Keymap::default();
        let shifted = Chord::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(map.lookup(Context::SessionList, shifted), None);
        assert_eq!(map.lookup(Context::SessionList, chord("G")), Some(ActionId::SelectLast));
    }

    #[test]
    fn entries_are_grouped_by_action_in_declaration_order() {
        let map = Keymap::default();
        let entries = map.entries(Context::SessionList);
        let (first, chords) = &entries[0];
        assert_eq!(*first, ActionId::SelectNext);
        assert_eq!(chords.len(), 2);
    }
}
