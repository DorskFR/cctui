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
    /// The new-session dialog.
    Spawn,
    /// The `f` sections popup over the list.
    Sections,
    /// The `/` prompt over the list.
    ListSearch,
    /// The `/` search and `:` command prompt.
    CmdLine,
    /// The `F` category menu.
    FilterMenu,
    /// A y/N prompt over the list: kill, or a batch archive.
    Confirm,
    /// The inline rename field on a list row.
    Rename,
    History,
    FileViewer,
    LabelPicker,
    LabelFilter,
    Pins,
    Macros,
    BookmarkPrompt,
    BookmarkConfirm,
    DraftEnv,
    DraftConfirm,
    SpawnProfileName,
    SpawnProfileConfirm,
    Terminal,
    Help,
    ModelPicker,
    Sidebar,
    Permission,
    Diagnose,
    Bookmarks,
    Overview,
    Ask,
    AskText,
    Plan,
    PlanText,
}

pub const CONTEXTS: &[Context] = &[
    Context::Global,
    Context::SessionList,
    Context::Confirm,
    Context::Rename,
    Context::Conversation,
    Context::Composer,
    Context::Spawn,
    Context::Sections,
    Context::ListSearch,
    Context::CmdLine,
    Context::FilterMenu,
    Context::History,
    Context::FileViewer,
    Context::LabelPicker,
    Context::LabelFilter,
    Context::Pins,
    Context::Macros,
    Context::BookmarkPrompt,
    Context::BookmarkConfirm,
    Context::DraftEnv,
    Context::DraftConfirm,
    Context::SpawnProfileName,
    Context::SpawnProfileConfirm,
    Context::Terminal,
    Context::Help,
    Context::ModelPicker,
    Context::Sidebar,
    Context::Permission,
    Context::Diagnose,
    Context::Bookmarks,
    Context::Overview,
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
            Self::Spawn => "spawn",
            Self::Sections => "sections",
            Self::ListSearch => "list-search",
            Self::CmdLine => "cmdline",
            Self::Confirm => "confirm",
            Self::Rename => "rename",
            Self::FilterMenu => "filter-menu",
            Self::History => "history",
            Self::FileViewer => "file-viewer",
            Self::LabelPicker => "label-picker",
            Self::LabelFilter => "label-filter",
            Self::Pins => "pins",
            Self::Macros => "macros",
            Self::BookmarkPrompt => "bookmark-prompt",
            Self::BookmarkConfirm => "bookmark-confirm",
            Self::DraftEnv => "draft-env",
            Self::DraftConfirm => "draft-confirm",
            Self::SpawnProfileName => "spawn-profile-name",
            Self::SpawnProfileConfirm => "spawn-profile-confirm",
            Self::Terminal => "terminal",
            Self::Help => "help",
            Self::ModelPicker => "model-picker",
            Self::Sidebar => "sidebar",
            Self::Permission => "permission",
            Self::Diagnose => "diagnose",
            Self::Bookmarks => "bookmarks",
            Self::Overview => "overview",
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
            Self::Spawn => "New session",
            Self::Sections => "Sections popup",
            Self::ListSearch => "List search",
            Self::CmdLine => "Search and commands",
            Self::Confirm => "Confirm prompt",
            Self::Rename => "Rename a session",
            Self::FilterMenu => "Filter menu",
            Self::History => "Prompt history",
            Self::FileViewer => "File viewer",
            Self::LabelPicker => "Labels",
            Self::LabelFilter => "Label filter",
            Self::Pins => "Pinned messages",
            Self::Macros => "Macros",
            Self::BookmarkPrompt => "Bookmarks — search or edit",
            Self::BookmarkConfirm => "Bookmarks — confirm",
            Self::DraftEnv => "Draft launch — env",
            Self::DraftConfirm => "Draft — confirm",
            Self::SpawnProfileName => "Profile — name",
            Self::SpawnProfileConfirm => "Profile — confirm",
            Self::Terminal => "Terminal pane",
            Self::Help => "Help",
            Self::ModelPicker => "Model picker",
            Self::Sidebar => "Sidebar",
            Self::Permission => "Permission card",
            Self::Diagnose => "Diagnose / info",
            Self::Bookmarks => "Bookmarks",
            Self::Overview => "Overview",
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
    SpawnOpen => "spawn-open", "Start a new session";
    SpawnNextField => "spawn-next-field", "Next field";
    SpawnPrevField => "spawn-prev-field", "Previous field";
    SpawnSubmit => "spawn-submit", "Launch";
    SpawnCancel => "spawn-cancel", "Close without launching";
    ListSections => "list-sections", "Choose which sections show";
    ListSortCycle => "list-sort", "Cycle the sort field";
    ListSortFlip => "list-sort-flip", "Flip the sort direction";
    ListGroupCycle => "list-group", "Cycle what rows group by";
    ListColorCycle => "list-color", "Cycle the row accent dimension";
    SectionsNext => "sections-next", "Next section";
    SectionsPrev => "sections-prev", "Previous section";
    SectionsToggle => "sections-toggle", "Show or hide this section";
    ListSearchComplete => "list-search-complete", "Complete the field or value";
    ListSearchCommit => "list-search-commit", "Open the result";
    ListSearchCancel => "list-search-cancel", "Clear the search";
    ListSearchArchived => "list-search-archived", "Include archived sessions";
    ListSearchMore => "list-search-more", "Load more results";
    ToggleUnreadOnly => "toggle-unread-only", "Show only unread sessions";
    TogglePin => "toggle-pin", "Pin or unpin the session";
    NewSession => "new-session", "Spawn a session";
    Archive => "archive", "Archive or unarchive the session";
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
    ModelPicker => "model-picker", "Change model and effort";
    ToggleSidebar => "toggle-sidebar", "Todo and subagent sidebar";
    SidebarNext => "sidebar-next", "Next subagent";
    SidebarPrev => "sidebar-prev", "Previous subagent";
    SidebarOpen => "sidebar-open", "Open the subagent";
    SidebarClose => "sidebar-close", "Close the sidebar";
    OpenParent => "open-parent", "Go to the parent session";
    PickerClose => "picker-close", "Close the picker";
    PickerNext => "picker-next", "Next entry";
    PickerPrev => "picker-prev", "Previous entry";
    PickerModelColumn => "picker-model-column", "Focus the model list";
    PickerEffortColumn => "picker-effort-column", "Focus the effort list";
    PickerApply => "picker-apply", "Apply model and effort";
    ToggleAutoApprove => "toggle-auto-approve", "Toggle auto-approve";
    LineCursor => "line-cursor", "Select transcript lines";
    OpenLabels => "open-labels", "Label this session";
    OpenLabelFilter => "open-label-filter", "Filter by label";
    LabelsClose => "labels-close", "Close the label list";
    LabelsNext => "labels-next", "Next label";
    LabelsPrev => "labels-prev", "Previous label";
    LabelsToggle => "labels-toggle", "Attach or detach this label";
    LabelsCreate => "labels-create", "Create a label";
    LabelsEdit => "labels-edit", "Rename or recolor";
    LabelsDelete => "labels-delete", "Delete this label";
    LabelsCommit => "labels-commit", "Confirm";
    LabelsCancel => "labels-cancel", "Back";
    LabelFilterToggle => "label-filter-toggle", "Include or exclude this label";
    LabelFilterClear => "label-filter-clear", "Show every label again";
    AttachFile => "attach-file", "Attach a file";
    RemoveAttachment => "remove-attachment", "Remove the focused attachment";
    FocusAttachments => "focus-attachments", "Focus the attachment chips";
    AttachmentNext => "attachment-next", "Next attachment chip";
    AttachmentPrev => "attachment-prev", "Previous attachment chip";
    OpenLinkedFile => "open-linked-file", "Open the file under the cursor";
    FileViewerClose => "file-viewer-close", "Close the file viewer";
    FileViewerOsOpen => "file-viewer-os-open", "Open in the desktop viewer";
    ToggleExpand => "toggle-expand", "Expand the focused line";
    ToggleExpandAll => "toggle-expand-all", "Expand every thinking and result block";
    RetrySend => "retry-send", "Retry the undelivered message";
    EditSend => "edit-send", "Edit the undelivered message";
    DiscardSend => "discard-send", "Drop the undelivered message";
    CopyMessage => "copy-message", "Copy the focused line as Markdown";
    CopyCodeBlock => "copy-code-block", "Copy the code under the cursor";
    CopySessionLink => "copy-session-link", "Copy a link to this session";
    Command => "command", "Run a command (:export)";
    FilterCycle => "filter-cycle", "Cycle assistant / you / tools";
    FilterMenu => "filter-menu", "Choose which lines to show";
    FilterShowAll => "filter-show-all", "Show every category";
    FilterReset => "filter-reset", "Back to the default filter";
    FilterMenuToggle => "filter-menu-toggle", "Show or hide this category";
    FilterMenuNext => "filter-menu-next", "Next category";
    FilterMenuPrev => "filter-menu-prev", "Previous category";
    CmdLineCommit => "cmdline-commit", "Run it";
    CmdLineCancel => "cmdline-cancel", "Abandon it";
    CmdLineComplete => "cmdline-complete", "Complete the path";
    OpenInEditor => "open-in-editor", "Compose in $EDITOR";
    RenameSession => "rename-session", "Rename the session";
    ArchiveSection => "archive-section", "Archive every session in the section";
    KillSession => "kill-session", "Kill the session";
    UndoArchive => "undo-archive", "Undo the last archive";
    SelectToggle => "select-toggle", "Select or deselect the row";
    SelectRange => "select-range", "Select up to the anchor";
    SelectAll => "select-all", "Select every visible row";
    SelectClear => "select-clear", "Leave select mode";
    ConfirmYes => "confirm-yes", "Yes";
    ConfirmNo => "confirm-no", "No";
    RenameCommit => "rename-commit", "Save the name";
    RenameCancel => "rename-cancel", "Discard the name";
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

    PinToggle => "pin-toggle", "Pin or unpin the focused line";
    PinsOpen => "pins-open", "Open the pinned messages";
    PinsClose => "pins-close", "Close the pinned messages";
    PinsSelectNext => "pins-select-next", "Next pin";
    PinsSelectPrev => "pins-select-prev", "Previous pin";
    PinsJump => "pins-jump", "Jump to the pinned message";
    PinsUnpin => "pins-unpin", "Unpin this message";

    BookmarkLine => "bookmark-line", "Save the focused message as a bookmark";
    BookmarksSelectNext => "bookmarks-select-next", "Next bookmark";
    BookmarksSelectPrev => "bookmarks-select-prev", "Previous bookmark";
    BookmarksSelectFirst => "bookmarks-select-first", "First bookmark";
    BookmarksSelectLast => "bookmarks-select-last", "Last bookmark";
    BookmarksPreviewDown => "bookmarks-preview-down", "Scroll the preview down";
    BookmarksPreviewUp => "bookmarks-preview-up", "Scroll the preview up";
    BookmarksSearch => "bookmarks-search", "Search the bookmarks";
    DraftLaunch => "draft-launch", "Launch the draft";
    DraftEdit => "draft-edit", "Edit the draft";
    DraftDiscard => "draft-discard", "Discard the draft";
    SpawnFromConfig => "spawn-from-config", "New session from this configuration";
    ProfileNameCommit => "profile-name-commit", "Save the profile";
    ProfileNameCancel => "profile-name-cancel", "Do not save";
    ProfileDeleteConfirm => "profile-delete-confirm", "Delete the profile";
    ProfileDeleteCancel => "profile-delete-cancel", "Keep the profile";
    DraftEnvNext => "draft-env-next", "Next variable";
    DraftEnvCommit => "draft-env-commit", "Launch with these values";
    DraftEnvCancel => "draft-env-cancel", "Do not launch";
    DraftDiscardConfirm => "draft-discard-confirm", "Discard it";
    DraftDiscardCancel => "draft-discard-cancel", "Keep it";
    BookmarksOpenSource => "bookmarks-open-source", "Open the source message";
    BookmarksCopy => "bookmarks-copy", "Copy the bookmark as markdown";
    BookmarksEdit => "bookmarks-edit", "Edit the title and note";
    BookmarksDelete => "bookmarks-delete", "Delete the bookmark";
    BookmarksPromptCommit => "bookmarks-prompt-commit", "Accept";
    BookmarksPromptCancel => "bookmarks-prompt-cancel", "Discard";
    BookmarksPromptSwitch => "bookmarks-prompt-switch", "Switch between title and note";
    BookmarksDeleteConfirm => "bookmarks-delete-confirm", "Delete it";
    BookmarksDeleteCancel => "bookmarks-delete-cancel", "Keep it";

    MentionAccept => "mention-accept", "Take the session completion";
    MacrosOpen => "macros-open", "Insert a canned prompt";
    MacrosClose => "macros-close", "Close the macro list";
    MacrosSelectNext => "macros-select-next", "Next macro";
    MacrosSelectPrev => "macros-select-prev", "Previous macro";
    MacrosInsert => "macros-insert", "Put this prompt in the composer";
    MacrosRun => "macros-run", "Run this macro as a new session";

    CloseHelp => "close-help", "Close this cheat sheet";

    OverviewScrollDown => "overview-scroll-down", "Scroll the overview down";
    OverviewScrollUp => "overview-scroll-up", "Scroll the overview up";
    OverviewRefresh => "overview-refresh", "Refresh the counts";

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
    JumpToAttention => "jump-to-attention", "Jump to the next session needing input";

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
    spec(Context::Global, "ctrl+g", ActionId::JumpToAttention),
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
    // The ticket asks for `n`, but decision 7 makes `n` next-search-hit
    // everywhere and a context binding would silently shadow it.
    spec(Context::SessionList, "ctrl+n", ActionId::SpawnOpen),
    spec(Context::SessionList, "f", ActionId::ListSections),
    spec(Context::SessionList, "o", ActionId::ListSortCycle),
    spec(Context::SessionList, "O", ActionId::ListSortFlip),
    spec(Context::SessionList, "v", ActionId::ListGroupCycle),
    // `V` is the range-select anchor; the accent dimension takes `c`.
    spec(Context::SessionList, "c", ActionId::ListColorCycle),
    spec(Context::SessionList, "U", ActionId::ToggleUnreadOnly),
    spec(Context::SessionList, "p", ActionId::TogglePin),
    spec(Context::SessionList, "r", ActionId::RenameSession),
    spec(Context::SessionList, "x", ActionId::Archive),
    spec(Context::SessionList, "X", ActionId::KillSession),
    spec(Context::SessionList, "A", ActionId::ArchiveSection),
    spec(Context::SessionList, "u", ActionId::UndoArchive),
    spec(Context::SessionList, "space", ActionId::SelectToggle),
    spec(Context::SessionList, "V", ActionId::SelectRange),
    spec(Context::SessionList, "*", ActionId::SelectAll),
    spec(Context::SessionList, "esc", ActionId::SelectClear),
];

