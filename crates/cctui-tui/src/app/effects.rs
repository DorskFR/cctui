use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_client::{Client, ConversationFetch, FileRead, Page, UploadFile, WsClient};
use cctui_proto::drafts::{composer_draft_key, session_history_key};
use cctui_proto::ws::AgentEvent;
use tokio::sync::mpsc;

use super::action::{Action, Effect};
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
use super::fileview::{self, FileViewAction};
use super::identity::AuthAction;
use super::labels::LabelAction;
use super::line::agent_event_to_line;
use super::machines::MachineAction;
use super::pins::PinAction;
use super::profiles::ProfileAction;
use super::send::SendAction;
use super::slice::SliceAction;
use super::spawn::{SpawnAction, SpawnFetch};
use super::spawn_drafts::SpawnDraftAction;
use super::state::{ConversationLine, PendingPermission};
use super::toast::Level;

const QUEUE: usize = 256;

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
}

impl Effects {
    /// Effects are executed one at a time: two messages typed in quick
    /// succession must reach the server in the order they were sent.
    pub fn start(server: Arc<Client>, ws: Arc<WsClient>) -> (Self, mpsc::Receiver<Action>) {
        let (tx, mut rx) = mpsc::channel::<Effect>(QUEUE);
        let (action_tx, action_rx) = mpsc::channel::<Action>(QUEUE);

        tokio::spawn(async move {
            let mut drafts = DraftSaver::new(Arc::clone(&server));
            while let Some(effect) = rx.recv().await {
                for action in run(&server, &ws, &mut drafts, effect).await {
                    if action_tx.send(action).await.is_err() {
                        return;
                    }
                }
            }
        });

        (Self { tx }, action_rx)
    }

    pub fn dispatch(&self, effect: Effect) {
        if self.tx.try_send(effect).is_err() {
            tracing::warn!("effect queue full; dropping effect");
        }
    }

    pub fn dispatch_all(&self, effects: Vec<Effect>) {
        for effect in effects {
            self.dispatch(effect);
        }
    }
}

/// Per-key debounce for draft writes: a pending save is replaced, not queued,
/// and the request is off the effect queue so typing never waits on it.
struct DraftSaver {
    server: Arc<Client>,
    pending: HashMap<String, tokio::task::JoinHandle<()>>,
}

impl DraftSaver {
    fn new(server: Arc<Client>) -> Self {
        Self { server, pending: HashMap::new() }
    }

