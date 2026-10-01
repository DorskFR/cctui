use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_client::{Client, ConversationFetch, FileRead, Page, UploadFile, WsClient};
use cctui_proto::drafts::{composer_draft_key, session_history_key};
use cctui_proto::ws::AgentEvent;
use tokio::sync::mpsc;

use super::account_switch::AccountSwitchAction;
use super::accounts::AccountAction;
use super::action::{AccountsFor, Action, Effect, ModelsFor};
use super::attach::AttachAction;
use super::attention::AttentionAction;
use super::bookmarks::BookmarkAction;
use super::controls::ControlsAction;
use super::conversation::ConversationAction;
use super::conversation_store::{PageKind, PageRequest};
use super::deeplink::DeepLinkAction;
use super::diagnose::DiagnoseAction;
use super::dispatch::DispatchAction;
use super::dispatchers::DispatcherAction;
use super::drafts::DraftAction;
use super::fileview::{self, FileViewAction};
use super::forkform::ForkAction;
use super::identity::AuthAction;
use super::images::ImagesAction;
use super::instance::InstanceAction;
use super::labels::LabelAction;
use super::line::agent_event_to_line;
use super::machines::MachineAction;
use super::pins::PinAction;
use super::pools::PoolAction;
use super::profiles::ProfileAction;
use super::send::SendAction;
use super::slice::SliceAction;
use super::spawn::{SpawnAction, SpawnFetch};
use super::spawn_drafts::SpawnDraftAction;
use super::spend::SpendAction;
use super::state::{ConversationLine, PendingPermission};
use super::toast::Level;

const QUEUE: usize = 256;

/// How many independent effects may be in flight beside the ordered lane. The
/// bound is what keeps a burst of reads from opening a connection each.
const CONCURRENCY: usize = 8;

/// Payload version sent with a settings write; the server migrates forward.
const SETTINGS_VERSION: i32 = 1;

/// Rows per page while walking a transcript for an export.
const EXPORT_PAGE: i64 = 500;

/// How long a composer sits still before its draft is written, matching the
/// web UI: a keystroke must not be a request.
const DRAFT_DEBOUNCE: Duration = Duration::from_millis(700);

/// Handle onto the effects worker. [`Effects::dispatch`] never awaits, so the
/// key-handling path never blocks on HTTP or the websocket.
pub struct Effects {
    tx: mpsc::Sender<Effect>,
    /// The action channel, so a dropped effect can still say so.
    notify: mpsc::Sender<Action>,
}

impl Effects {
    /// Ordered effects are executed one at a time: two messages typed in quick
    /// succession must reach the server in the order they were sent. Effects
    /// that answer only to themselves ([`Effect::runs_concurrently`]) go to
    /// bounded side tasks, so one slow read cannot hold the lane.
    pub fn start(server: Arc<Client>, ws: Arc<WsClient>) -> (Self, mpsc::Receiver<Action>) {
        let (tx, mut rx) = mpsc::channel::<Effect>(QUEUE);
        let (action_tx, action_rx) = mpsc::channel::<Action>(QUEUE);
        let notify = action_tx.clone();

        tokio::spawn(async move {
            let drafts = Arc::new(DraftSaver::new(Arc::clone(&server), action_tx.clone()));
            let limit = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
            while let Some(effect) = rx.recv().await {
                if effect.runs_concurrently() {
                    let (server, ws, drafts) =
                        (Arc::clone(&server), Arc::clone(&ws), Arc::clone(&drafts));
                    let action_tx = action_tx.clone();
                    let limit = Arc::clone(&limit);
                    tokio::spawn(async move {
                        // Must stay inside the task: awaiting capacity on the lane
                        // puts every ordered send behind these reads.
                        let Ok(_permit) = limit.acquire_owned().await else { return };
                        for action in run_guarded(server, ws, drafts, effect).await {
                            if action_tx.send(action).await.is_err() {
                                return;
                            }
                        }
                    });
                    continue;
                }
                for action in run(&server, &ws, &drafts, effect).await {
                    if action_tx.send(action).await.is_err() {
                        return;
                    }
                }
            }
        });

        (Self { tx, notify }, action_rx)
    }

    /// A dropped effect is a key press that did nothing, so the user is told
    /// rather than left guessing.
    ///
    /// A dropped send needs more than a toast: nothing else will ever report on
    /// it, so it is failed here or it sits at "Sending" for good.
    pub fn dispatch(&self, effect: Effect) {
        let Err(rejected) = self.tx.try_send(effect) else { return };
        tracing::warn!("the effect queue is full; dropping an effect");
        if let Effect::SendMessage { send_id, .. } = rejected.into_inner() {
            let _ = self.notify.try_send(Action::Send(SendAction::DispatchFailed {
                send_id,
                reason: "the effect queue is full".to_owned(),
            }));
            return;
        }
        let _ = self.notify.try_send(Action::Toast(
            Level::Error,
            "the server is not keeping up — that action was dropped".to_owned(),
        ));
    }

    pub fn dispatch_all(&self, effects: Vec<Effect>) {
        for effect in effects {
            self.dispatch(effect);
        }
    }

    /// Wait, at most `budget`, for everything already queued on the ordered lane
    /// to finish. Quit calls this so the draft writes it just asked for actually
    /// leave; a server that has stopped answering costs the budget, not the exit.
    pub async fn drain(&self, budget: std::time::Duration) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        // The send is inside the budget too: on a full queue it waits for room,
        // which is exactly the case that used to hold quit for minutes.
        let _ = tokio::time::timeout(budget, async {
            if self.tx.send(Effect::Barrier(tx)).await.is_ok() {
                let _ = rx.await;
            }
        })
        .await;
    }
}

impl Effect {
    /// Whether this effect may run beside the ordered lane.
    ///
    /// The lane exists so that two sends keep their order, and so that a read
    /// issued after a mutation sees it. Only effects that are both read-only and
    /// not a refetch of something just mutated are listed here — the ones that
    /// page a whole transcript or relay through a machine, which are exactly the
    /// ones that can block for a long time.
    const fn runs_concurrently(&self) -> bool {
        matches!(
            self,
            Self::LoadConversationPage { .. }
                | Self::ExportConversation { .. }
                | Self::OpenLinkedFile { .. }
                | Self::ReadAttachment { .. }
                | Self::ReadSpawnFile { .. }
                | Self::ReadClipboardImage
                | Self::FetchSessionImage { .. }
                | Self::FetchDiagnose { .. }
                | Self::FetchGitInfo { .. }
                | Self::FetchMachineDirs { .. }
                | Self::FetchRecentDirs { .. }
                | Self::FetchSpawnMemory
                | Self::FetchChangelog { .. }
                | Self::FetchSelfUpdateRun
                | Self::FetchHarnessModels { .. }
                | Self::FetchSessionLangfuse { .. }
                | Self::SearchSessions { .. }
                | Self::SearchValues { .. }
        )
    }

    /// What to call this effect in a message to the user.
    const fn label(&self) -> &'static str {
        match self {
            Self::LoadConversationPage { .. } => "loading the transcript",
            Self::ExportConversation { .. } => "the export",
            Self::OpenLinkedFile { .. } => "opening the file",
            Self::ReadAttachment { .. } | Self::ReadSpawnFile { .. } => "reading the attachment",
            Self::ReadClipboardImage => "reading the clipboard image",
            Self::FetchSessionImage { .. } => "loading the image",
            Self::FetchDiagnose { .. } => "the diagnose",
            Self::FetchGitInfo { .. } => "reading the git info",
            Self::FetchMachineDirs { .. } | Self::FetchRecentDirs { .. } => "listing directories",
            Self::FetchSpawnMemory => "loading the spawn memory",
            Self::FetchChangelog { .. } => "loading the changelog",
            Self::FetchSelfUpdateRun => "checking the update",
            Self::FetchHarnessModels { .. } => "loading the models",
            Self::FetchSessionLangfuse { .. } => "loading the spend",
            Self::SearchSessions { .. } | Self::SearchValues { .. } => "the search",
            _ => "that request",
        }
    }
}

/// The row a `save_draft` spawn created. The draft route answers with the new
/// row's id in `command_id` and no `session_id`, so that is what identifies it.
fn created_draft_id(reply: &cctui_proto::api::SpawnResponse) -> Option<String> {
    (reply.status == "draft").then(|| reply.command_id.to_string())
}

/// Runs one effect, turning a panic into a toast.
///
/// A side task's panic must not reach the process hook: the hook tears the
/// terminal down for a panic that ends the UI, and this one does not — the TUI
/// is still drawing.
async fn run_guarded(
    server: Arc<Client>,
    ws: Arc<WsClient>,
    drafts: Arc<DraftSaver>,
    effect: Effect,
) -> Vec<Action> {
    let label = effect.label();
    // The inner task is what isolates the unwind: tokio reports it as a
    // JoinError instead of letting it reach the process hook.
    let work = tokio::spawn(async move { run(&server, &ws, &drafts, effect).await });
    match work.await {
        Ok(actions) => actions,
        Err(e) if e.is_panic() => {
            tracing::error!(effect = label, "an effect panicked");
            vec![Action::Toast(Level::Error, format!("{label} failed unexpectedly"))]
        }
        Err(_) => Vec::new(),
    }
}

/// Per-key debounce for draft writes: a pending save is replaced, not queued,
/// and the request is off the effect queue so typing never waits on it.
struct DraftSaver {
    server: Arc<Client>,
    /// The first spawn autosave is what mints the draft row; its id has to reach
    /// the reducer or every later save mints another row.
    actions: mpsc::Sender<Action>,
    pending: std::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    /// The spawn autosave keeps its own slot rather than a key in `pending`:
    /// cancelling it has to be able to tell "still sleeping" from "already on
    /// the wire", and the second case must be left alone.
    spawn_save: std::sync::Mutex<Option<SpawnSave>>,
}