const CONFIRM: &[BindingSpec] = &[
    spec(Context::Confirm, "y, Y", ActionId::ConfirmYes),
    spec(Context::Confirm, "n, N, esc, q", ActionId::ConfirmNo),
];

const RENAME: &[BindingSpec] = &[
    spec(Context::Rename, "enter", ActionId::RenameCommit),
    spec(Context::Rename, "esc", ActionId::RenameCancel),
];

/// One prompt serves the bookmark search and the bookmark edit: `Tab` is only
/// meaningful for the edit, which types a title and a note.
const BOOKMARK_PROMPT: &[BindingSpec] = &[
    spec(Context::BookmarkPrompt, "enter", ActionId::BookmarksPromptCommit),
    spec(Context::BookmarkPrompt, "esc", ActionId::BookmarksPromptCancel),
    spec(Context::BookmarkPrompt, "tab", ActionId::BookmarksPromptSwitch),
];

const BOOKMARK_CONFIRM: &[BindingSpec] = &[
    spec(Context::BookmarkConfirm, "y, enter", ActionId::BookmarksDeleteConfirm),
    spec(Context::BookmarkConfirm, "n, esc, q", ActionId::BookmarksDeleteCancel),
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
    spec(Context::Conversation, "M", ActionId::ModelPicker),
    spec(Context::Conversation, ">", ActionId::ToggleSidebar),
    spec(Context::Conversation, "u", ActionId::OpenParent),
    spec(Context::Conversation, "ctrl+f", ActionId::Fork),
    spec(Context::Conversation, "ctrl+a", ActionId::ToggleAutoApprove),
    spec(Context::Conversation, "ctrl+r", ActionId::HistoryOpen),
    spec(Context::Conversation, "y", ActionId::CopyMessage),
    spec(Context::Conversation, "Y", ActionId::CopyCodeBlock),
    spec(Context::Conversation, "ctrl+y", ActionId::CopySessionLink),
    spec(Context::Conversation, ":", ActionId::Command),
    spec(Context::Conversation, "f", ActionId::FilterCycle),
    spec(Context::Conversation, "F", ActionId::FilterMenu),
    spec(Context::Conversation, "tab", ActionId::FocusPrompt),
    spec(Context::Conversation, "b", ActionId::BookmarkLine),
    spec(Context::Conversation, "m", ActionId::PinToggle),
    spec(Context::Conversation, "'", ActionId::PinsOpen),
    spec(Context::Conversation, "ctrl+t", ActionId::MacrosOpen),
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
    spec(Context::Composer, "ctrl+t", ActionId::MacrosOpen),
    spec(Context::Composer, "tab", ActionId::MentionAccept),
];