    fn save(&mut self, key: String, text: String) {
        self.pending.retain(|_, handle| !handle.is_finished());
        self.cancel(&key);
        let server = Arc::clone(&self.server);
        let target = key.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(DRAFT_DEBOUNCE).await;
            if let Err(e) = server.put_draft(&target, &text).await {
                tracing::warn!(%e, "draft save failed");
            }
        });
        self.pending.insert(key, handle);
    }

    /// One pending autosave at a time, keyed on the dialog rather than a
    /// draft id: the first save is what mints the id.
    fn autosave(
        &mut self,
        session_id: Option<String>,
        request: Box<cctui_proto::api::SpawnRequest>,
    ) {
        const KEY: &str = "\u{0}spawn-draft";
        self.cancel(KEY);
        let server = Arc::clone(&self.server);
        let handle = tokio::spawn(async move {
            tokio::time::sleep(DRAFT_DEBOUNCE).await;
            let outcome = match session_id.as_deref() {
                Some(id) => server.update_draft(id, &request).await.map(|_| ()),
                // An autosave stores names, never bytes: the files go up at launch.
                None => server.spawn_session(&request, Vec::new()).await,
            };
            if let Err(e) = outcome {
                tracing::warn!(%e, "autosaving the spawn draft failed");
            }
        });
        self.pending.insert(KEY.to_owned(), handle);
    }

    fn cancel(&mut self, key: &str) {
        if let Some(handle) = self.pending.remove(key) {
            handle.abort();
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run(
    server: &Client,
    ws: &WsClient,
    drafts: &mut DraftSaver,
    effect: Effect,
) -> Vec<Action> {
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
        Effect::LoadConversationPage { session_id, kind, page, etag } => {
            load_conversation_page(server, &session_id, kind, page, etag.as_deref()).await
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
        Effect::AutosaveDraft { session_id, request } => {
            drafts.autosave(session_id, request);
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
        Effect::SendMessage { send_id, session_id, content, ask_picks, turn_id } => {
            let client_msg_id = uuid::Uuid::new_v4().to_string();
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
        Effect::Fork { session_id } => match server.fork(&session_id).await {
            Ok(resp) => vec![Action::Controls(ControlsAction::Forked(resp.session_id))],
            Err(e) => {
                tracing::warn!(%e, "fork failed");
                vec![Action::Toast(Level::Error, format!("fork failed: {e}"))]
            }
        },
        Effect::FetchHarnessModels { harness, machine_id, model } => {
            match server.harness_models(&harness, Some(&machine_id), &model).await {
                Ok(models) => {
                    vec![Action::Controls(ControlsAction::ModelsLoaded(Box::new(models)))]
                }
                Err(e) => {
                    tracing::warn!(%e, "harness model list fetch failed");
                    vec![Action::Toast(Level::Warn, "could not read the model list".to_owned())]
                }
            }
        }
        Effect::SetModel { session_id, model, effort } => {
            match server.set_model(&session_id, Some(&model), Some(&effort)).await {
                Ok(()) => vec![Action::Controls(ControlsAction::ModelSet { model, effort })],
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
        Effect::ReadAttachment { session_id, path } => read_attachment(&session_id, &path),
        Effect::UploadAttachments { session_id, content, files } => {
            upload_attachments(server, session_id, content, files).await
        }
        Effect::OpenLinkedFile { session_id, machine_id, path } => {
            open_linked_file(server, &session_id, &machine_id, &path).await
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
        Effect::FetchMachines => match server.machines().await {
            Ok(rows) => vec![Action::Machines(MachineAction::Loaded(rows))],
            Err(e) => {
                tracing::warn!(%e, "listing machines failed");
                vec![Action::Machines(MachineAction::Failed(machine_error(&e)))]
            }
        },
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
        Effect::FetchAccounts => match server.accounts().await {
            Ok(accounts) => vec![spawn_data(SpawnFetch::Accounts(accounts))],
            Err(e) => {
                tracing::warn!(%e, "listing accounts failed");
                vec![Action::Toast(Level::Warn, "could not list accounts".to_owned())]
            }
        },
        Effect::FetchAccountPools => match server.account_pools().await {
            Ok(pools) => vec![spawn_data(SpawnFetch::Pools(pools))],
            Err(e) => {
                tracing::warn!(%e, "listing account pools failed");
                Vec::new()
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
        Effect::SpawnSession { request, files } => {
            let files = files
                .into_iter()
                .map(|(name, bytes)| cctui_client::UploadFile { name, bytes })
                .collect();
            match server.spawn_session(&request, files).await {
                Ok(()) => Vec::new(),
                Err(e) => {
                    tracing::warn!(%e, "the spawn request failed");
                    vec![Action::Spawn(super::spawn::SpawnAction::Failed(e.to_string()))]
                }
            }
        }
        Effect::SaveSettings { data } => {
            // The version the server last reported travels with the blob; it
            // migrates an older payload forward rather than rejecting it.
            if let Err(e) = server.put_settings(SETTINGS_VERSION, data).await {
                tracing::warn!(%e, "cannot save the list settings");
                return vec![Action::Toast(
                    Level::Warn,
                    "could not save the list settings".to_owned(),
                )];
            }
            Vec::new()
        }
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
            })];
        }
    };

    let ConversationFetch::Page { rows, etag, has_more } = fetch else {
        return vec![Action::Conversation(ConversationAction::NotModified {
            session_id: session_id.to_owned(),
            kind,
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
fn read_attachment(session_id: &str, path: &str) -> Vec<Action> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return vec![Action::Attach(AttachAction::ReadFailed(format!(
                "cannot read {path}: {e}"
            )))];
        }
    };
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    if name.is_empty() {
        return vec![Action::Attach(AttachAction::ReadFailed(format!("{path} is not a file")))];
    }
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

/// Write the bytes to a temp file and hand it to the desktop's opener. A
/// terminal that cannot draw images still gets the user to the picture.
fn open_in_os_viewer(name: &str, bytes: &[u8]) {
    let path = std::env::temp_dir().join(format!("cctui-{name}"));
    if let Err(e) = std::fs::write(&path, bytes) {
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