/// One in-progress spawn autosave.
struct SpawnSave {
    /// Raised once the request is actually out. Aborting after that loses the
    /// draft id the reply carries.
    committed: Arc<std::sync::atomic::AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl DraftSaver {
    fn new(server: Arc<Client>, actions: mpsc::Sender<Action>) -> Self {
        Self {
            server,
            actions,
            pending: std::sync::Mutex::new(HashMap::new()),
            spawn_save: std::sync::Mutex::new(None),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, tokio::task::JoinHandle<()>>> {
        self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save(&self, key: String, text: String) {
        self.pending().retain(|_, handle| !handle.is_finished());
        self.cancel(&key);
        let server = Arc::clone(&self.server);
        let target = key.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(DRAFT_DEBOUNCE).await;
            if let Err(e) = server.put_draft(&target, &text).await {
                tracing::warn!(%e, "draft save failed");
            }
        });
        self.pending().insert(key, handle);
    }

    /// One pending autosave at a time, keyed on the dialog rather than a
    /// draft id: the first save is what mints the id.
    ///
    /// `immediate` skips the debounce, for the flush on quit.
    fn autosave(
        &self,
        session_id: Option<String>,
        request: Box<cctui_proto::api::SpawnRequest>,
        immediate: bool,
        generation: u64,
    ) {
        // A create already on the wire owns the row this dialog is about to get.
        // Replacing it would lose that id and mint a second row, so the save is
        // skipped; the next keystroke saves against the adopted id.
        if self.spawn_create_in_flight() {
            return;
        }
        let committed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let creating = session_id.is_none();
        let server = Arc::clone(&self.server);
        let actions = self.actions.clone();
        let flag = Arc::clone(&committed);
        let handle = tokio::spawn(async move {
            if !immediate {
                tokio::time::sleep(DRAFT_DEBOUNCE).await;
            }
            if creating {
                flag.store(true, std::sync::atomic::Ordering::Release);
            }
            let outcome = match session_id.as_deref() {
                Some(id) => server.update_draft(id, &request).await.map(|_| None),
                // An autosave stores names, never bytes: the files go up at launch.
                None => server
                    .spawn_session(&request, Vec::new())
                    .await
                    .map(|reply| created_draft_id(&reply)),
            };
            match outcome {
                Ok(Some(session_id)) => {
                    let _ = actions
                        .send(Action::SpawnDrafts(SpawnDraftAction::DraftCreated {
                            session_id,
                            generation,
                        }))
                        .await;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(%e, "autosaving the spawn draft failed"),
            }
        });
        let previous = self.spawn_save().replace(SpawnSave { committed, handle });
        if let Some(previous) = previous {
            previous.handle.abort();
        }
    }

    fn spawn_save(&self) -> std::sync::MutexGuard<'_, Option<SpawnSave>> {
        self.spawn_save.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether a draft-creating save has passed the point where cancelling it
    /// would throw the new row's id away.
    fn spawn_create_in_flight(&self) -> bool {
        let mut slot = self.spawn_save();
        let Some(save) = slot.as_ref() else { return false };
        let running =
            save.committed.load(std::sync::atomic::Ordering::Acquire) && !save.handle.is_finished();
        if !running && save.handle.is_finished() {
            *slot = None;
        }
        running
    }

    /// The dialog is gone: stop the save it still owed. A create already on the
    /// wire is left to land, so the row it mints is still reported and can be
    /// discarded by id rather than stranded.
    fn cancel_spawn_autosave(&self) {
        if self.spawn_create_in_flight() {
            return;
        }
        let save = self.spawn_save().take();
        if let Some(save) = save {
            save.handle.abort();
        }
    }

    fn cancel(&self, key: &str) {
        let removed = self.pending().remove(key);
        if let Some(handle) = removed {
            handle.abort();
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run(server: &Client, ws: &WsClient, drafts: &DraftSaver, effect: Effect) -> Vec<Action> {
    match effect {
        Effect::RefreshSessions => match server.list_sessions().await {
            Ok(resp) => vec![Action::SessionsLoaded(resp.sessions)],
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, "session refresh failed");
                vec![Action::Toast(Level::Warn, "session refresh failed".to_owned())]
            }
        },
        Effect::FetchIdentity => match server.me().await {
            Ok(me) => vec![Action::Auth(AuthAction::Identified(Box::new(me)))],
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, "identity fetch failed");
                Vec::new()
            }
        },
        Effect::FetchPendingPermissions => fetch_pending_permissions(server).await,
        Effect::LoadConversationPage { session_id, kind, claim, page, etag } => {
            load_conversation_page(server, &session_id, kind, claim, page, etag.as_deref()).await
        }
        Effect::MarkSeen { session_id } => {
            if let Err(e) = server.mark_seen(&session_id).await {
                tracing::warn!(%e, "marking the session seen failed");
            }
            Vec::new()
        }
        Effect::LoadPins { session_id } => match server.list_pins(&session_id).await {
            Ok(pins) => vec![Action::Pins(PinAction::Loaded {
                session_id,
                seqs: pins.iter().map(|p| p.seq).collect(),
            })],
            Err(e) => {
                tracing::warn!(%e, session_id, "pin list fetch failed");
                Vec::new()
            }
        },
        Effect::LoadBookmarks { q, before } => {
            match server.list_bookmarks(&q, before, super::bookmarks::PAGE).await {
                Ok(rows) => vec![Action::Bookmarks(BookmarkAction::Loaded {
                    rows,
                    append: before.is_some(),
                })],
                Err(e) => {
                    tracing::warn!(%e, "bookmark list fetch failed");
                    vec![Action::Bookmarks(BookmarkAction::Failed)]
                }
            }
        }
        Effect::LoadProfiles => match server.profiles().await {
            Ok(list) => vec![Action::Profiles(ProfileAction::Loaded(list))],
            Err(e) => {
                tracing::warn!(%e, "profile list fetch failed");
                vec![Action::Profiles(ProfileAction::Failed)]
            }
        },
        Effect::CreateProfile { name, spec } => {
            let body = cctui_proto::api::profiles::CreateProfileRequest { name, spec: *spec };
            match server.create_profile(&body).await {
                Ok(profile) => vec![Action::Profiles(ProfileAction::Stored(Box::new(profile)))],
                Err(e) => {
                    tracing::warn!(%e, "profile create failed");
                    vec![Action::Toast(Level::Error, "could not create the profile".to_owned())]
                }
            }
        }
        Effect::UpdateProfile { id, name, spec } => {
            let body = cctui_proto::api::profiles::UpdateProfileRequest { name, spec: Some(*spec) };
            match server.update_profile(&id, &body).await {
                Ok(profile) => vec![Action::Profiles(ProfileAction::Stored(Box::new(profile)))],
                Err(e) => {
                    tracing::warn!(%e, id, "profile update failed");
                    vec![Action::Toast(Level::Error, "could not save the profile".to_owned())]
                }
            }
        }
        Effect::DeleteProfile { id } => match server.delete_profile(&id).await {
            Ok(()) => uuid::Uuid::parse_str(&id).map_or_else(
                |_| Vec::new(),
                |id| vec![Action::Profiles(ProfileAction::Deleted(id))],
            ),
            Err(e) => {
                tracing::warn!(%e, id, "profile delete failed");
                vec![Action::Toast(Level::Error, "could not delete the profile".to_owned())]
            }
        },
        Effect::ReorderProfiles { ids } => match server.reorder_profiles(ids).await {
            Ok(list) => vec![Action::Profiles(ProfileAction::Reordered(list))],
            Err(e) => {
                tracing::warn!(%e, "profile reorder failed");
                vec![Action::Toast(Level::Error, "could not reorder the profiles".to_owned())]
            }
        },
        Effect::AutosaveDraft { session_id, request, immediate, generation } => {
            drafts.autosave(session_id, request, immediate, generation);
            Vec::new()
        }
        Effect::CancelSpawnAutosave => {
            drafts.cancel_spawn_autosave();
            Vec::new()
        }
        Effect::LaunchDraft { session_id, env } => {
            match server.launch_draft(&session_id, &env).await {
                Ok(_) => vec![Action::SpawnDrafts(SpawnDraftAction::Launched { session_id })],
                Err(e) => {
                    tracing::warn!(%e, session_id, "draft launch failed");
                    vec![Action::Toast(Level::Error, "could not launch the draft".to_owned())]
                }
            }
        }
        Effect::DiscardDraftSession { session_id } => {
            match server.discard_draft(&session_id).await {
                Ok(()) => vec![Action::SpawnDrafts(SpawnDraftAction::Discarded { session_id })],
                Err(e) => {
                    tracing::warn!(%e, session_id, "draft discard failed");
                    vec![Action::Toast(Level::Error, "could not discard the draft".to_owned())]
                }
            }
        }
        Effect::CreateBookmark { draft } => match server.create_bookmark(&draft).await {
            Ok(bookmark) => vec![Action::Bookmarks(BookmarkAction::Saved(Box::new(bookmark)))],
            Err(e) => {
                tracing::warn!(%e, "bookmark save failed");
                vec![Action::Toast(Level::Error, "could not save the bookmark".to_owned())]
            }
        },
        Effect::UpdateBookmark { id, title, note } => {
            match server.update_bookmark(&id, &title, note.as_deref()).await {
                Ok(bookmark) => {
                    vec![Action::Bookmarks(BookmarkAction::Updated(Box::new(bookmark)))]
                }
                Err(e) => {
                    tracing::warn!(%e, id, "bookmark update failed");
                    vec![Action::Toast(Level::Error, "could not save the bookmark".to_owned())]
                }
            }
        }
        Effect::DeleteBookmark { id } => match server.delete_bookmark(&id).await {
            Ok(()) => uuid::Uuid::parse_str(&id).map_or_else(
                |_| Vec::new(),
                |id| vec![Action::Bookmarks(BookmarkAction::Deleted { id })],
            ),
            Err(e) => {
                tracing::warn!(%e, id, "bookmark delete failed");
                vec![Action::Toast(Level::Error, "could not delete the bookmark".to_owned())]
            }
        },
        Effect::PinMessage { session_id, seq } => pin(server, session_id, seq, true).await,
        Effect::UnpinMessage { session_id, seq } => pin(server, session_id, seq, false).await,
        Effect::Copy { text, label } => copy(&text, label),
        Effect::ExportConversation { session_id, meta, filter, format, path } => {
            export(server, &session_id, &meta, &filter, format, &path).await
        }
        Effect::LoadDraftIndex => load_draft_index(server).await,
        Effect::LoadDrafts { session_id } => load_drafts(server, session_id).await,
        Effect::SaveDraft { key, text } => {
            drafts.save(key, text);
            Vec::new()
        }
        Effect::DiscardDraft { key } => {
            drafts.cancel(&key);
            if let Err(e) = server.delete_draft(&key).await {
                tracing::warn!(%e, "draft discard failed");
            }
            Vec::new()
        }
        Effect::Subscribe { session_id } => {
            subscribe(ws, session_id).await;
            Vec::new()
        }
        Effect::Unsubscribe { session_id } => {
            if let Err(e) = ws.unsubscribe(session_id).await {
                tracing::warn!(%e, "unsubscribe failed");
            }
            Vec::new()
        }
        Effect::WatchTerminal { session_id, watch } => {
            if let Err(e) = ws.watch_terminal(session_id, watch).await {
                tracing::warn!(%e, watch, "terminal watch failed");
                return vec![Action::Toast(Level::Error, "terminal watch failed".to_owned())];
            }
            Vec::new()
        }
        Effect::SendMessage { send_id, session_id, content, ask_picks, turn_id, client_msg_id } => {
            let client_msg_id = client_msg_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let turn_id = turn_id.unwrap_or_else(uuid::Uuid::new_v4);
            match ws
                .send_message_as(
                    session_id,
                    content,
                    client_msg_id.clone(),
                    ask_picks,
                    Some(turn_id),
                )
                .await
            {
                Ok(()) => {
                    vec![Action::Send(SendAction::Dispatched { send_id, client_msg_id, turn_id })]
                }
                Err(e) => vec![Action::Send(SendAction::DispatchFailed {
                    send_id,
                    reason: e.to_string(),
                })],
            }
        }
        Effect::ArchiveSessions { ids, archived } => {
            let verb = if archived { "archive" } else { "unarchive" };
            match server.archive_sessions(&ids, archived).await {
                Ok(()) => vec![Action::RefreshSessions],
                Err(e) => {
                    tracing::warn!(%e, verb, "batch archive failed");
                    vec![Action::Toast(Level::Error, format!("{verb} failed: {e}"))]
                }
            }
        }
        Effect::PinSessions { ids, pinned } => {
            let verb = if pinned { "pin" } else { "unpin" };
            match server.pin_sessions(&ids, pinned).await {
                Ok(()) => vec![Action::RefreshSessions],
                Err(e) => {
                    tracing::warn!(%e, verb, "pin toggle failed");
                    vec![Action::Toast(Level::Error, format!("{verb} failed: {e}"))]
                }
            }
        }
        Effect::RenameSession { session_id, name } => {
            match server.rename_session(&session_id, &name).await {
                Ok(()) => vec![
                    Action::Toast(Level::Info, format!("renamed to \"{name}\"")),
                    Action::RefreshSessions,
                ],
                Err(e) => {
                    tracing::warn!(%e, "rename failed");
                    vec![Action::Toast(Level::Error, format!("rename failed: {e}"))]
                }
            }
        }
        Effect::KillSession { session_id } => match server.kill_session(&session_id).await {
            Ok(()) => {
                vec![Action::Toast(Level::Info, "killed".to_owned()), Action::RefreshSessions]
            }
            Err(e) => {
                tracing::warn!(%e, "kill failed");
                vec![Action::Toast(Level::Error, format!("kill failed: {e}"))]
            }
        },
        Effect::Interrupt { session_id } => {
            let error = server.interrupt(&session_id).await.err();
            if let Some(e) = error.as_ref() {
                tracing::warn!(%e, "interrupt failed");
            }
            vec![Action::Controls(ControlsAction::InterruptFinished {
                session_id,
                error: error.map(|e| e.to_string()),
            })]
        }
        Effect::Resume { session_id } => {
            let error = server.resume(&session_id).await.err();
            if let Some(e) = error.as_ref() {
                tracing::warn!(%e, "resume failed");
            }
            vec![Action::Fork(ForkAction::Resumed(error.map(|e| e.to_string())))]
        }
        Effect::FetchSpawnDispatchers => match server.spawn_dispatchers().await {
            Ok(names) => vec![spawn_data(SpawnFetch::Dispatchers(names))],
            Err(e) => {
                tracing::warn!(%e, "spawn dispatcher list fetch failed");
                Vec::new()
            }
        },
        Effect::Dispatch { body } => match server.dispatch(&body).await {
            Ok(resp) => vec![Action::Dispatch(DispatchAction::Submitted {
                session_id: resp.session_id,
                // `deduplicated` is the idempotency key landing on the job that
                // is already running.
                existing: resp.status == "deduplicated",
            })],
            Err(e) => {
                tracing::warn!(%e, "dispatch failed");
                vec![Action::Toast(Level::Error, format!("dispatch failed: {e}"))]
            }
        },
        Effect::Fork { session_id, request } => match server.fork(&session_id, &request).await {
            Ok(resp) => vec![Action::Controls(ControlsAction::Forked(resp.session_id))],
            Err(e) => {
                tracing::warn!(%e, "fork failed");
                vec![Action::Toast(Level::Error, format!("fork failed: {e}"))]
            }
        },
        Effect::FetchHarnessModels { want, harness, machine_id, model } => {
            match server.harness_models(&harness, Some(&machine_id), &model).await {
                Ok(models) => match want {
                    ModelsFor::RunningSession => {
                        vec![Action::Controls(ControlsAction::ModelsLoaded(Box::new(models)))]
                    }
                    ModelsFor::SpawnDialog => {
                        vec![Action::Spawn(super::spawn::SpawnAction::ModelsLoaded(Box::new(
                            models,
                        )))]
                    }
                    ModelsFor::ForkDialog => {
                        vec![Action::Fork(ForkAction::ModelsLoaded(Box::new(models)))]
                    }
                },
                Err(e) => {
                    tracing::warn!(%e, "harness model list fetch failed");
                    vec![Action::Toast(Level::Warn, "could not read the model list".to_owned())]
                }
            }
        }
        Effect::SetModel { session_id, model, effort } => {
            match server.set_model(&session_id, Some(&model), Some(&effort)).await {
                Ok(()) => {
                    vec![Action::Controls(ControlsAction::ModelSet { session_id, model, effort })]
                }
                Err(e) => {
                    tracing::warn!(%e, "set-model failed");
                    vec![Action::Toast(Level::Error, format!("set-model failed: {e}"))]
                }
            }
        }
        Effect::SetAutoApprove { session_id, enabled } => {
            match server.set_auto_approve(&session_id, enabled).await {
                Ok(()) => vec![Action::AutoApproveSet { session_id, enabled }],
                Err(e) => {
                    tracing::warn!(%e, "auto-approve toggle failed");
                    vec![Action::Toast(Level::Error, "auto-approve toggle failed".to_owned())]
                }
            }
        }
        Effect::ReadAttachment { session_id, path } => read_attachment(&session_id, &path).await,
        Effect::ReadSpawnFile { path } => read_spawn_file(&path).await,
        Effect::SaveDraftNow { key, text, recovery } => {
            match server.put_draft(&key, &text).await {
                // The local copy was written before this was sent, so success is
                // what removes it rather than failure being what writes it.
                Ok(()) => {
                    if let Some(target) = recovery {
                        crate::config::recovery::confirm(&target.path, &target.owner, &key);
                    }
                }
                Err(e) => tracing::warn!(%e, "flushing a draft on quit failed; it is kept locally"),
            }
            Vec::new()
        }
        Effect::Barrier(done) => {
            let _ = done.send(());
            Vec::new()
        }
        Effect::UploadAttachments { session_id, content, files } => {
            upload_attachments(server, session_id, content, files).await
        }
        Effect::OpenLinkedFile { session_id, machine_id, path } => {
            open_linked_file(server, &session_id, &machine_id, &path).await
        }
        Effect::ReadClipboardImage => vec![read_clipboard_image()],
        Effect::FetchSessionImage { session_id, image_id } => {
            match server.session_image(&session_id, &image_id).await {
                Ok(cctui_client::FileRead::Ok { content_type, bytes }) => {
                    vec![Action::Images(ImagesAction::Fetched {
                        name: format!("{image_id}.{}", image_extension(&content_type)),
                        content_type,
                        bytes,
                    })]
                }
                Ok(cctui_client::FileRead::Refused(refusal)) => {
                    vec![Action::FileView(FileViewAction::Refused {
                        name: "the image".to_owned(),
                        refusal: Box::new(refusal),
                        source: fileview::FileSource::Blob,
                    })]
                }
                Err(e) => {
                    tracing::warn!(%e, "image fetch failed");
                    vec![Action::Toast(Level::Error, "could not fetch the image".to_owned())]
                }
            }
        }
        Effect::OpenInOsViewer { name, bytes } => {
            open_in_os_viewer(&name, &bytes);
            Vec::new()
        }
        Effect::FetchSessionStats => {
            match server.session_stats(&super::slice::local_timezone()).await {
                Ok(stats) => vec![Action::Slice(SliceAction::StatsLoaded(Box::new(stats)))],
                Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
                Err(e) => {
                    tracing::warn!(%e, "session stats fetch failed");
                    vec![Action::Slice(SliceAction::StatsFailed)]
                }
            }
        }
        Effect::FetchSession { session_id, seq } => match server.get_session(&session_id).await {
            Ok(session) => {
                vec![Action::DeepLink(DeepLinkAction::Fetched { session: Box::new(session), seq })]
            }
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, session_id, "session fetch failed");
                vec![Action::DeepLink(DeepLinkAction::Failed { session_id, error: e.to_string() })]
            }
        },
        // The accounts, usage and spend arms live in their own runner: one
        // function holding every arm builds a future too large for the stack.
        rest @ (Effect::FetchAccounts { .. }
        | Effect::FetchRedirects
        | Effect::FetchAccountPools { .. }
        | Effect::UpdateAccount { .. }
        | Effect::ClaimLimitReset { .. }
        | Effect::PutRedirect { .. }
        | Effect::DeleteRedirect { .. }
        | Effect::CreatePool { .. }
        | Effect::UpdatePool { .. }
        | Effect::DeletePool { .. }
        | Effect::FetchMachines
        | Effect::FetchUsage
        | Effect::FetchSpend { .. }
        | Effect::FetchSessionLangfuse { .. }
        | Effect::FetchAccountSwitch { .. }
        | Effect::SwitchSessionAccount { .. }
        | Effect::FetchVersion
        | Effect::RefreshVersion
        | Effect::FetchSelfUpdateRun
        | Effect::FetchChangelog
        | Effect::LaunchSelfUpdate
        | Effect::Access(..)
        | Effect::FetchDispatchers
        | Effect::EnrollDispatcher { .. }
        | Effect::UpdateDispatcher { .. }
        | Effect::DeleteDispatcher { .. }
        | Effect::FetchAccountsUsage
        | Effect::FetchLabels
        | Effect::CreateLabel { .. }
        | Effect::UpdateLabel { .. }
        | Effect::DeleteLabel { .. }
        | Effect::AttachLabel { .. }
        | Effect::DetachLabel { .. }
        | Effect::SaveUiState(..)
        | Effect::SearchSessions { .. }
        | Effect::SearchValues { .. }
        | Effect::FetchGitInfo { .. }
        | Effect::FetchMachineDirs { .. }
        | Effect::FetchRecentDirs
        | Effect::FetchSpawnMemory
        | Effect::PutSpawnMemory { .. }
        | Effect::RefreshCodexModels { .. }
        | Effect::SpawnSession { .. }
        | Effect::SaveSettings { .. }
        | Effect::RespondPermission { .. }
        | Effect::FetchDiagnose { .. }) => run_accounts(server, ws, rest).await,
    }
}