/// Typed characters reach the buffer through the unbound fall-through, so only
/// the two keys that end the prompt live here.
const CMDLINE: &[BindingSpec] = &[
    spec(Context::CmdLine, "enter", ActionId::CmdLineCommit),
    spec(Context::CmdLine, "esc", ActionId::CmdLineCancel),
];

const FILTER_MENU: &[BindingSpec] = &[
    spec(Context::FilterMenu, "esc, q, F", ActionId::FilterMenu),
    spec(Context::FilterMenu, "j, down", ActionId::FilterMenuNext),
    spec(Context::FilterMenu, "k, up", ActionId::FilterMenuPrev),
    spec(Context::FilterMenu, "space, enter", ActionId::FilterMenuToggle),
    spec(Context::FilterMenu, "a", ActionId::FilterShowAll),
    spec(Context::FilterMenu, "r", ActionId::FilterReset),
];

/// Typed characters reach the focused field through the unbound fall-through.
const SPAWN: &[BindingSpec] = &[
    spec(Context::Spawn, "tab", ActionId::SpawnNextField),
    spec(Context::Spawn, "backtab, shift+tab", ActionId::SpawnPrevField),
    spec(Context::Spawn, "ctrl+s", ActionId::SpawnSubmit),
    spec(Context::Spawn, "esc", ActionId::SpawnCancel),
];

