//! Composer attachments: staged locally, uploaded on send.
//!
//! Nothing here touches the network or the disk — reading a path and staging the
//! bytes are [`Effect`]s — so the caps, the naming and the token rewriting stay
//! testable against `cctui_clientcore::uploads`, which is also what the webui
//! uses.

use std::collections::HashMap;

use cctui_clientcore::uploads::{
    UploadCaps, append_file_tokens, cap_error, default_caps, fmt_size, merge_renamed,
    next_paste_index, rewrite_file_tokens,
};

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// Pasted text at or above this many bytes becomes an attachment instead of
/// flooding the composer.
pub const PASTE_THRESHOLD_BYTES: usize = 4 * 1024;

/// One staged file, held in memory until the send that uploads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub bytes: Vec<u8>,
    pub content_type: String,
    /// Pixel size, when it is an image the `image` crate could measure.
    pub dimensions: Option<(u32, u32)>,
}

impl Attachment {
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    #[must_use]
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// `[image: shot.png 1280x720 340 KB]` for an image, `[design.pdf 1.2 MB]`
    /// otherwise; the chip row wraps this in its own brackets.
    #[must_use]
    pub fn chip_label(&self) -> String {
        let size = fmt_size(self.size());
        match (self.is_image(), self.dimensions) {
            (true, Some((w, h))) => format!("image: {} {w}x{h} {size}", self.name),
            (true, None) => format!("image: {} {size}", self.name),
            (false, _) => format!("{} {size}", self.name),
        }
    }
}

/// What a session has staged, plus the names it already uploaded so
/// `paste-N.txt` keeps counting up across messages.
#[derive(Debug, Default, Clone)]
pub struct Stage {
    pub items: Vec<Attachment>,
    pub used_names: Vec<String>,
    /// The cap breach the last attempt hit, shown above the composer.
    pub error: Option<String>,
}

impl Stage {
    fn names(&self) -> Vec<String> {
        self.items.iter().map(|a| a.name.clone()).collect()
    }

    fn sizes(&self) -> Vec<u64> {
        self.items.iter().map(Attachment::size).collect()
    }
}

/// Every session's staging area, plus which chip `Backspace` removes.
#[derive(Debug, Default)]
pub struct Attachments {
    stages: HashMap<String, Stage>,
    /// `Some` only while the chip row has focus; the composer has it otherwise.
    pub chip_cursor: Option<usize>,
    caps: UploadCaps,
}

impl Attachments {
    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<&Stage> {
        self.stages.get(session_id).filter(|s| !s.items.is_empty() || s.error.is_some())
    }

    #[must_use]
    pub fn items(&self, session_id: &str) -> &[Attachment] {
        self.stages.get(session_id).map_or(&[], |s| s.items.as_slice())
    }

    #[must_use]
    pub fn is_empty(&self, session_id: &str) -> bool {
        self.items(session_id).is_empty()
    }

    fn stage_mut(&mut self, session_id: &str) -> &mut Stage {
        self.stages.entry(session_id.to_owned()).or_default()
    }

    #[cfg(test)]
    pub const fn caps(&self) -> UploadCaps {
        self.caps
    }
}

impl Attachments {
    #[must_use]
    pub fn new() -> Self {
        Self { stages: HashMap::new(), chip_cursor: None, caps: default_caps() }
    }
}

pub enum AttachAction {
    /// A path's bytes arrived from the disk.
    Read {
        session_id: String,
        name: String,
        bytes: Vec<u8>,
        content_type: String,
        dimensions: Option<(u32, u32)>,
    },
    /// Reading a path failed, with the reason already worded.
    ReadFailed(String),
    /// A bracketed paste large enough to become `paste-N.txt`.
    LargePaste(String),
    /// Move focus into the chip row, or between chips.
    FocusChips,
    MoveChip(i32),
    /// `Backspace` in the composer: a chip when one is focused, text otherwise.
    BackspaceOrChip,
    /// Staging succeeded: `paths` is the absolute path per name, in order.
    Uploaded {
        session_id: String,
        content: String,
        names: Vec<String>,
        paths: Vec<String>,
    },
    UploadFailed {
        session_id: String,
        content: String,
        message: String,
    },
}