#[allow(clippy::too_many_lines)]
async fn run_accounts(server: &Client, ws: &WsClient, effect: Effect) -> Vec<Action> {
    match effect {
        Effect::FetchAccounts { want } => match server.accounts().await {
            Ok(rows) => match want {
                AccountsFor::Slice => vec![Action::Accounts(AccountAction::Loaded(rows))],
                AccountsFor::SpawnDialog => vec![spawn_data(SpawnFetch::Accounts(rows))],
            },
            Err(e) => {
                tracing::warn!(%e, "listing accounts failed");
                match want {
                    AccountsFor::Slice => {
                        vec![Action::Accounts(AccountAction::Failed(account_error(&e)))]
                    }
                    AccountsFor::SpawnDialog => {
                        vec![Action::Toast(Level::Warn, "could not list accounts".to_owned())]
                    }
                }
            }
        },
        Effect::FetchRedirects => match server.redirects().await {
            Ok(rules) => vec![Action::Accounts(AccountAction::RedirectsLoaded(rules))],
            Err(e) => {
                tracing::warn!(%e, "listing redirect rules failed");
                Vec::new()
            }
        },
        Effect::FetchAccountPools { want } => match server.account_pools().await {
            Ok(rows) => match want {
                AccountsFor::Slice => vec![Action::Pools(PoolAction::Loaded(rows))],
                AccountsFor::SpawnDialog => vec![spawn_data(SpawnFetch::Pools(rows))],
            },
            Err(e) => {
                tracing::warn!(%e, "listing account pools failed");
                match want {
                    AccountsFor::Slice => {
                        vec![Action::Pools(PoolAction::Failed(account_error(&e)))]
                    }
                    AccountsFor::SpawnDialog => Vec::new(),
                }
            }
        },
        Effect::UpdateAccount { id, request } => match server.update_account(&id, &request).await {
            Ok(()) => refetch_accounts(server).await,
            Err(e) => vec![account_refusal(&e, "could not edit the account")],
        },
        Effect::ClaimLimitReset { provider_id, credit_id } => {
            let request = cctui_client::LimitResetRequest { credit_id };
            match server.limit_reset(&provider_id, &request).await {
                Ok(outcome) => vec![Action::Accounts(AccountAction::ResetDone(Box::new(outcome)))],
                Err(e) => vec![account_refusal(&e, "could not claim the reset")],
            }
        }
        Effect::PutRedirect { account_id, to_account, family } => {
            let request = cctui_client::PutRedirect {
                to_account: Some(to_account),
                family,
                ..Default::default()
            };
            match server.put_account_redirect(&account_id, &request).await {
                Ok(()) => refetch_accounts(server).await,
                Err(e) => vec![account_refusal(&e, "could not set the redirect")],
            }
        }
        Effect::DeleteRedirect { id } => match server.delete_redirect(&id).await {
            Ok(()) => refetch_accounts(server).await,
            Err(e) => vec![account_refusal(&e, "could not clear the redirect")],
        },
        Effect::CreatePool { request } => match server.create_account_pool(&request).await {
            Ok(_) => refetch_pools(server).await,
            Err(e) => vec![pool_refusal(&e, "could not create the pool")],
        },
        Effect::UpdatePool { id, request } => {
            match server.update_account_pool(&id, &request).await {
                Ok(()) => refetch_pools(server).await,
                Err(e) => vec![pool_refusal(&e, "could not edit the pool")],
            }
        }
        Effect::DeletePool { id } => match server.delete_account_pool(&id).await {
            Ok(()) => refetch_pools(server).await,
            Err(e) => vec![pool_refusal(&e, "could not delete the pool")],
        },
        Effect::FetchMachines => match server.machines().await {
            Ok(rows) => vec![Action::Machines(MachineAction::Loaded(rows))],
            Err(e) => {
                tracing::warn!(%e, "listing machines failed");
                vec![Action::Machines(MachineAction::Failed(machine_error(&e)))]
            }
        },
        Effect::FetchUsage => fetch_usage(server).await,
        Effect::FetchSpend { tz_offset } => fetch_spend(server, tz_offset).await,
        Effect::FetchSessionLangfuse { session_id } => {
            let usage = server.session_langfuse(&session_id).await.ok().map(|u| {
                cctui_clientcore::spend::LangfuseSpend {
                    cost_usd: u.cost_usd,
                    trace_count: u.trace_count,
                }
            });
            vec![Action::Spend(SpendAction::Langfuse { session_id, usage })]
        }
        Effect::FetchAccountSwitch { session_id } => {
            match super::account_switch::load(server, &session_id).await {
                Ok((bindings, credentials)) => {
                    vec![Action::AccountSwitch(AccountSwitchAction::Loaded {
                        bindings,
                        credentials,
                    })]
                }
                Err(e) => {
                    tracing::warn!(%e, "loading the account picker failed");
                    vec![Action::AccountSwitch(AccountSwitchAction::Failed(e.to_string()))]
                }
            }
        }
        Effect::SwitchSessionAccount { session_id, account, account_name, family } => {
            match server.switch_session_account(&session_id, &account, &family).await {
                Ok(()) => {
                    vec![Action::AccountSwitch(AccountSwitchAction::Switched {
                        account_name,
                        family,
                    })]
                }
                Err(e) => {
                    tracing::warn!(%e, "switching the session account failed");
                    vec![Action::AccountSwitch(AccountSwitchAction::SwitchFailed(e.to_string()))]
                }
            }
        }
        Effect::FetchVersion => match server.version().await {
            Ok(info) => vec![Action::Instance(InstanceAction::Loaded(Box::new(info)))],
            Err(cctui_client::ClientError::Forbidden { .. }) => {
                vec![Action::Instance(InstanceAction::Forbidden)]
            }
            Err(e) => {
                tracing::warn!(%e, "reading the server version failed");
                vec![Action::Instance(InstanceAction::Failed(e.to_string()))]
            }
        },
        Effect::RefreshVersion => match server.refresh_version().await {
            Ok(()) => vec![Action::Instance(InstanceAction::Refresh)],
            Err(e) => {
                tracing::warn!(%e, "probing upstream failed");
                vec![Action::Instance(InstanceAction::Failed(e.to_string()))]
            }
        },
        Effect::FetchSelfUpdateRun => match server.self_update_status().await {
            Ok(run) => vec![Action::Instance(InstanceAction::RunLoaded(run.map(Box::new)))],
            Err(cctui_client::ClientError::Forbidden { .. }) => {
                vec![Action::Instance(InstanceAction::Forbidden)]
            }
            Err(e) => {
                tracing::warn!(%e, "reading the self-update run failed");
                vec![Action::Instance(InstanceAction::Failed(e.to_string()))]
            }
        },
        Effect::FetchChangelog => match server.version_changelog().await {
            Ok(log) => vec![Action::Instance(InstanceAction::ChangelogLoaded(log.releases))],
            Err(e) => {
                tracing::warn!(%e, "reading the changelog failed");
                Vec::new()
            }
        },
        Effect::LaunchSelfUpdate => match server.self_update().await {
            Ok(launch) => vec![Action::Instance(InstanceAction::Launched(Box::new(launch)))],
            Err(e) => {
                tracing::warn!(%e, "launching the self-update failed");
                vec![Action::Instance(InstanceAction::LaunchFailed(e.to_string()))]
            }
        },
        Effect::Access(effect) => super::admin::run(*effect, server).await,
        Effect::FetchDispatchers => match server.dispatchers().await {
            Ok(rows) => vec![Action::Dispatchers(DispatcherAction::Loaded(rows))],
            Err(e) => {
                tracing::warn!(%e, "listing dispatchers failed");
                vec![Action::Dispatchers(DispatcherAction::Failed(dispatcher_error(&e)))]
            }
        },
        // The reply carries the key: it goes straight into the action and is
        // never logged, because the log is not somewhere a secret may land.
        Effect::EnrollDispatcher { name, request } => {
            match server.enroll_dispatcher(&request).await {
                Ok(reply) => vec![Action::Dispatchers(DispatcherAction::Enrolled {
                    name,
                    reply: Box::new(reply),
                })],
                Err(e) => {
                    tracing::warn!(%e, "enrolling a dispatcher failed");
                    vec![Action::Toast(Level::Error, format!("could not enroll {name}"))]
                }
            }
        }
        Effect::UpdateDispatcher { id, request } => {
            match server.update_dispatcher(&id, &request).await {
                Ok(_) => refetch_dispatchers(server).await,
                Err(e) => {
                    tracing::warn!(%e, "editing a dispatcher failed");
                    vec![Action::Toast(Level::Error, "could not edit the dispatcher".to_owned())]
                }
            }
        }
        Effect::DeleteDispatcher { id } => match server.delete_dispatcher(&id).await {
            Ok(()) => refetch_dispatchers(server).await,
            Err(e) => {
                tracing::warn!(%e, "removing a dispatcher failed");
                vec![Action::Toast(Level::Error, "could not remove the dispatcher".to_owned())]
            }
        },
        Effect::FetchAccountsUsage => match server.accounts_usage().await {
            Ok(usage) => vec![spawn_data(SpawnFetch::Usage(usage))],
            Err(e) => {
                tracing::warn!(%e, "reading account usage failed");
                Vec::new()
            }
        },
        Effect::FetchLabels => match server.labels().await {
            Ok(labels) => vec![Action::Labels(LabelAction::Loaded(labels))],
            Err(e) => {
                tracing::warn!(%e, "fetching labels failed");
                Vec::new()
            }
        },
        // Creating a label from a row means you wanted it on that row, so the
        // attach happens here rather than asking the user for a second gesture.
        Effect::CreateLabel { name, color, session_id } => {
            let label = match server.create_label(&name, &color).await {
                Ok(label) => label,
                Err(e) => {
                    tracing::warn!(%e, "creating a label failed");
                    return vec![Action::Toast(Level::Error, format!("could not create {name}"))];
                }
            };
            let mut actions = refetch_labels(server).await;
            match server.attach_label(&session_id, &label.id).await {
                Ok(()) => actions
                    .push(Action::Labels(LabelAction::Attached { session_id, label_id: label.id })),
                Err(e) => {
                    tracing::warn!(%e, "attaching a fresh label failed");
                    actions.push(Action::Toast(
                        Level::Warn,
                        format!("{name} was created but not attached"),
                    ));
                }
            }
            actions
        }
        Effect::UpdateLabel { id, name, color } => {
            match server.update_label(&id, name, color).await {
                Ok(_) => refetch_labels(server).await,
                Err(e) => {
                    tracing::warn!(%e, "editing a label failed");
                    vec![Action::Toast(Level::Error, "could not edit the label".to_owned())]
                }
            }
        }
        Effect::DeleteLabel { id } => match server.delete_label(&id).await {
            Ok(()) => refetch_labels(server).await,
            Err(e) => {
                tracing::warn!(%e, "deleting a label failed");
                vec![Action::Toast(Level::Error, "could not delete the label".to_owned())]
            }
        },
        Effect::AttachLabel { session_id, label_id } => {
            match server.attach_label(&session_id, &label_id).await {
                Ok(()) => vec![Action::Labels(LabelAction::Attached { session_id, label_id })],
                Err(e) => {
                    tracing::warn!(%e, "attaching a label failed");
                    vec![Action::Toast(Level::Error, "could not attach the label".to_owned())]
                }
            }
        }
        Effect::DetachLabel { session_id, label_id } => {
            match server.detach_label(&session_id, &label_id).await {
                Ok(()) => vec![Action::Labels(LabelAction::Detached { session_id, label_id })],
                Err(e) => {
                    tracing::warn!(%e, "detaching a label failed");
                    vec![Action::Toast(Level::Error, "could not detach the label".to_owned())]
                }
            }
        }
        Effect::SaveUiState(state) => {
            crate::config::uistate::save(&state);
            Vec::new()
        }
        Effect::SearchSessions { q, include_archived, offset } => {
            let limit = super::list_search::LIMIT;
            let at = i64::try_from(offset).unwrap_or(i64::MAX);
            match server.search_sessions(&q, include_archived, limit, at).await {
                Ok(resp) => {
                    // A full page means there is probably another: the route
                    // reports no total, so the page size is the only signal.
                    let has_more = i64::try_from(resp.sessions.len()).unwrap_or(0) >= limit;
                    vec![Action::ListSearch(super::list_search::ListSearchAction::Loaded {
                        query: q,
                        offset,
                        sessions: resp.sessions,
                        has_more,
                    })]
                }
                Err(e) => {
                    tracing::warn!(%e, "the session search failed");
                    vec![Action::ListSearch(super::list_search::ListSearchAction::Failed(
                        "search failed".to_owned(),
                    ))]
                }
            }
        }
        Effect::SearchValues { field, q } => match server.search_values(&field, &q).await {
            Ok(values) => {
                vec![Action::ListSearch(super::list_search::ListSearchAction::ValuesLoaded {
                    values,
                })]
            }
            Err(e) => {
                tracing::warn!(%e, field, "the value autocomplete failed");
                Vec::new()
            }
        },
        Effect::FetchGitInfo { machine_id, path } => {
            // A failure is the answer, not an error: an unreadable path is how
            // the badge learns to say "not a directory".
            let info = server.machine_git_info(&machine_id, &path).await.ok().map(Box::new);
            vec![Action::Spawn(super::spawn::SpawnAction::GitInfo { machine_id, path, info })]
        }
        Effect::FetchMachineDirs { machine_id, path } => {
            match server.machine_dirs(&machine_id, &path).await {
                Ok(dirs) => vec![Action::Spawn(super::spawn::SpawnAction::DirsLoaded(dirs))],
                Err(e) => {
                    tracing::warn!(%e, machine_id, "cannot list directories");
                    Vec::new()
                }
            }
        }
        Effect::FetchRecentDirs => match server.recent_dirs().await {
            Ok(dirs) => vec![Action::Spawn(super::spawn::SpawnAction::RecentDirsLoaded(dirs))],
            Err(e) => {
                tracing::warn!(%e, "cannot read the recent directories");
                Vec::new()
            }
        },
        Effect::FetchSpawnMemory => match server.spawn_memory().await {
            Ok(payload) => {
                vec![Action::Spawn(super::spawn::SpawnAction::MemoryLoaded(Box::new(payload)))]
            }
            Err(e) => {
                tracing::debug!(%e, "no spawn memory");
                Vec::new()
            }
        },
        Effect::PutSpawnMemory { entries } => {
            let payload = cctui_proto::drafts::SpawnMemoryPayload { entries };
            if let Err(e) = server.put_spawn_memory(&payload).await {
                tracing::warn!(%e, "cannot remember this spawn");
            }
            Vec::new()
        }
        Effect::RefreshCodexModels { machine_id } => {
            if let Err(e) = server.refresh_codex_models(&machine_id).await {
                tracing::warn!(%e, machine_id, "the codex catalog refresh failed");
            }
            vec![Action::Spawn(super::spawn::SpawnAction::ModelsRefreshed)]
        }
        Effect::SpawnSession { request, files } => {
            let files = files
                .into_iter()
                .map(|(name, bytes)| cctui_client::UploadFile { name, bytes })
                .collect();
            match server.spawn_session(&request, files).await {
                Ok(resp) => vec![Action::Spawn(super::spawn::SpawnAction::Accepted {
                    command_id: resp.command_id,
                    session_id: resp.session_id.map(|id| id.to_string()),
                })],
                Err(e) => {
                    tracing::warn!(%e, "the spawn request failed");
                    vec![Action::Spawn(super::spawn::SpawnAction::Failed(e.to_string()))]
                }
            }
        }
        Effect::SaveSettings { patch } => save_settings(server, patch).await,
        Effect::RespondPermission { session_id, request_id, behavior } => {
            if let Err(e) = ws.respond_permission(session_id, request_id, behavior.to_owned()).await
            {
                tracing::warn!(%e, "permission response failed");
            }
            Vec::new()
        }
        Effect::FetchDiagnose { session_id } => match server.diagnose(&session_id).await {
            Ok(report) => {
                vec![Action::Diagnose(DiagnoseAction::Loaded {
                    session_id,
                    report: Box::new(report),
                })]
            }
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, session_id, "diagnose fetch failed");
                vec![Action::Diagnose(DiagnoseAction::Failed { session_id, error: e.to_string() })]
            }
        },
        // Everything else is the first runner's.
        _ => Vec::new(),
    }
}

