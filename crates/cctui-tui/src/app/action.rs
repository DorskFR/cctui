use cctui_proto::api::SessionListItem;
use cctui_proto::models::SessionStatus;
use crossterm::event::KeyEvent;

use super::attach::AttachAction;
use super::attention::AttentionAction;
use super::bookmarks::BookmarkAction;
use super::controls::ControlsAction;
use super::conversation::ConversationAction;
use super::conversation_store::{PageKind, PageRequest};
use super::deeplink::DeepLinkAction;
use super::diagnose::DiagnoseAction;
use super::dispatchers::DispatcherAction;
use super::drafts::DraftAction;
use super::fileview::FileViewAction;
use super::account_switch::AccountSwitchAction;
use super::harness_mode::HarnessModeAction;
use super::identity::AuthAction;
use super::images::ImagesAction;
use super::labels::LabelAction;
use super::accounts::AccountAction;
use super::machines::MachineAction;
use super::pools::PoolAction;
use super::macros::MacroAction;
use super::pins::PinAction;
use super::prompt::PromptAction;
use super::row_actions::RowAction;
use super::send::SendAction;
use super::session_live::SessionLiveAction;
use super::sidebar::SidebarAction;
use super::slice::SliceAction;
use super::state::ConversationLine;
use super::terminal::TerminalAction;
use super::toast::Level;
use super::unread::UnreadAction;

/// Everything that can change the app. Key handlers, the websocket and
/// completed effects all funnel through this one vocabulary.
pub enum Action {
    Quit,

    SelectNext,
    SelectPrev,
    SelectFirst,
    SelectLast,
    SelectIndex(usize),

    ToggleTimestamps,

    OpenHelp,
    CloseHelp,
    OpenSelectedConversation,
    LeaveConversation,

    /// `release_follow` mirrors the per-binding behaviour: scrolling up always
    /// detaches from the tail, scrolling down only does for line-wise keys.
    Scroll {
        lines: i32,
        release_follow: bool,
    },
    ScrollToTop,
    ScrollToBottom,

    /// A key that no navigation binding claimed: it both opens the composer
    /// and types its first character.
    ActivateInputWith(KeyEvent),
    CancelInput,
    InputKey(KeyEvent),
    InputNewline,
    SubmitInput,

    Controls(ControlsAction),
    Sidebar(SidebarAction),
    Unread(UnreadAction),
    ToggleAutoApproveSelected,
    AutoApproveSet {
        session_id: String,
        enabled: bool,
    },

    Attach(AttachAction),
    Images(ImagesAction),
    Access(super::admin::AccessAction),
    Instance(super::instance::InstanceAction),
    Labels(LabelAction),
    Machines(MachineAction),
    Accounts(AccountAction),
    Pools(PoolAction),
    Dispatchers(DispatcherAction),
    Usage(super::usage::UsageAction),
    Spend(super::spend::SpendAction),
    /// A lead chord of a two-chord binding is held; the next key completes it.
    PendingChord(crate::config::chord::Chord),
    /// A paste small enough to type straight into the composer.
    PasteText(String),
    Attention(AttentionAction),
    FileView(FileViewAction),

    RefreshSessions,
    SessionsLoaded(Vec<SessionListItem>),
    Conversation(ConversationAction),
    ListShape(super::list_shape_reduce::ListShapeAction),
    ListSearch(super::list_search::ListSearchAction),
    /// `/` and `n`/`N`: decision 7 scopes them to the view in front.
    SearchCurrentView,
    SearchHitNext,
    SearchHitPrev,
    CmdLine(super::cmdline::CmdAction),
    /// `y` / `Y` / the link key, all resolved against the focused line.
    Copy(CopyWhat),
    Prompt(PromptAction),
    Diagnose(DiagnoseAction),
    Slice(SliceAction),
    HarnessMode(HarnessModeAction),
    AccountSwitch(AccountSwitchAction),
    DeepLink(DeepLinkAction),

    StreamLine {
        session_id: String,
        seq: Option<i64>,
        /// `None` for an event with nothing to render; the `usage` beside it may
        /// still move the session row. Boxed: a line is far the largest payload
        /// in this enum.
        line: Option<Box<ConversationLine>>,
        usage: Option<HeartbeatUsage>,
    },
    SessionStatusChanged {
        session_id: String,
        status: SessionStatus,
    },
    SessionRegistered(Box<cctui_proto::models::Session>),
    SessionDeregistered(String),

    Auth(AuthAction),
    Drafts(DraftAction),
    Pins(PinAction),
    Bookmarks(BookmarkAction),
    Macros(MacroAction),
    /// Take the highlighted `#session` completion. Carries the key so a
    /// composer with no popup open still types it.
    AcceptMention(KeyEvent),
    Send(SendAction),
    RowAction(RowAction),
    Terminal(TerminalAction),
    SessionLive(SessionLiveAction),