const SECTIONS_POPUP: &[BindingSpec] = &[
    spec(Context::Sections, "esc, q, f", ActionId::ListSections),
    spec(Context::Sections, "j, down", ActionId::SectionsNext),
    spec(Context::Sections, "k, up", ActionId::SectionsPrev),
    spec(Context::Sections, "space, enter", ActionId::SectionsToggle),
];

/// Typed characters reach the query through the unbound fall-through.
const LIST_SEARCH: &[BindingSpec] = &[
    spec(Context::ListSearch, "tab", ActionId::ListSearchComplete),
    spec(Context::ListSearch, "enter", ActionId::ListSearchCommit),
    spec(Context::ListSearch, "esc", ActionId::ListSearchCancel),
    spec(Context::ListSearch, "ctrl+a", ActionId::ListSearchArchived),
    spec(Context::ListSearch, "ctrl+n", ActionId::SearchNext),
    spec(Context::ListSearch, "ctrl+p", ActionId::SearchPrev),
    spec(Context::ListSearch, "ctrl+m", ActionId::ListSearchMore),
];

const HISTORY: &[BindingSpec] = &[
    spec(Context::History, "esc", ActionId::HistoryClose),
    spec(Context::History, "down, ctrl+n", ActionId::HistorySelectNext),
    spec(Context::History, "up, ctrl+p", ActionId::HistorySelectPrev),
    spec(Context::History, "enter", ActionId::HistoryRecall),
];