pub fn reduce_attach(app: &mut App, action: AttachAction) -> Vec<Effect> {
    match action {
        AttachAction::Read { session_id, name, bytes, content_type, dimensions } => {
            add(app, &session_id, name, bytes, content_type, dimensions)
        }
        AttachAction::ReadFailed(message) => {
            app.toast(Level::Error, message);
            Vec::new()
        }
        AttachAction::LargePaste(text) => large_paste(app, text),
        AttachAction::FocusChips => {
            focus_chips(app);
            Vec::new()
        }
        AttachAction::MoveChip(delta) => {
            move_chip(app, delta);
            Vec::new()
        }
        // Backspace belongs to the composer unless a chip has the focus, or the
        // key could never delete a character again.
        AttachAction::BackspaceOrChip => {
            if app.attachments.chip_cursor.is_some() {
                return remove_focused(app);
            }
            app.message_input.input(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Backspace,
                crossterm::event::KeyModifiers::NONE,
            ));
            super::drafts::on_input(app)
        }
        AttachAction::Uploaded { session_id, content, names, paths } => {
            let content = rewrite_file_tokens(&content, &names, &paths);
            app.attachments
                .stage_mut(&session_id)
                .used_names
                .extend(paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_owned()));
            app.attachments.stage_mut(&session_id).items.clear();
            app.attachments.chip_cursor = None;
            super::send::submit(app, session_id, content, None)
        }
        AttachAction::UploadFailed { session_id, content, message } => {
            // The staged files stay put so the send can be retried; the composer
            // gets its text back rather than losing it to a failed upload.
            app.attachments.stage_mut(&session_id).error = Some(message.clone());
            app.set_input_text(&content);
            app.toast(Level::Error, message);
            Vec::new()
        }
    }
}

/// Stage one file, renaming a clash and rejecting a cap breach with the same
/// wording the webui uses.
fn add(
    app: &mut App,
    session_id: &str,
    name: String,
    bytes: Vec<u8>,
    content_type: String,
    dimensions: Option<(u32, u32)>,
) -> Vec<Effect> {
    let caps = app.attachments.caps;
    let stage = app.attachments.stage_mut(session_id);
    let mut existing = stage.names();
    let added = merge_renamed(&mut existing, &[name]);
    let unique = added.into_iter().next().unwrap_or_default();

    let mut sizes = stage.sizes();
    sizes.push(bytes.len() as u64);
    if let Some(error) = cap_error(&sizes, caps) {
        stage.error = Some(error.clone());
        app.toast(Level::Error, error);
        return Vec::new();
    }

    stage.error = None;
    stage.items.push(Attachment { name: unique.clone(), bytes, content_type, dimensions });
    // Appending the token is a composer edit like any other, so it goes through
    // the draft store: otherwise the next session switch would restore a draft
    // that never heard about it.
    let text = app.message_input.lines().join("\n");
    app.set_input_text(&append_file_tokens(&text, &[unique]));
    super::drafts::on_input(app)
}

/// A paste past the threshold becomes `paste-N.txt` rather than thousands of
/// columns in the composer.
fn large_paste(app: &mut App, text: String) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let stage = app.attachments.stage_mut(&session_id);
    let names = stage.names();
    let used = stage.used_names.clone();
    let draft = app.message_input.lines().join("\n");
    let index =
        next_paste_index(names.iter().map(String::as_str), &draft, used.iter().map(String::as_str));
    let name = format!("paste-{index}.txt");
    add(app, &session_id, name, text.into_bytes(), "text/plain".to_owned(), None)
}

fn focus_chips(app: &mut App) {
    let Some(session_id) = app.selected_session_id() else { return };
    let count = app.attachments.items(&session_id).len();
    if count > 0 {
        app.attachments.chip_cursor = Some(count - 1);
    }
}

fn move_chip(app: &mut App, delta: i32) {
    let Some(session_id) = app.selected_session_id() else { return };
    let count = app.attachments.items(&session_id).len();
    if count == 0 {
        app.attachments.chip_cursor = None;
        return;
    }
    let current = app.attachments.chip_cursor.unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        current.saturating_add(delta.unsigned_abs() as usize)
    };
    app.attachments.chip_cursor = Some(next.min(count - 1));
}

fn remove_focused(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let Some(at) = app.attachments.chip_cursor else { return Vec::new() };
    let stage = app.attachments.stage_mut(&session_id);
    if at >= stage.items.len() {
        app.attachments.chip_cursor = None;
        return Vec::new();
    }
    let removed = stage.items.remove(at);
    stage.error = None;
    let remaining = stage.items.len();
    app.attachments.chip_cursor = (remaining > 0).then(|| at.min(remaining - 1));

    // The `[name]` token goes with the chip, or the agent is told to read a file
    // that was never uploaded.
    let text = app.message_input.lines().join("\n");
    let stripped = text.replace(&format!("[{}]", removed.name), "");
    app.set_input_text(stripped.trim());
    super::drafts::on_input(app)
}