/// A pin is applied once the server owns it, so a failed write leaves no
/// marker the next list fetch would contradict.
async fn pin(server: &Client, session_id: String, seq: i64, pinned: bool) -> Vec<Action> {
    let outcome = if pinned {
        server.pin_message(&session_id, seq).await.map(|_| ())
    } else {
        server.unpin_message(&session_id, seq).await
    };
    match outcome {
        Ok(()) => vec![Action::Pins(PinAction::Changed { session_id, seq, pinned })],
        Err(e) => {
            tracing::warn!(%e, session_id, seq, "pin write failed");
            let what = if pinned { "pin" } else { "unpin" };
            vec![Action::Toast(Level::Error, format!("could not {what} that message"))]
        }
    }
}

async fn load_draft_index(server: &Client) -> Vec<Action> {
    match server.list_drafts().await {
        Ok(list) => vec![Action::Drafts(DraftAction::IndexLoaded(Box::new(list)))],
        Err(e) => {
            tracing::warn!(%e, "draft index fetch failed");
            Vec::new()
        }
    }
}

/// A draft the web UI has since edited must win over the startup index, so the
/// session's two keys are read again as its conversation opens.
async fn load_drafts(server: &Client, session_id: String) -> Vec<Action> {
    let text = server.get_draft(&composer_draft_key(&session_id)).await;
    let history = server.get_draft(&session_history_key(&session_id)).await;
    if let Err(e) = &text {
        tracing::warn!(%e, session_id, "draft fetch failed");
    }
    vec![Action::Drafts(DraftAction::Loaded {
        session_id,
        text: text.unwrap_or_default(),
        history: history.unwrap_or_default(),
    })]
}