/// `Ctrl-O` opens L3's command line with `attach ` already typed, so there is
/// one command line rather than a second prompt beside it.
const ATTACH: &[BindingSpec] = &[
    spec(Context::Composer, "ctrl+o", ActionId::AttachFile),
    spec(Context::Conversation, "ctrl+o", ActionId::AttachFile),
    spec(Context::Composer, "alt+backspace", ActionId::FocusAttachments),
    spec(Context::CmdLine, "tab", ActionId::CmdLineComplete),
];

/// The chip row only has focus while a chip is selected, which is why these live
/// in the composer context rather than one of their own.
const CHIPS: &[BindingSpec] = &[
    spec(Context::Composer, "backspace", ActionId::RemoveAttachment),
    spec(Context::Composer, "left", ActionId::AttachmentPrev),
    spec(Context::Composer, "right", ActionId::AttachmentNext),
];

const FILE_VIEWER: &[BindingSpec] = &[
    spec(Context::Conversation, "g f", ActionId::OpenLinkedFile),
    spec(Context::FileViewer, "esc, q", ActionId::FileViewerClose),
    spec(Context::FileViewer, "j, down", ActionId::ScrollDown),
    spec(Context::FileViewer, "k, up", ActionId::ScrollUp),
    spec(Context::FileViewer, "pagedown", ActionId::PageDown),
    spec(Context::FileViewer, "pageup", ActionId::PageUp),
    spec(Context::FileViewer, "g", ActionId::ScrollToTop),
    spec(Context::FileViewer, "o", ActionId::FileViewerOsOpen),
];