    /// A pure clock advance: it moves delivery deadlines, re-evaluates the
    /// clock-derived row signals and polls the session list when one is due.
    Tick,

    Reconnected,
    Toast(Level, String),
    /// A websocket frame the TUI could not deserialize.
    UndecodableWsMessage(String),
    /// Persisted agent events the TUI could not deserialize.
    UndecodableAgentEvents(usize),
}

/// What a copy key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyWhat {
    /// The focused line as a Markdown block.
    Line,
    /// Just the code under the cursor.
    CodeBlock,
    /// A link to this session in the webui.
    SessionLink,
}

/// Token/cost figures a heartbeat carries for the session row.
pub struct HeartbeatUsage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

/// The only way the reducer reaches the network. Nothing here runs on the
/// key-handling path; the effects runner owns them.
pub enum Effect {
    RefreshSessions,
    /// `GET /me`: resolve the identity behind the configured key.
    FetchIdentity,
    /// `GET /permissions/pending`: the requests the server still holds, so a
    /// prompt raised before this client connected still gets a card.
    FetchPendingPermissions,
    LoadConversationPage {
        session_id: String,
        kind: PageKind,
        page: PageRequest,
        etag: Option<String>,
    },
    MarkSeen {
        session_id: String,
    },
    /// `OSC 52` first so a copy works over ssh, then the local clipboard.
    Copy {
        text: String,
        label: &'static str,
    },
    /// Refetches the whole transcript: the store holds rendered lines, and an
    /// export needs the events behind them.
    ExportConversation {
        session_id: String,
        meta: Box<super::export::Meta>,
        filter: Box<super::transcript_filter::Filter>,
        format: super::export::Format,
        path: std::path::PathBuf,
    },
    /// `GET /drafts`: every unsent draft, pulled once at startup.
    LoadDraftIndex,
    /// `GET /bookmarks`: one page, newest first, filtered by `q`.
    LoadBookmarks {
        q: String,
        before: Option<chrono::DateTime<chrono::Utc>>,
    },
    CreateBookmark {
        draft: Box<cctui_proto::api::bookmarks::CreateBookmark>,
    },
    UpdateBookmark {
        id: String,
        title: String,
        note: Option<String>,
    },
    DeleteBookmark {
        id: String,
    },
    /// `GET /sessions/{id}/pins`: the caller's pins in one session.
    LoadPins {
        session_id: String,
    },
    PinMessage {
        session_id: String,
        seq: i64,
    },
    UnpinMessage {
        session_id: String,
        seq: i64,
    },
    /// Re-read one session's draft and prompt history.
    LoadDrafts {
        session_id: String,
    },
    /// Save a draft, debounced per key. Empty text deletes it.
    SaveDraft {
        key: String,
        text: String,
    },
    /// Drop a draft now, cancelling any debounced save of it.
    DiscardDraft {
        key: String,
    },
    Subscribe {
        session_id: String,
    },
    Unsubscribe {
        session_id: String,
    },
    /// Start or stop the PTY relay for one session.
    WatchTerminal {
        session_id: String,
        watch: bool,
    },
    SendMessage {
        send_id: u64,
        session_id: String,
        content: String,
        /// 0-based option picks per question when the message answers a prompt.
        /// Replayed on every retry, so the daemon can still drive the real
        /// form after a frame the server never received.
        ask_picks: Option<Vec<Vec<usize>>>,
        /// Minted on the first attempt and replayed on every retry.
        turn_id: Option<uuid::Uuid>,
    },
    Interrupt {
        session_id: String,
    },
    /// One request for the whole batch; the server filters it to what the
    /// caller owns.
    ArchiveSessions {
        ids: Vec<String>,
        archived: bool,
    },
    PinSessions {
        ids: Vec<String>,
        pinned: bool,
    },
    RenameSession {
        session_id: String,
        name: String,
    },
    KillSession {
        session_id: String,
    },
    Fork {
        session_id: String,
    },
    /// `GET /models/{harness}`: the picker's model and effort lists.
    FetchHarnessModels {
        harness: String,
        machine_id: String,
        model: String,
    },
    SetModel {
        session_id: String,
        model: String,
        effort: String,
    },
    SetAutoApprove {
        session_id: String,
        enabled: bool,
    },
    RespondPermission {
        session_id: String,
        request_id: String,
        behavior: &'static str,
    },
    /// Read one local path into a composer attachment.
    ReadAttachment {
        session_id: String,
        path: String,
    },
    /// Stage the composer's files, then send `content` with its tokens rewritten.
    UploadAttachments {
        session_id: String,
        content: String,
        files: Vec<(String, Vec<u8>)>,
    },
    /// Read an agent-linked path for the file viewer.
    OpenLinkedFile {
        session_id: String,
        machine_id: String,
        path: String,
    },
    /// Read an image off the clipboard: a bracketed paste cannot carry one.
    ReadClipboardImage,
    /// Fetch an agent-posted image blob for the pager.
    FetchSessionImage {
        session_id: String,
        image_id: String,
    },
    /// Hand a staged attachment to the OS viewer.
    OpenInOsViewer {
        name: String,
        bytes: Vec<u8>,
    },
    /// `GET /sessions/{id}/diagnose`: everything the daemon and the server know.
    FetchDiagnose {
        session_id: String,
    },
    /// `GET /machines/resources`: the caller's daemon machines.
    FetchMachines,
    /// `GET /version`: what this deployment runs and what is available.
    FetchVersion,
    /// `POST /version/refresh`: probe upstream now.
    RefreshVersion,
    /// `GET /version/self-update`: the most recent update-hook run.
    FetchSelfUpdateRun,
    /// `GET /version/changelog`: notes for every release newer than this build.
    FetchChangelog,
    /// `POST /version/self-update`: deploy the newer release (admin).
    LaunchSelfUpdate,
    /// `GET /accounts`: the caller's account identities.
    FetchAccounts,
    /// `GET /redirects`: the live launch-time redirect rules.
    FetchRedirects,
    /// `GET /account-pools`: the pools with their membership.
    FetchAccountPools,
    UpdateAccount {
        id: String,
        request: Box<cctui_client::UpdateAccount>,
    },
    /// `POST /accounts/{provider_id}/limit-reset`.
    ClaimLimitReset {
        provider_id: String,
        credit_id: Option<String>,
    },
    PutRedirect {
        account_id: String,
        to_account: String,
        family: String,
    },
    DeleteRedirect {
        id: String,
    },
    CreatePool {
        request: Box<cctui_client::CreatePool>,
    },
    UpdatePool {
        id: String,
        request: Box<cctui_client::UpdatePool>,
    },
    DeletePool {
        id: String,
    },
    /// `GET /dispatchers`: the enrolled executors.
    FetchDispatchers,
    /// One admin read or mutation for the Access slice.
    Access(Box<super::admin::AccessEffect>),
    /// A session's bindings and the caller's credential usage, for the account picker.
    FetchAccountSwitch {
        session_id: String,
    },
    SwitchSessionAccount {
        session_id: String,
        /// The identity id; `family` says which binding it rebinds.
        account: String,
        account_name: String,
        family: String,
    },
    /// `GET /account-pools/usage` and `GET /accounts/usage`, as one refresh.
    FetchUsage,
    /// The three spend reads — token windows, usage analytics, cache busts —
    /// as one refresh, so a partial reply cannot leave half a table.
    FetchSpend {
        /// Minutes to subtract from UTC for local time.
        tz_offset: i32,
    },
    /// `GET /sessions/{id}/langfuse`: the cost line of one conversation.
    FetchSessionLangfuse {
        session_id: String,
    },
    EnrollDispatcher {
        name: String,
        request: Box<cctui_client::EnrollDispatcher>,
    },
    UpdateDispatcher {
        id: String,
        request: Box<cctui_client::UpdateDispatcher>,
    },
    DeleteDispatcher {
        id: String,
    },
    /// `GET /labels`: the whole catalogue.
    FetchLabels,
    CreateLabel {
        name: String,
        color: String,
        /// Attach it to this session as soon as it exists — creating a label
        /// from a row means you wanted it on that row.
        session_id: String,
    },
    UpdateLabel {
        id: String,
        name: Option<String>,
        color: Option<String>,
    },
    DeleteLabel {
        id: String,
    },
    AttachLabel {
        session_id: String,
        label_id: String,
    },
    DetachLabel {
        session_id: String,
        label_id: String,
    },
    /// Persist the fold state to `tui-state.json`.
    /// `GET /sessions/stats`: the counts the summary line and Overview show.
    FetchSessionStats,
    /// `GET /sessions/{id}`: a session the list does not carry, e.g. archived.
    FetchSession {
        session_id: String,
        /// Transcript position to land on once it opens.
        seq: Option<i64>,
    },
    SaveUiState(crate::config::uistate::UiState),
    /// `PUT /settings` with the whole blob, patched: the route replaces.
    SaveSettings {
        data: serde_json::Value,
    },
    SearchSessions {
        q: String,
        include_archived: bool,
        offset: usize,
    },
    SearchValues {
        field: String,
        q: String,
    },
}