async fn fetch_pending_permissions(server: &Client) -> Vec<Action> {
    match server.pending_permissions().await {
        Ok(items) => {
            let items = items
                .into_iter()
                .map(|p| PendingPermission {
                    session_id: p.session_id,
                    request_id: p.request_id,
                    tool_name: p.tool_name,
                    description: p.description,
                    input_preview: p.input_preview,
                })
                .collect();
            vec![Action::Attention(AttentionAction::PendingPermissionsLoaded(items))]
        }
        Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
        Err(e) => {
            tracing::warn!(%e, "pending permission fetch failed");
            Vec::new()
        }
    }
}

fn spawn_data(fetch: SpawnFetch) -> Action {
    Action::Spawn(SpawnAction::DataLoaded(Box::new(fetch)))
}

async fn subscribe(ws: &WsClient, session_id: String) {
    if let Err(e) = ws.subscribe(session_id).await {
        tracing::warn!(%e, "subscribe failed");
    }
}

/// Every copy in the TUI lands here, so both routes and the toast are decided in
/// one place.
fn copy(text: &str, label: &'static str) -> Vec<Action> {
    if crate::clipboard::copy_with_fallback(text) {
        vec![Action::Toast(Level::Info, format!("copied the {label}"))]
    } else {
        vec![Action::Toast(Level::Warn, format!("cannot copy the {label}"))]
    }
}