/// The picker is modal, so it claims plain letters: `space` toggles, and the
/// manage verbs sit on the keys the webui's menu uses.
const LABELS: &[BindingSpec] = &[
    spec(Context::SessionList, "s", ActionId::DraftLaunch),
    spec(Context::SessionList, "E", ActionId::DraftEdit),
    spec(Context::SessionList, "d", ActionId::DraftDiscard),
    spec(Context::SessionList, "C", ActionId::SpawnFromConfig),
    spec(Context::SessionList, "l", ActionId::OpenLabels),
    spec(Context::SessionList, "L", ActionId::OpenLabelFilter),
    spec(Context::LabelPicker, "esc", ActionId::LabelsClose),
    spec(Context::LabelPicker, "down, ctrl+n", ActionId::LabelsNext),
    spec(Context::LabelPicker, "up, ctrl+p", ActionId::LabelsPrev),
    spec(Context::LabelPicker, "space", ActionId::LabelsToggle),
    spec(Context::LabelPicker, "ctrl+c", ActionId::LabelsCreate),
    spec(Context::LabelPicker, "ctrl+e", ActionId::LabelsEdit),
    spec(Context::LabelPicker, "ctrl+d", ActionId::LabelsDelete),
    spec(Context::LabelPicker, "enter", ActionId::LabelsCommit),
    spec(Context::LabelFilter, "esc", ActionId::LabelsClose),
    spec(Context::LabelFilter, "down, ctrl+n", ActionId::LabelsNext),
    spec(Context::LabelFilter, "up, ctrl+p", ActionId::LabelsPrev),
    spec(Context::LabelFilter, "space, enter", ActionId::LabelFilterToggle),
    spec(Context::LabelFilter, "a", ActionId::LabelFilterClear),
];

const PINS: &[BindingSpec] = &[
    spec(Context::Pins, "esc", ActionId::PinsClose),
    spec(Context::Pins, "down, ctrl+n", ActionId::PinsSelectNext),
    spec(Context::Pins, "up, ctrl+p", ActionId::PinsSelectPrev),
    spec(Context::Pins, "enter", ActionId::PinsJump),
    spec(Context::Pins, "m, d", ActionId::PinsUnpin),
];

const MACROS: &[BindingSpec] = &[
    spec(Context::Macros, "esc", ActionId::MacrosClose),
    spec(Context::Macros, "down, ctrl+n", ActionId::MacrosSelectNext),
    spec(Context::Macros, "up, ctrl+p", ActionId::MacrosSelectPrev),
    spec(Context::Macros, "enter", ActionId::MacrosInsert),
    spec(Context::Macros, "R", ActionId::MacrosRun),
];

