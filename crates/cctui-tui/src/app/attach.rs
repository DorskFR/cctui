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
    /// `Ctrl-O`: open the path prompt.
    OpenPrompt,
    ClosePrompt,
    PromptKey(crossterm::event::KeyEvent),
    /// `Tab` in the prompt: complete the longest unambiguous path.
    CompletePrompt,
    /// The prompt was accepted; its text is a path to read.
    SubmitPrompt,
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
        AttachAction::OpenPrompt => {
            app.attach_prompt = Some(AttachPrompt::default());
            app.router.push(super::state::View::AttachPrompt);
            Vec::new()
        }
        AttachAction::ClosePrompt => {
            close_prompt(app);
            Vec::new()
        }
        AttachAction::PromptKey(key) => {
            if let Some(prompt) = app.attach_prompt.as_mut() {
                prompt.key(key);
            }
            Vec::new()
        }
        AttachAction::CompletePrompt => {
            if let Some(prompt) = app.attach_prompt.as_mut() {
                prompt.completion = None;
            }
            complete_prompt(app)
        }
        AttachAction::SubmitPrompt => submit_prompt(app),
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

/// The path prompt `Ctrl-O` opens. `:attach <path>` is accepted too, so the
/// webui's command muscle memory types something that works.
#[derive(Debug, Default, Clone)]
pub struct AttachPrompt {
    pub text: String,
    /// The last completion offered, so repeated `Tab` does not re-append it.
    pub completion: Option<String>,
}

impl AttachPrompt {
    /// The path the prompt refers to, with any `:attach ` prefix and `~` gone.
    #[must_use]
    pub fn path(&self) -> String {
        let raw = self.text.trim();
        let raw = raw.strip_prefix(":attach").map_or(raw, str::trim_start);
        expand_home(raw.trim())
    }

    fn key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        self.completion = None;
        match key.code {
            KeyCode::Char(c) => self.text.push(c),
            KeyCode::Backspace => {
                self.text.pop();
            }
            _ => {}
        }
    }
}

/// `~` and `~/x` resolve against `$HOME`; anything else is returned as given.
#[must_use]
pub fn expand_home(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else { return path.to_owned() };
    let Some(home) = std::env::var_os("HOME") else { return path.to_owned() };
    let home = home.to_string_lossy().into_owned();
    if rest.is_empty() { home } else { format!("{home}{rest}") }
}

fn close_prompt(app: &mut App) {
    if app.attach_prompt.take().is_some() {
        app.router.pop();
    }
}

fn submit_prompt(app: &mut App) -> Vec<Effect> {
    let Some(prompt) = app.attach_prompt.clone() else { return Vec::new() };
    close_prompt(app);
    let path = prompt.path();
    if path.is_empty() {
        return Vec::new();
    }
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    vec![Effect::ReadAttachment { session_id, path }]
}

/// Longest unambiguous completion of the prompt's path, from the directory it
/// names. A directory completion keeps its trailing slash so the next `Tab`
/// descends into it.
fn complete_prompt(app: &mut App) -> Vec<Effect> {
    let Some(prompt) = app.attach_prompt.as_ref() else { return Vec::new() };
    let path = prompt.path();
    let (dir, prefix) =
        path.rfind('/').map_or(("./", path.as_str()), |at| (&path[..=at], &path[at + 1..]));
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut matches: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        matches.push(if is_dir { format!("{name}/") } else { name });
    }
    if matches.is_empty() {
        return Vec::new();
    }
    matches.sort();
    let common = longest_common_prefix(&matches);
    if let Some(prompt) = app.attach_prompt.as_mut() {
        prompt.text = format!("{dir}{common}");
        prompt.completion = (matches.len() > 1).then(|| matches.join("  "));
    }
    Vec::new()
}

fn longest_common_prefix(names: &[String]) -> String {
    let Some(first) = names.first() else { return String::new() };
    let mut end = first.len();
    for name in &names[1..] {
        end = end.min(
            first
                .char_indices()
                .zip(name.chars())
                .take_while(|((_, a), b)| a == b)
                .last()
                .map_or(0, |((i, a), _)| i + a.len_utf8()),
        );
    }
    first[..end].to_owned()
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
    use super::{AttachAction, AttachPrompt, PASTE_THRESHOLD_BYTES, expand_home};
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
    fn the_prompt_accepts_a_bare_path_or_the_attach_command() {
        let mut prompt = AttachPrompt { text: "  /tmp/a.txt ".to_owned(), completion: None };
        assert_eq!(prompt.path(), "/tmp/a.txt");
        prompt.text = ":attach /tmp/a.txt".to_owned();
        assert_eq!(prompt.path(), "/tmp/a.txt");
        prompt.text = ":attach".to_owned();
        assert_eq!(prompt.path(), "");
    }

    #[test]
    fn a_home_relative_path_expands() {
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(expand_home("~/x.txt"), format!("{home}/x.txt"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/abs/x"), "/abs/x");
        assert_eq!(expand_home("rel/x"), "rel/x");
    }

    #[test]
    fn submitting_the_prompt_asks_for_the_file_and_closes_it() {
        let mut app = app();
        attach(&mut app, AttachAction::OpenPrompt);
        assert!(app.attach_prompt.is_some());
        if let Some(prompt) = app.attach_prompt.as_mut() {
            prompt.text = "/tmp/a.txt".to_owned();
        }
        let effects = attach(&mut app, AttachAction::SubmitPrompt);
        match effects.as_slice() {
            [Effect::ReadAttachment { session_id, path }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(path, "/tmp/a.txt");
            }
            _ => panic!("expected a read effect"),
        }
        assert!(app.attach_prompt.is_none());
    }

    #[test]
    fn an_empty_prompt_asks_for_nothing() {
        let mut app = app();
        attach(&mut app, AttachAction::OpenPrompt);
        assert!(attach(&mut app, AttachAction::SubmitPrompt).is_empty());
    }

    #[test]
    fn tab_completes_against_the_real_directory() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("alpha.txt"), b"x").expect("write");
        std::fs::write(dir.path().join("alphabet.txt"), b"x").expect("write");
        std::fs::write(dir.path().join("beta.txt"), b"x").expect("write");

        let mut app = app();
        attach(&mut app, AttachAction::OpenPrompt);
        if let Some(prompt) = app.attach_prompt.as_mut() {
            prompt.text = format!("{}/al", dir.path().display());
        }
        attach(&mut app, AttachAction::CompletePrompt);
        let prompt = app.attach_prompt.as_ref().expect("the prompt");
        assert_eq!(prompt.text, format!("{}/alpha", dir.path().display()));
        assert!(prompt.completion.is_some(), "two candidates are listed");

        if let Some(prompt) = app.attach_prompt.as_mut() {
            prompt.text = format!("{}/b", dir.path().display());
        }
        attach(&mut app, AttachAction::CompletePrompt);
        let prompt = app.attach_prompt.as_ref().expect("the prompt");
        assert_eq!(prompt.text, format!("{}/beta.txt", dir.path().display()));
        assert!(prompt.completion.is_none(), "one candidate needs no list");
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