/// Walks the whole transcript, oldest page first: the store holds rendered
/// lines, and an export needs the events behind them.
async fn export(
    server: &Client,
    session_id: &str,
    meta: &super::export::Meta,
    filter: &super::transcript_filter::Filter,
    format: super::export::Format,
    path: &std::path::Path,
) -> Vec<Action> {
    let mut events: Vec<AgentEvent> = Vec::new();
    let mut before = None;
    loop {
        let page = Page { before, after: None, limit: Some(EXPORT_PAGE) };
        let fetch = match server.conversation(session_id, page, None).await {
            Ok(fetch) => fetch,
            Err(e) => {
                tracing::warn!(%e, session_id, "the export could not read the transcript");
                return vec![Action::Toast(
                    Level::Error,
                    "export failed: cannot read the transcript".to_owned(),
                )];
            }
        };
        let ConversationFetch::Page { rows, .. } = fetch else { break };
        if rows.is_empty() {
            break;
        }
        let oldest = rows.iter().map(|r| r.seq).min();
        let mut page_events: Vec<AgentEvent> = rows
            .into_iter()
            .filter_map(|row| serde_json::from_value::<AgentEvent>(row.event).ok())
            .collect();
        page_events.append(&mut events);
        events = page_events;
        match oldest {
            Some(seq) => before = Some(seq),
            None => break,
        }
    }

    let body = match format {
        super::export::Format::Markdown => super::export::to_markdown(meta, &events, filter),
        super::export::Format::Html => super::export::to_html(meta, &events, filter),
    };
    if let Some(dir) = path.parent()
        && let Err(e) = tokio::fs::create_dir_all(dir).await
    {
        tracing::warn!(%e, "cannot create the export directory");
        return vec![Action::Toast(
            Level::Error,
            "export failed: cannot create the directory".to_owned(),
        )];
    }
    match tokio::fs::write(path, body).await {
        Ok(()) => vec![Action::Toast(
            Level::Info,
            format!("exported {} events to {}", events.len(), path.display()),
        )],
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "cannot write the export");
            vec![Action::Toast(Level::Error, format!("export failed: {e}"))]
        }
    }
}

async fn load_conversation_page(
    server: &Client,
    session_id: &str,
    kind: PageKind,
    claim: Option<u64>,
    page: PageRequest,
    etag: Option<&str>,
) -> Vec<Action> {
    let request = Page { before: page.before, after: page.after, limit: page.limit };
    let fetch = match server.conversation(session_id, request, etag).await {
        Ok(fetch) => fetch,
        Err(e) => {
            tracing::warn!(%e, session_id, "conversation fetch failed");
            return vec![Action::Conversation(ConversationAction::Failed {
                session_id: session_id.to_owned(),
                kind,
                claim,
            })];
        }
    };

    let ConversationFetch::Page { rows, etag, has_more } = fetch else {
        return vec![Action::Conversation(ConversationAction::NotModified {
            session_id: session_id.to_owned(),
            kind,
            claim,
        })];
    };

    let total = rows.len();
    let mut undecodable = 0;
    let mut decoded: Vec<(i64, ConversationLine)> = Vec::with_capacity(total);
    for row in rows {
        let seq = row.seq;
        match serde_json::from_value::<AgentEvent>(row.event) {
            // An event with nothing to render is not a decoding failure.
            Ok(event) => decoded.extend(agent_event_to_line(&event).map(|line| (seq, line))),
            Err(_) => undecodable += 1,
        }
    }

    let mut actions = vec![Action::Conversation(ConversationAction::Loaded {
        session_id: session_id.to_owned(),
        kind,
        claim,
        rows: decoded,
        etag,
        has_more,
    })];
    if undecodable > 0 {
        actions.push(Action::UndecodableAgentEvents(undecodable));
    }
    actions
}

/// Read one path off the local disk into a composer attachment. The name is the
/// basename, and an image's pixel size is measured here so the chip can show it.
async fn read_attachment(session_id: &str, path: &str) -> Vec<Action> {
    let failed = |message: String| vec![Action::Attach(AttachAction::ReadFailed(message))];
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    if name.is_empty() {
        return failed(format!("{path} is not a file"));
    }
    let bytes = match read_capped(path, &name).await {
        Ok(bytes) => bytes,
        Err(message) => return failed(message),
    };
    let content_type = guess_content_type(&name);
    let dimensions = image_dimensions(&bytes);
    vec![Action::Attach(AttachAction::Read {
        session_id: session_id.to_owned(),
        name,
        bytes,
        content_type,
        dimensions,
    })]
}

/// The spawn dialog's files section, gated exactly like the composer's.
async fn read_spawn_file(path: &str) -> Vec<Action> {
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    let outcome = if name.is_empty() {
        Err(format!("{path} is not a file"))
    } else {
        read_capped(path, &name).await
    };
    vec![Action::Spawn(crate::app::spawn::SpawnAction::FileRead {
        path: path.to_owned(),
        outcome: outcome.map(|bytes| (bytes, guess_content_type(&name))),
    })]
}

/// Stat, gate, then read — never the other way round, so an unbounded or
/// unreadable path costs a `stat` instead of the process.
async fn read_capped(path: &str, name: &str) -> Result<Vec<u8>, String> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| format!("cannot read {path}: {e}"))?;
    if let Some(refusal) = crate::app::attach::path_refusal(name, &meta, crate::app::attach::caps())
    {
        return Err(refusal);
    }
    tokio::fs::read(path).await.map_err(|e| format!("cannot read {path}: {e}"))
}

/// Extension-based content type; only the families the composer treats
/// specially need naming, everything else is opaque bytes.
pub fn guess_content_type(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "txt" | "log" | "rs" | "toml" | "py" | "ts" | "js" | "sh" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_owned()
}

/// Pixel size of an image, or `None` when the bytes are not an image the
/// decoders compiled in can read.
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

async fn upload_attachments(
    server: &Client,
    session_id: String,
    content: String,
    files: Vec<(String, Vec<u8>)>,
) -> Vec<Action> {
    let names: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
    let payload =
        files.into_iter().map(|(name, bytes)| UploadFile { name, bytes }).collect::<Vec<_>>();
    match server.stage_session_files(&session_id, payload).await {
        Ok(resp) => vec![Action::Attach(AttachAction::Uploaded {
            session_id,
            content,
            names,
            paths: resp.paths,
        })],
        Err(e) => {
            tracing::warn!(%e, %session_id, "staging attachments failed");
            vec![Action::Attach(AttachAction::UploadFailed {
                session_id,
                content,
                message: format!("attaching files failed: {e}"),
            })]
        }
    }
}

/// Read an agent-linked path, re-asking the machine that linked it when this one
/// says the path is denied or absent — the webui's `attemptOpen` chain.
async fn open_linked_file(
    server: &Client,
    session_id: &str,
    machine_id: &str,
    path: &str,
) -> Vec<Action> {
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    let first = match server.read_machine_file(machine_id, path, session_id).await {
        Ok(read) => read,
        Err(e) => {
            tracing::warn!(%e, "reading a linked file failed");
            FileRead::Refused(cctui_client::FileRefusal::network())
        }
    };
    let refusal = match first {
        FileRead::Ok { content_type, bytes } => {
            return vec![opened(name, path, content_type, bytes)];
        }
        FileRead::Refused(refusal) => refusal,
    };
    if !fileview::may_live_elsewhere(refusal.status) {
        return vec![refused(name, refusal)];
    }
    let Ok(Some(owner)) = server.linked_file_owner(session_id, path).await else {
        return vec![refused(name, refusal)];
    };
    match server.read_machine_file(&owner.machine_id, path, &owner.session_id).await {
        Ok(FileRead::Ok { content_type, bytes }) => {
            vec![opened(name, path, content_type, bytes)]
        }
        // The owning machine had nothing better to say, so the first refusal stands.
        _ => vec![refused(name, refusal)],
    }
}

fn opened(name: String, path: &str, content_type: String, bytes: Vec<u8>) -> Action {
    Action::FileView(FileViewAction::Opened { name, path: path.to_owned(), content_type, bytes })
}

fn refused(name: String, refusal: cctui_client::FileRefusal) -> Action {
    Action::FileView(FileViewAction::Refused {
        name,
        refusal: Box::new(refusal),
        source: fileview::FileSource::Machine,
    })
}

/// Stages the bytes somewhere only this user can read and hands the path over.
///
/// The directory is created fresh with 0700 and the file written 0600, so the
/// path cannot be guessed or pre-planted as a symlink by another local user. It
/// is deliberately leaked: the handler opens it after this returns.
fn open_in_os_viewer(name: &str, bytes: &[u8]) {
    let dir = match tempfile::Builder::new().prefix("cctui-").tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(%e, "cannot make a private directory for the OS viewer");
            return;
        }
    };
    // tempdir takes its mode from the umask, which is usually world-readable.
    #[cfg(unix)]
    if let Err(e) =
        std::fs::set_permissions(dir.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700))
    {
        tracing::warn!(%e, "cannot make the staging directory private");
        return;
    }
    let path = dir.path().join(super::fileview::staged_file_name(name));
    let written = {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&path).and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
    };
    if let Err(e) = written {
        tracing::warn!(%e, "cannot stage a file for the OS viewer");
        return;
    }
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(e) = std::process::Command::new(opener)
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        tracing::warn!(%e, opener, "cannot launch the OS viewer");
    }
    // The handler reads the file after this returns, so the directory outlives us.
    std::mem::forget(dir);
}

/// The catalogue after a change, so a rename or a delete shows everywhere at
/// once rather than only where it was made.
async fn refetch_labels(server: &Client) -> Vec<Action> {
    match server.labels().await {
        Ok(labels) => vec![Action::Labels(LabelAction::Loaded(labels))],
        Err(e) => {
            tracing::warn!(%e, "refetching labels failed");
            Vec::new()
        }
    }
}

/// Read, merge, write. The read is taken here rather than reused from startup
/// so a key the web UI changed meanwhile is not reverted, and a read that fails
/// cancels the write instead of replacing the row from the patch alone.
async fn save_settings(server: &Client, patch: serde_json::Value) -> Vec<Action> {
    // The version travels with the body from the same read the body is merged
    // into, so a server that has moved its settings version on is handed its
    // own number back rather than a stale constant.
    let (fresh, version) = match server.settings().await {
        Ok(payload) => (Some(payload.data), payload.version),
        Err(e) => {
            tracing::warn!(%e, "cannot read the settings to merge into");
            (None, SETTINGS_VERSION)
        }
    };
    let body = match crate::app::settings_write::plan(fresh, patch) {
        crate::app::settings_write::WritePlan::Put(body) => body,
        crate::app::settings_write::WritePlan::Refuse => {
            return vec![
                Action::SettingsWriteFailed,
                Action::Toast(
                    Level::Warn,
                    "could not read your settings — nothing was saved".to_owned(),
                ),
            ];
        }
    };
    if let Err(e) = server.put_settings(version, body.clone()).await {
        tracing::warn!(%e, "cannot save the settings");
        return vec![
            Action::SettingsWriteFailed,
            Action::Toast(Level::Warn, "could not save your settings".to_owned()),
        ];
    }
    vec![Action::SettingsSaved(Box::new(body))]
}