/// Slice roots, not overlays: each falls through to the globals, so `1-9`,
/// `?` and `q` keep working from them.
const BOOKMARKS: &[BindingSpec] = &[
    spec(Context::Bookmarks, "j, down", ActionId::BookmarksSelectNext),
    spec(Context::Bookmarks, "k, up", ActionId::BookmarksSelectPrev),
    spec(Context::Bookmarks, "g", ActionId::BookmarksSelectFirst),
    spec(Context::Bookmarks, "G", ActionId::BookmarksSelectLast),
    spec(Context::Bookmarks, "pagedown, ctrl+f", ActionId::BookmarksPreviewDown),
    spec(Context::Bookmarks, "pageup, ctrl+b", ActionId::BookmarksPreviewUp),
    spec(Context::Bookmarks, "/", ActionId::BookmarksSearch),
    spec(Context::Bookmarks, "enter", ActionId::BookmarksOpenSource),
    spec(Context::Bookmarks, "y", ActionId::BookmarksCopy),
    spec(Context::Bookmarks, "e", ActionId::BookmarksEdit),
    spec(Context::Bookmarks, "d", ActionId::BookmarksDelete),
];

const OVERVIEW: &[BindingSpec] = &[
    spec(Context::Overview, "j, down", ActionId::OverviewScrollDown),
    spec(Context::Overview, "k, up", ActionId::OverviewScrollUp),
    spec(Context::Overview, "r", ActionId::OverviewRefresh),
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

const SPAWN_PROFILE_NAME: &[BindingSpec] = &[
    spec(Context::SpawnProfileName, "enter", ActionId::ProfileNameCommit),
    spec(Context::SpawnProfileName, "esc", ActionId::ProfileNameCancel),
];

const SPAWN_PROFILE_CONFIRM: &[BindingSpec] = &[
    spec(Context::SpawnProfileConfirm, "y, enter", ActionId::ProfileDeleteConfirm),
    spec(Context::SpawnProfileConfirm, "n, esc, q", ActionId::ProfileDeleteCancel),
];

const DRAFT_ENV: &[BindingSpec] = &[
    spec(Context::DraftEnv, "tab", ActionId::DraftEnvNext),
    spec(Context::DraftEnv, "enter", ActionId::DraftEnvCommit),
    spec(Context::DraftEnv, "esc", ActionId::DraftEnvCancel),
];

const DRAFT_CONFIRM: &[BindingSpec] = &[
    spec(Context::DraftConfirm, "y, enter", ActionId::DraftDiscardConfirm),
    spec(Context::DraftConfirm, "n, esc, q", ActionId::DraftDiscardCancel),
];

const HELP: &[BindingSpec] = &[
    spec(Context::Help, "esc, q, ?", ActionId::CloseHelp),
    spec(Context::Help, "j, down", ActionId::ScrollDown),
    spec(Context::Help, "k, up", ActionId::ScrollUp),
    spec(Context::Help, "pagedown", ActionId::PageDown),
    spec(Context::Help, "pageup", ActionId::PageUp),
];

/// Focused but not modal: it claims its own keys and leaves the rest to the
/// conversation, so the transcript still scrolls with the panel up.
const SIDEBAR: &[BindingSpec] = &[
    spec(Context::Sidebar, "esc, >", ActionId::SidebarClose),
    spec(Context::Sidebar, "j, down", ActionId::SidebarNext),
    spec(Context::Sidebar, "k, up", ActionId::SidebarPrev),
    spec(Context::Sidebar, "enter", ActionId::SidebarOpen),
    spec(Context::Sidebar, "u", ActionId::OpenParent),
];

/// Modal over the conversation: everything it does not claim stays claimed,
/// so a stray key cannot type into the composer behind it.
const MODEL_PICKER: &[BindingSpec] = &[
    spec(Context::ModelPicker, "esc, q", ActionId::PickerClose),
    spec(Context::ModelPicker, "j, down", ActionId::PickerNext),
    spec(Context::ModelPicker, "k, up", ActionId::PickerPrev),
    spec(Context::ModelPicker, "h, left", ActionId::PickerModelColumn),
    spec(Context::ModelPicker, "l, right, tab", ActionId::PickerEffortColumn),
    spec(Context::ModelPicker, "enter", ActionId::PickerApply),
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
    SPAWN,
    SECTIONS_POPUP,
    LIST_SEARCH,
    CONFIRM,
    RENAME,
    CONVERSATION,
    COMPOSER,
    ATTACH,
    CHIPS,
    FILE_VIEWER,
    CMDLINE,
    FILTER_MENU,
    HISTORY,
    LABELS,
    PINS,
    MACROS,
    BOOKMARK_PROMPT,
    BOOKMARK_CONFIRM,
    DRAFT_ENV,
    DRAFT_CONFIRM,
    SPAWN_PROFILE_NAME,
    SPAWN_PROFILE_CONFIRM,
    TERMINAL,
    HELP,
    MODEL_PICKER,
    SIDEBAR,
    PERMISSION,
    DIAGNOSE,
    BOOKMARKS,
    OVERVIEW,
    ASK,
    ASK_TEXT,
    PLAN,
    PLAN_TEXT,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: HashMap<(Context, Chord), ActionId>,
    /// Two-chord bindings, written `"g f"` in a spec. A chord that only ever
    /// leads a sequence resolves to nothing on its own, which is what lets `g`
    /// prefix `gf` without shadowing anything bound to plain `g`.
    sequences: HashMap<(Context, Chord, Chord), ActionId>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = Self { bindings: HashMap::new(), sequences: HashMap::new() };
        let mut problems = Vec::new();
        map.load_defaults(&mut problems);
        debug_assert!(problems.is_empty(), "built-in keymap: {problems:?}");
        map
    }
}

