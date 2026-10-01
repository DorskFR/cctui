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
use super::controls::ControlsAction;
use super::conversation::ConversationAction;
use super::conversation_store::{PageKind, PageRequest};
use super::diagnose::DiagnoseAction;
use super::drafts::DraftAction;
use super::fileview::{self, FileViewAction};
use super::identity::AuthAction;
use super::line::agent_event_to_line;
use super::pins::PinAction;
use super::send::SendAction;
use super::state::{ConversationLine, PendingPermission};
use super::toast::Level;

const QUEUE: usize = 256;

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
        Effect::SaveUiState(state) => {
            crate::config::uistate::save(&state);
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
fn guess_content_type(name: &str) -> String {
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