/// Both halves of the usage panel, in one round trip each. A half that fails is
/// reported on its own: pools and credentials are read by different scopes, so
/// losing one must not blank the other.
async fn fetch_usage(server: &Client) -> Vec<Action> {
    use crate::app::usage::UsageAction;

    let (pools, accounts) = tokio::join!(server.account_pools_usage(), server.accounts_usage());
    let mut out = Vec::new();
    let mut failure = None;
    match pools {
        Ok(rows) => out.push(Action::Usage(UsageAction::PoolsLoaded(rows))),
        Err(e) => {
            tracing::warn!(%e, "reading pool usage failed");
            failure = Some(usage_error(&e));
        }
    }
    match accounts {
        Ok(rows) => out.push(Action::Usage(UsageAction::AccountsLoaded(rows))),
        Err(e) => {
            tracing::warn!(%e, "reading account usage failed");
            failure = Some(usage_error(&e));
        }
    }
    if out.is_empty() {
        return vec![Action::Usage(UsageAction::Failed(
            failure.unwrap_or_else(|| "could not read usage".to_owned()),
        ))];
    }
    out
}

fn usage_error(e: &cctui_client::ClientError) -> String {
    match e {
        cctui_client::ClientError::Forbidden { .. } => "this key may not read usage".to_owned(),
        cctui_client::ClientError::Unauthorized => "the server rejected this key".to_owned(),
        other => format!("could not read usage: {other}"),
    }
}

/// Why the machines list is empty, in the words the view shows. A 403 is worth
/// naming on its own: it means the key cannot enumerate machines, which is a
/// different problem from having none.
fn machine_error(e: &cctui_client::ClientError) -> String {
    match e {
        cctui_client::ClientError::Forbidden { .. } => "this key may not list machines".to_owned(),
        cctui_client::ClientError::Unauthorized => "the server rejected this key".to_owned(),
        other => format!("could not list machines: {other}"),
    }
}

/// A 403 here means the key may not read them at all, which the view says
/// rather than looking like an install with no accounts.
fn account_error(e: &cctui_client::ClientError) -> String {
    match e {
        cctui_client::ClientError::Forbidden { .. } => "this key may not list accounts".to_owned(),
        cctui_client::ClientError::Unauthorized => "the server rejected this key".to_owned(),
        other => format!("could not list accounts: {other}"),
    }
}

/// A refused write turns the slice read-only; anything else is a one-off toast
/// that leaves the editing keys live.
fn account_refusal(e: &cctui_client::ClientError, what: &str) -> Action {
    tracing::warn!(%e, "{what}");
    if matches!(e, cctui_client::ClientError::Forbidden { .. }) {
        return Action::Accounts(AccountAction::Refused(format!("{what}: not allowed")));
    }
    Action::Toast(Level::Error, format!("{what}: {e}"))
}

fn pool_refusal(e: &cctui_client::ClientError, what: &str) -> Action {
    tracing::warn!(%e, "{what}");
    if matches!(e, cctui_client::ClientError::Forbidden { .. }) {
        return Action::Pools(PoolAction::Refused(format!("{what}: not allowed")));
    }
    Action::Toast(Level::Error, format!("{what}: {e}"))
}

/// Every account write refreshes the list and the rules: a redirect changes
/// what a row says about itself, not just the rule store.
async fn refetch_accounts(server: &Client) -> Vec<Action> {
    let mut out = match server.accounts().await {
        Ok(rows) => vec![Action::Accounts(AccountAction::Loaded(rows))],
        Err(e) => {
            tracing::warn!(%e, "refetching accounts failed");
            Vec::new()
        }
    };
    if let Ok(rules) = server.redirects().await {
        out.push(Action::Accounts(AccountAction::RedirectsLoaded(rules)));
    }
    out
}

/// A membership change moves accounts between pools, so both panes are refetched.
async fn refetch_pools(server: &Client) -> Vec<Action> {
    let mut out = match server.account_pools().await {
        Ok(rows) => vec![Action::Pools(PoolAction::Loaded(rows))],
        Err(e) => {
            tracing::warn!(%e, "refetching account pools failed");
            Vec::new()
        }
    };
    out.extend(refetch_accounts(server).await);
    out
}

async fn refetch_dispatchers(server: &Client) -> Vec<Action> {
    match server.dispatchers().await {
        Ok(rows) => vec![Action::Dispatchers(DispatcherAction::Loaded(rows))],
        Err(e) => {
            tracing::warn!(%e, "refetching dispatchers failed");
            Vec::new()
        }
    }
}

/// A 403 here means the key may not even list them, which is worth saying apart
/// from an install with none enrolled.
fn dispatcher_error(e: &cctui_client::ClientError) -> String {
    match e {
        cctui_client::ClientError::Forbidden { .. } => {
            "this key may not list dispatchers".to_owned()
        }
        cctui_client::ClientError::Unauthorized => "the server rejected this key".to_owned(),
        other => format!("could not list dispatchers: {other}"),
    }
}

/// The three spend reads in one round trip. The token windows and the analytics
/// are both load-bearing, so either failing fails the panel; cache-loss is a
/// footnote and degrades to empty.
async fn fetch_spend(server: &Client, tz_offset: i32) -> Vec<Action> {
    use crate::app::spend::{CACHE_LOSS_DAYS, RANGE_DAYS, SpendData};

    let (windows, analytics, cache_loss) = tokio::join!(
        server.session_token_stats(tz_offset),
        server.session_usage_analytics(RANGE_DAYS, tz_offset),
        server.cache_loss(CACHE_LOSS_DAYS, tz_offset),
    );
    let (windows, analytics) = match (windows, analytics) {
        (Ok(windows), Ok(analytics)) => (windows, analytics),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(%e, "reading spend failed");
            return vec![Action::Spend(SpendAction::Failed(spend_error(&e)))];
        }
    };
    if let Err(e) = &cache_loss {
        tracing::warn!(%e, "reading cache-bust loss failed");
    }
    vec![Action::Spend(SpendAction::Loaded(Box::new(SpendData {
        windows,
        analytics,
        cache_loss: cache_loss.unwrap_or_default(),
    })))]
}

fn spend_error(e: &cctui_client::ClientError) -> String {
    match e {
        cctui_client::ClientError::Forbidden { .. } => {
            "this key may not read usage stats".to_owned()
        }
        other => format!("could not read spend: {other}"),
    }
}

fn image_extension(content_type: &str) -> String {
    cctui_clientcore::uploads::ext_for_type(content_type)
}