impl Keymap {
    pub fn new(problems: &mut Vec<String>) -> Self {
        let mut map = Self { bindings: HashMap::new(), sequences: HashMap::new() };
        map.load_defaults(problems);
        map
    }

    fn load_defaults(&mut self, problems: &mut Vec<String>) {
        for group in DEFAULT_BINDINGS {
            for spec in *group {
                if let Some((lead, rest)) = spec.keys.split_once(' ')
                    && !rest.trim().is_empty()
                    && !lead.trim().is_empty()
                    && !spec.keys.contains(',')
                {
                    match (Chord::parse(lead.trim()), Chord::parse(rest.trim())) {
                        (Ok(lead), Ok(second)) => {
                            self.sequences.insert((spec.context, lead, second), spec.action);
                        }
                        _ => problems.push(format!(
                            "default sequence for `{}`: `{}` is not two chords",
                            spec.action, spec.keys
                        )),
                    }
                    continue;
                }
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

    /// Whether `chord` leads a two-chord binding in `context` (or a fallback),
    /// so the caller holds it and waits for the second key.
    #[must_use]
    pub fn is_prefix(&self, context: Context, chord: Chord) -> bool {
        std::iter::once(context)
            .chain(Self::fallbacks(context).iter().copied())
            .any(|c| self.sequences.keys().any(|(kc, lead, _)| *kc == c && *lead == chord))
    }

    /// The action a held `lead` plus `second` resolves to.
    #[must_use]
    pub fn lookup_sequence(
        &self,
        context: Context,
        lead: Chord,
        second: Chord,
    ) -> Option<ActionId> {
        std::iter::once(context)
            .chain(Self::fallbacks(context).iter().copied())
            .find_map(|c| self.sequences.get(&(c, lead, second)).copied())
    }

    const fn fallbacks(context: Context) -> &'static [Context] {
        match context {
            // The sidebar is focused but not modal: it claims its own keys and
            // leaves the rest to the transcript underneath.
            Context::Permission | Context::Sidebar => &[Context::Conversation, Context::Global],
            // The pager is a plain reader and keeps the globals; the attach
            // prompt swallows typed characters and falls through to nothing.
            Context::SessionList
            | Context::Conversation
            | Context::FileViewer
            | Context::Help
            | Context::Bookmarks
            | Context::Overview => &[Context::Global],
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

    /// Two-chord labels (`g f`) bound to `action` in `context`, for the cheat
    /// sheet: a sequence has no single [`Chord`] to report.
    #[must_use]
    pub fn sequence_labels(&self, context: Context, action: ActionId) -> Vec<String> {
        let mut out: Vec<String> = self
            .sequences
            .iter()
            .filter(|((c, _, _), a)| *c == context && **a == action)
            .map(|((_, lead, second), _)| format!("{} {}", lead.label(), second.label()))
            .collect();
        out.sort();
        out
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