/// The upload a send has to do first, or `None` when nothing is staged and the
/// send can go straight out.
#[must_use]
pub fn upload_effect(app: &App, session_id: &str, content: &str) -> Option<Effect> {
    if app.attachments.is_empty(session_id) {
        return None;
    }
    let items = app.attachments.items(session_id);
    Some(Effect::UploadAttachments {
        session_id: session_id.to_owned(),
        content: content.to_owned(),
        files: items.iter().map(|a| (a.name.clone(), a.bytes.clone())).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::{AttachAction, PASTE_THRESHOLD_BYTES};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app.router.push(crate::app::View::Conversation);
        // Opening a conversation binds the composer to it; without that the
        // draft sync would treat the first edit as a session switch.
        let _ = crate::app::drafts::sync_composer(&mut app);
        app
    }

    fn attach(app: &mut App, action: AttachAction) -> Vec<Effect> {
        reduce(app, Action::Attach(action))
    }

    fn read(name: &str, bytes: usize) -> AttachAction {
        AttachAction::Read {
            session_id: "s-a".to_owned(),
            name: name.to_owned(),
            bytes: vec![b'x'; bytes],
            content_type: "text/plain".to_owned(),
            dimensions: None,
        }
    }

    fn names(app: &App) -> Vec<String> {
        app.attachments.items("s-a").iter().map(|a| a.name.clone()).collect()
    }

    fn text(app: &App) -> String {
        app.message_input.lines().join("\n")
    }

    #[test]
    fn a_staged_file_gets_a_chip_and_a_token_in_the_draft() {
        let mut app = app();
        attach(&mut app, read("design.pdf", 12));
        assert_eq!(names(&app), ["design.pdf"]);
        assert_eq!(text(&app), "[design.pdf]");
        assert_eq!(app.attachments.items("s-a")[0].chip_label(), "design.pdf 12 B");
    }

    #[test]
    fn a_clashing_name_is_renamed_rather_than_replaced() {
        let mut app = app();
        attach(&mut app, read("a.txt", 1));
        attach(&mut app, read("a.txt", 1));
        assert_eq!(names(&app), ["a.txt", "a-2.txt"]);
        assert_eq!(text(&app), "[a.txt] [a-2.txt]");
    }

    #[test]
    fn a_large_paste_becomes_a_numbered_text_attachment() {
        let mut app = app();
        let big = "x".repeat(PASTE_THRESHOLD_BYTES);
        attach(&mut app, AttachAction::LargePaste(big.clone()));
        assert_eq!(names(&app), ["paste-1.txt"]);
        assert_eq!(app.attachments.items("s-a")[0].bytes.len(), big.len());
        assert_eq!(text(&app), "[paste-1.txt]");

        attach(&mut app, AttachAction::LargePaste(big));
        assert_eq!(names(&app), ["paste-1.txt", "paste-2.txt"]);
    }

    #[test]
    fn paste_numbering_continues_past_what_was_already_uploaded() {
        let mut app = app();
        attach(
            &mut app,
            AttachAction::Uploaded {
                session_id: "s-a".to_owned(),
                content: "hi".to_owned(),
                names: vec!["paste-1.txt".to_owned()],
                paths: vec!["/w/paste-1.txt".to_owned()],
            },
        );
        attach(&mut app, AttachAction::LargePaste("x".repeat(10)));
        assert_eq!(names(&app), ["paste-2.txt"], "a fresh draft must not reuse paste-1");
    }

    #[test]
    fn a_cap_breach_is_refused_with_the_web_ui_wording() {
        let mut app = app();
        let over = app.attachments.caps().max_file_bytes as usize + 1;
        attach(&mut app, read("huge.bin", over));
        assert!(names(&app).is_empty(), "nothing is staged");
        assert_eq!(
            app.attachments.get("s-a").and_then(|s| s.error.clone()).as_deref(),
            Some("A file exceeds the 5.0 MB per-file cap")
        );
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn too_many_files_is_refused_on_the_one_that_breaks_the_cap() {
        let mut app = app();
        let max = app.attachments.caps().max_files as usize;
        for i in 0..max {
            attach(&mut app, read(&format!("f{i}.txt"), 1));
        }
        assert_eq!(names(&app).len(), max);
        attach(&mut app, read("one-too-many.txt", 1));
        assert_eq!(names(&app).len(), max, "the list is unchanged");
        assert_eq!(
            app.attachments.get("s-a").and_then(|s| s.error.clone()).as_deref(),
            Some("Too many files (max 10)")
        );
    }

    #[test]
    fn backspace_on_a_focused_chip_removes_it_and_its_token() {
        let mut app = app();
        attach(&mut app, read("a.txt", 1));
        attach(&mut app, read("b.txt", 1));
        attach(&mut app, AttachAction::FocusChips);
        assert_eq!(app.attachments.chip_cursor, Some(1));

        attach(&mut app, AttachAction::BackspaceOrChip);
        assert_eq!(names(&app), ["a.txt"]);
        assert_eq!(text(&app), "[a.txt]");
        assert_eq!(app.attachments.chip_cursor, Some(0));

        attach(&mut app, AttachAction::BackspaceOrChip);
        assert!(names(&app).is_empty());
        assert_eq!(app.attachments.chip_cursor, None, "no chips left to focus");
    }

    #[test]
    fn the_chip_cursor_stays_inside_the_row() {
        let mut app = app();
        attach(&mut app, read("a.txt", 1));
        attach(&mut app, read("b.txt", 1));
        attach(&mut app, AttachAction::FocusChips);
        attach(&mut app, AttachAction::MoveChip(-5));
        assert_eq!(app.attachments.chip_cursor, Some(0));
        attach(&mut app, AttachAction::MoveChip(9));
        assert_eq!(app.attachments.chip_cursor, Some(1));
    }

    #[test]
    fn sending_uploads_first_and_only_then_submits() {
        let mut app = app();
        attach(&mut app, read("a.txt", 3));
        let effects = reduce(&mut app, Action::SubmitInput);
        match effects.as_slice() {
            [Effect::UploadAttachments { session_id, content, files }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(content, "[a.txt]");
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].0, "a.txt");
            }
            other => panic!("expected one upload effect, got {}", other.len()),
        }
        assert!(app.outbox.tracked().next().is_none(), "nothing is sent before the upload lands");
    }

    #[test]
    fn a_completed_upload_rewrites_tokens_to_the_staged_names_and_sends() {
        let mut app = app();
        attach(&mut app, read("paste-1.txt", 3));
        let effects = attach(
            &mut app,
            AttachAction::Uploaded {
                session_id: "s-a".to_owned(),
                content: "see [paste-1.txt]".to_owned(),
                names: vec!["paste-1.txt".to_owned()],
                paths: vec!["/w/paste-1-1.txt".to_owned()],
            },
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::SendMessage { .. })));
        let sent = app.outbox.tracked().next().expect("a tracked send");
        assert_eq!(sent.content, "see [paste-1-1.txt]", "the token follows the staged name");
        assert!(app.attachments.is_empty("s-a"), "the chips go with the send");
    }

    #[test]
    fn a_failed_upload_keeps_the_chips_and_gives_the_text_back() {
        let mut app = app();
        attach(&mut app, read("a.txt", 3));
        attach(
            &mut app,
            AttachAction::UploadFailed {
                session_id: "s-a".to_owned(),
                content: "keep me [a.txt]".to_owned(),
                message: "the daemon is offline".to_owned(),
            },
        );
        assert_eq!(names(&app), ["a.txt"], "retryable");
        assert_eq!(text(&app), "keep me [a.txt]");
        assert!(app.outbox.tracked().next().is_none());
    }

    #[test]
    fn a_send_with_nothing_staged_skips_the_upload() {
        let mut app = app();
        app.input_active = true;
        app.message_input.insert_str("plain text");
        let effects = reduce(&mut app, Action::SubmitInput);
        assert!(effects.iter().any(|e| matches!(e, Effect::SendMessage { .. })));
        assert!(!effects.iter().any(|e| matches!(e, Effect::UploadAttachments { .. })));
    }

    #[test]
    fn an_image_chip_names_its_dimensions() {
        let mut app = app();
        attach(
            &mut app,
            AttachAction::Read {
                session_id: "s-a".to_owned(),
                name: "shot.png".to_owned(),
                bytes: vec![0; 340 * 1024],
                content_type: "image/png".to_owned(),
                dimensions: Some((1280, 720)),
            },
        );
        let item = &app.attachments.items("s-a")[0];
        assert!(item.is_image());
        assert_eq!(item.chip_label(), "image: shot.png 1280x720 340 KB");
    }
}