/// Read a picture off the clipboard. `arboard` hands over raw RGBA, which is
/// re-encoded as PNG so the staged attachment is a real image file.
fn read_clipboard_image() -> Action {
    let image = match arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
        Ok(image) => image,
        Err(e) => {
            tracing::debug!(%e, "no image on the clipboard");
            return Action::Images(ImagesAction::NoImage);
        }
    };
    let (width, height) = (image.width as u32, image.height as u32);
    let Some(buffer) = image::RgbaImage::from_raw(width, height, image.bytes.into_owned()) else {
        return Action::Images(ImagesAction::NoImage);
    };
    let mut png = std::io::Cursor::new(Vec::new());
    if let Err(e) =
        image::DynamicImage::ImageRgba8(buffer).write_to(&mut png, image::ImageFormat::Png)
    {
        tracing::warn!(%e, "cannot encode the pasted image");
        return Action::Images(ImagesAction::NoImage);
    }
    Action::Images(ImagesAction::Pasted { png: png.into_inner(), width, height })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CONCURRENCY, Effect, Effects};
    use crate::app::Action;

    /// Accepts the connection and then answers nothing, standing in for a daemon
    /// that has stopped responding while its socket stays up.
    fn hung_server() -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let handle = std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept() {
                held.push(stream);
                if held.len() > 16 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_secs(20));
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn effects_against(base: &str) -> (Effects, tokio::sync::mpsc::Receiver<Action>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A request budget far longer than the test, so the read really does hang
        // rather than being rescued by its own timeout.
        let client = std::sync::Arc::new(cctui_client::Client::with_timeouts(
            base,
            "tok",
            Duration::from_secs(30),
            Duration::from_secs(30),
        ));
        let ws = std::sync::Arc::new(cctui_client::WsClient::start(client.ws_connector()).0);
        Effects::start(client, ws)
    }

    /// F8: a read that never comes back used to sit on the one lane and hold
    /// every later effect behind it.
    #[tokio::test]
    async fn a_hung_read_does_not_hold_up_a_later_effect() {
        let (base, _server) = hung_server();
        let (effects, mut actions) = effects_against(&base);

        effects.dispatch(Effect::FetchDiagnose { session_id: "s-hangs".to_owned() });
        // Local and ordered: it needs no server, so it can only be late if the
        // hung read is in front of it.
        effects.dispatch(Effect::Copy { text: "hi".to_owned(), label: "thing" });

        let action = tokio::time::timeout(Duration::from_secs(5), actions.recv())
            .await
            .expect("the ordered lane was still blocked by the hung read")
            .expect("an action");
        match action {
            Action::Toast(_, text) => assert!(text.contains("thing"), "got {text:?}"),
            _ => panic!("expected the copy's toast"),
        }
    }

    /// The permit must be taken inside the side task. One slow read fits under
    /// the cap and proves nothing; a full cap is what used to stall the lane.
    #[tokio::test]
    async fn a_saturated_side_lane_does_not_delay_a_send() {
        let (base, _server) = hung_server();
        let (effects, mut actions) = effects_against(&base);

        for i in 0..=CONCURRENCY {
            effects.dispatch(Effect::FetchDiagnose { session_id: format!("s-hangs-{i}") });
        }
        // Ordered and local: only a blocked lane can make this late.
        effects.dispatch(Effect::Copy { text: "hi".to_owned(), label: "answer" });

        let action = tokio::time::timeout(Duration::from_secs(5), actions.recv())
            .await
            .expect("a full side lane blocked the ordered lane")
            .expect("an action");
        match action {
            Action::Toast(_, text) => assert!(text.contains("answer"), "got {text:?}"),
            _ => panic!("expected the ordered effect's toast"),
        }
    }

    /// N1: a send dropped on a full queue had no deadline, so it sat at "Sending"
    /// with nothing left to report on it.
    #[tokio::test]
    async fn a_send_dropped_on_a_full_queue_is_failed_not_forgotten() {
        let (tx, _held) = tokio::sync::mpsc::channel::<Effect>(1);
        let (notify, mut actions) = tokio::sync::mpsc::channel::<Action>(4);
        let effects = Effects { tx, notify };

        let send = || Effect::SendMessage {
            send_id: 7,
            session_id: "s".to_owned(),
            content: "hello".to_owned(),
            ask_picks: None,
            turn_id: None,
            client_msg_id: None,
        };
        // Fills the one slot, then overflows: nothing drains this queue.
        effects.dispatch(send());
        effects.dispatch(send());

        match actions.try_recv() {
            Ok(Action::Send(crate::app::send::SendAction::DispatchFailed { send_id, .. })) => {
                assert_eq!(send_id, 7, "the dropped send is the one failed");
            }
            Ok(_) => panic!("a dropped send got a bare toast and no failure path"),
            Err(e) => panic!("a dropped send said nothing: {e:?}"),
        }
    }

    /// F24: quit queues the draft writes and then has to wait for them, or the
    /// process goes before they leave.
    #[tokio::test]
    async fn drain_waits_for_what_is_already_queued() {
        let (base, _server) = hung_server();
        let (effects, mut actions) = effects_against(&base);

        effects.dispatch(Effect::Copy { text: "hi".to_owned(), label: "flushed" });
        tokio::time::timeout(Duration::from_secs(5), effects.drain(Duration::from_secs(5)))
            .await
            .expect("the barrier never came back");

        // The copy ran before the barrier did, so its action is already waiting.
        let action = actions.try_recv().expect("the queued effect ran before the drain returned");
        match action {
            Action::Toast(_, text) => assert!(text.contains("flushed"), "got {text:?}"),
            _ => panic!("expected the copy's toast"),
        }
    }

    /// R10 residual: cancelling a create that is already on the wire would throw
    /// away the row id it is about to report, and the next save would mint a
    /// second row.
    #[tokio::test]
    async fn an_autosave_already_on_the_wire_is_not_cancelled() {
        let (base, _server) = hung_server();
        let (_effects, action_rx) = effects_against(&base);
        drop(action_rx);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let client = std::sync::Arc::new(cctui_client::Client::with_timeouts(
            &base,
            "tok",
            Duration::from_secs(30),
            Duration::from_secs(30),
        ));
        let saver = super::DraftSaver::new(client, tx);
        let request = || {
            let mut form = crate::app::spawn::SpawnForm::new();
            form.fields.machine_id = "m-1".to_owned();
            form.fields.working_dir = "/w".to_owned();
            Box::new(form.request())
        };

        saver.autosave(None, request(), true, 1);
        // `immediate` means the request is already going out; give it the tick it
        // needs to reach the hung server and raise its flag.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(saver.spawn_create_in_flight(), "the create is past the point of no return");

        saver.cancel_spawn_autosave();
        assert!(saver.spawn_create_in_flight(), "it is let go of, not aborted");

        // And a save that arrives while it is in flight does not mint a rival row.
        saver.autosave(None, request(), true, 1);
        assert!(
            saver.spawn_create_in_flight(),
            "the second save is skipped until the first one's id lands"
        );
    }

    /// Answers 200 to everything, on every connection it is given.
    ///
    /// It has to keep accepting: building the client also starts a websocket to
    /// the same host, so a one-shot listener is spent before the request under
    /// test ever arrives.
    fn accepting_server() -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming().take(32) {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut buf = [0u8; 8192];
                    let _ = stream.read(&mut buf);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                    );
                    let _ = stream.flush();
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    /// The other half of the flush: once the server has the text, the local copy
    /// must go, or the next start would restore text that is already saved.
    #[tokio::test]
    async fn a_draft_the_server_takes_is_dropped_from_the_local_copy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        let owner = crate::config::recovery::Owner::new("https://one.example", "user-alice");
        crate::config::recovery::write_to(
            &path,
            &owner,
            std::iter::once(("draft:s-a".to_owned(), "saved after all".to_owned())).collect(),
        );
        assert!(path.exists(), "the fixture needs a local copy to start from");

        let (base, _server) = accepting_server();
        let (effects, _actions) = effects_against(&base);
        effects.dispatch(Effect::SaveDraftNow {
            key: "draft:s-a".to_owned(),
            text: "saved after all".to_owned(),
            recovery: Some(crate::config::recovery::Target {
                owner: owner.clone(),
                path: path.clone(),
            }),
        });
        effects.drain(Duration::from_secs(10)).await;

        assert!(
            crate::config::recovery::load_from(&path, &owner).is_empty(),
            "a confirmed save must take its key with it"
        );
        assert!(!path.exists(), "and the file goes once nothing is left to recover");
    }

    /// R13: the whole point of the flush is the case where the server is gone,
    /// so the text has to land somewhere the next start can find it.
    #[tokio::test]
    async fn a_draft_the_server_refuses_is_kept_on_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        let owner = crate::config::recovery::Owner::new("https://one.example", "user-alice");
        // The local copy is written by `on_quit` before the save is dispatched;
        // this is the half that must leave it alone when the server never takes it.
        crate::config::recovery::write_to(
            &path,
            &owner,
            std::iter::once(("draft:s-a".to_owned(), "the text the server never took".to_owned()))
                .collect(),
        );

        // Nothing listening at all, so the PUT fails rather than hanging.
        let (effects, _actions) = effects_against("http://127.0.0.1:1");
        effects.dispatch(Effect::SaveDraftNow {
            key: "draft:s-a".to_owned(),
            text: "the text the server never took".to_owned(),
            recovery: Some(crate::config::recovery::Target {
                owner: owner.clone(),
                path: path.clone(),
            }),
        });
        effects.drain(Duration::from_secs(10)).await;

        let held = crate::config::recovery::load_from(&path, &owner);
        assert_eq!(
            held.drafts.get("draft:s-a").map(String::as_str),
            Some("the text the server never took"),
            "a refused flush must leave the local copy in place"
        );
    }

    /// A server that stopped answering costs the budget, not the exit.
    #[tokio::test]
    async fn drain_gives_up_after_its_budget() {
        let (base, _server) = hung_server();
        let (effects, _actions) = effects_against(&base);

        effects.dispatch(Effect::SaveDraftNow {
            key: "k".to_owned(),
            text: "t".to_owned(),
            recovery: None,
        });
        let started = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(10), effects.drain(Duration::from_millis(300)))
            .await
            .expect("drain must return on its own");
        assert!(started.elapsed() < Duration::from_secs(5), "it waited past its budget");
    }

    /// Anything that changes server state, or reads back something just changed,
    /// stays on the ordered lane.
    #[test]
    fn only_self_contained_reads_leave_the_ordered_lane() {
        for ordered in [
            Effect::SendMessage {
                send_id: 1,
                session_id: "s".to_owned(),
                content: "a".to_owned(),
                ask_picks: None,
                turn_id: None,
                client_msg_id: None,
            },
            Effect::Interrupt { session_id: "s".to_owned() },
            Effect::RenameSession { session_id: "s".to_owned(), name: "n".to_owned() },
            Effect::SaveDraft { key: "k".to_owned(), text: "t".to_owned() },
            Effect::RefreshSessions,
            Effect::MarkSeen { session_id: "s".to_owned() },
            Effect::Subscribe { session_id: "s".to_owned() },
        ] {
            assert!(!ordered.runs_concurrently(), "this effect must keep its order");
        }

        for independent in [
            Effect::FetchDiagnose { session_id: "s".to_owned() },
            Effect::LoadConversationPage {
                session_id: "s".to_owned(),
                kind: super::PageKind::Latest,
                claim: None,
                page: super::PageRequest { before: None, after: None, limit: None },
                etag: None,
            },
            Effect::FetchSelfUpdateRun,
            Effect::FetchRecentDirs,
        ] {
            assert!(independent.runs_concurrently(), "this effect can run beside the lane");
        }
    }

    /// F9: the staged name used to be interpolated straight into a path under a
    /// shared /tmp, so a crafted name could escape the directory.
    #[test]
    fn a_staged_name_is_one_harmless_path_component() {
        use crate::app::fileview::staged_file_name;
        for (given, want) in [
            ("report.pdf", "report.pdf"),
            ("../../etc/passwd", "passwd"),
            ("/etc/shadow", "shadow"),
            ("..", "file"),
            ("", "file"),
            (".bashrc", "bashrc"),
            ("a b;rm -rf x.txt", "a_b_rm_-rf_x.txt"),
            ("x/y\\z.bin", "z.bin"),
        ] {
            let got = staged_file_name(given);
            assert_eq!(got, want, "{given:?}");
            assert!(!got.contains('/') && !got.contains('\\'), "{given:?} -> {got:?}");
            assert!(got != ".." && !got.starts_with('.'), "{given:?} -> {got:?}");
        }
        assert!(staged_file_name(&"n".repeat(500)).len() <= 96, "a name cannot be unbounded");
    }

    /// F9: a predictable path on a shared /tmp could be pre-planted as a symlink,
    /// and `fs::write` would follow it.
    #[cfg(unix)]
    #[test]
    fn staged_bytes_are_private_to_this_user() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::Builder::new().prefix("cctui-").tempdir().expect("a dir");
        std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(0o700)).expect("chmod");
        let path = dir.path().join(crate::app::fileview::staged_file_name("secret.bin"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path).expect("a fresh file");
        std::io::Write::write_all(&mut file, b"x").expect("write");

        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "nobody else may read a staged session file");
        let dir_mode =
            std::fs::metadata(dir.path()).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "the directory is private too");

        // create_new is what refuses a pre-planted path rather than following it.
        let mut again = std::fs::OpenOptions::new();
        again.write(true).create_new(true);
        assert!(again.open(&path).is_err(), "staging must never write through an existing path");
    }

    /// F8: a full queue used to drop the effect with only a log line, so a key
    /// press simply did nothing.
    #[tokio::test]
    async fn a_dropped_effect_is_reported_instead_of_vanishing() {
        let (tx, _held) = tokio::sync::mpsc::channel::<Effect>(1);
        let (notify, mut actions) = tokio::sync::mpsc::channel::<Action>(4);
        let effects = Effects { tx, notify };

        // Fills the one slot, then overflows: nothing is draining this queue.
        effects.dispatch(Effect::RefreshSessions);
        effects.dispatch(Effect::RefreshSessions);

        match actions.try_recv() {
            Ok(Action::Toast(level, text)) => {
                assert_eq!(level, crate::app::toast::Level::Error);
                assert!(text.contains("dropped"), "got {text:?}");
            }
            Ok(_) => panic!("a dropped effect produced some other action"),
            Err(e) => panic!("a dropped effect said nothing: {e:?}"),
        }
    }
}
