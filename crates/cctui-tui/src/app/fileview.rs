//! The overlay pager for agent-linked local files.
//!
//! Path detection is `cctui_proto::paths`, the one implementation the server's
//! transcript indexer and the webui regex are also held to. Refusal wording is
//! the webui's `refusalMessage` verbatim, so the same denial reads the same in
//! both clients.

use cctui_client::FileRefusal;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// What the viewer does with a response of this content type; the webui's
/// `classify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Text,
    Markdown,
    /// Nothing the pager can show; the OS viewer gets it instead.
    Download,
}

#[must_use]
pub fn classify(content_type: &str) -> FileKind {
    let base = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if base.starts_with("image/") {
        return FileKind::Image;
    }
    match base.as_str() {
        "text/markdown" => FileKind::Markdown,
        "text/plain" | "application/json" => FileKind::Text,
        _ => FileKind::Download,
    }
}

/// Which route a read came from. A blob is the server's own store and knows
/// nothing about any machine, so its refusals must never be worded as a
/// machine-side absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSource {
    Machine,
    Blob,
}

/// The roots a denial was checked against: the structured list wins, and the
/// prose parse is the fallback for a server that sends none.
#[must_use]
pub fn denied_roots(detail: &str, allowed_folders: &[String]) -> Vec<String> {
    if !allowed_folders.is_empty() {
        return allowed_folders.to_vec();
    }
    let Some(at) = detail.find("allowed roots:") else { return Vec::new() };
    detail[at + "allowed roots:".len()..]
        .split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .collect()
}

/// User-facing text for a refused read, worded exactly as the webui's
/// `refusalMessage` words it.
#[must_use]
pub fn refusal_message(refusal: &FileRefusal, name: &str, source: FileSource) -> String {
    if refusal.status == 0 {
        return format!("Could not open {name} (network)");
    }
    if source == FileSource::Blob {
        return if refusal.status == 404 {
            format!("Cannot open {name}: the attachment is no longer stored on the server")
        } else {
            format!("Could not open {name} ({})", refusal.status)
        };
    }
    match refusal.status {
        413 => format!("Cannot open {name}: the file is over the 32 MiB limit"),
        403 => {
            let roots = denied_roots(&refusal.detail, &refusal.allowed_folders);
            if roots.is_empty() {
                format!("Cannot open {name}: the path is outside what the machine allows")
            } else {
                format!(
                    "Cannot open {name}: the path is outside the roots the machine allows ({})",
                    roots.join(", ")
                )
            }
        }
        404 => format!("Cannot open {name}: the file is no longer on the machine"),
        503 | 504 => format!("Cannot open {name}: the machine is offline"),
        status => format!("Could not open {name} ({status})"),
    }
}

/// Whether the owning machine might answer where this one refused: the path may
/// simply belong to another machine's session.
#[must_use]
pub const fn may_live_elsewhere(status: u16) -> bool {
    matches!(status, 403 | 404)
}

/// An open file in the pager.
#[derive(Debug, Clone)]
pub struct FileView {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    /// Decoded text for a text or markdown file; empty for an image.
    pub text: String,
    /// Raw bytes, kept so `o` can hand the file to the OS viewer.
    pub bytes: Vec<u8>,
    pub scroll: usize,
}

impl FileView {
    /// The syntax token the highlighter keys off: the file's extension.
    #[must_use]
    pub fn extension(&self) -> &str {
        cctui_proto::paths::extension_of(&self.path).unwrap_or("")
    }
}

pub enum FileViewAction {
    /// `gf`: open the path under the line cursor.
    OpenUnderCursor,
    Opened {
        name: String,
        path: String,
        content_type: String,
        bytes: Vec<u8>,
    },
    Refused {
        name: String,
        refusal: Box<FileRefusal>,
        source: FileSource,
    },
    Scroll(i32),
    /// `o`: hand the open file to the OS viewer.
    OpenInOsViewer,
    Close,
}

pub fn reduce_fileview(app: &mut App, action: FileViewAction) -> Vec<Effect> {
    match action {
        FileViewAction::OpenUnderCursor => open_under_cursor(app),
        FileViewAction::Opened { name, path, content_type, bytes } => {
            let kind = classify(&content_type);
            if kind == FileKind::Download {
                // Nothing the pager can render; the OS viewer is the honest answer.
                return vec![Effect::OpenInOsViewer { name, bytes }];
            }
            let text = if kind == FileKind::Image {
                String::new()
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            app.file_view = Some(FileView { name, path, kind, text, bytes, scroll: 0 });
            app.router.push(super::state::View::FileViewer);
            Vec::new()
        }
        FileViewAction::Refused { name, refusal, source } => {
            app.toast(Level::Error, refusal_message(&refusal, &name, source));
            Vec::new()
        }
        FileViewAction::Scroll(delta) => {
            if let Some(view) = app.file_view.as_mut() {
                view.scroll = view.scroll.saturating_add_signed(delta as isize);
            }
            Vec::new()
        }
        FileViewAction::OpenInOsViewer => {
            let Some(view) = app.file_view.as_ref() else { return Vec::new() };
            vec![Effect::OpenInOsViewer { name: view.name.clone(), bytes: view.bytes.clone() }]
        }
        FileViewAction::Close => {
            app.file_view = None;
            app.router.pop();
            Vec::new()
        }
    }
}

/// The path the line cursor sits on. Only meaningful in line-select mode: there
/// is no cursor otherwise, so there is no path to pick.
fn open_under_cursor(app: &mut App) -> Vec<Effect> {
    let Some(session) = app.selected_session().cloned() else { return Vec::new() };
    let Some(cursor) = app.line_cursor else { return Vec::new() };
    let Some(store) = app.conversations.get(&session.id) else { return Vec::new() };
    let Some(entry) = store.entries().get(cursor) else { return Vec::new() };

    let paths = cctui_proto::paths::scan(&entry.line.text);
    let Some(path) = paths.into_iter().next() else {
        app.toast(Level::Info, "no local file path on this line");
        return Vec::new();
    };
    vec![Effect::OpenLinkedFile {
        session_id: session.id.clone(),
        machine_id: session.machine_id.clone(),
        path,
    }]
}

#[cfg(test)]
mod tests {
    use cctui_client::FileRefusal;

    use super::{
        FileKind, FileSource, FileViewAction, classify, denied_roots, may_live_elsewhere,
        refusal_message,
    };
    use crate::app::action::Effect;
    use crate::app::{Action, App, LineKind, reduce};
    use crate::testsupport::session;

    fn refusal(status: u16, detail: &str, folders: &[&str]) -> FileRefusal {
        FileRefusal {
            status,
            detail: detail.to_owned(),
            allowed_folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn app_with_line(text: &str) -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        let line = crate::app::ConversationLine::new(LineKind::Assistant, text.to_owned(), 0);
        app.conversation_mut("s-a").push_live(Some(1), line);
        app.router.push(crate::app::View::Conversation);
        app
    }

    #[test]
    fn content_types_classify_like_the_web_ui() {
        assert_eq!(classify("image/png"), FileKind::Image);
        assert_eq!(classify("text/markdown; charset=utf-8"), FileKind::Markdown);
        assert_eq!(classify("text/plain"), FileKind::Text);
        assert_eq!(classify("application/json"), FileKind::Text);
        assert_eq!(classify("application/octet-stream"), FileKind::Download);
        assert_eq!(classify(""), FileKind::Download);
    }

    #[test]
    fn every_machine_refusal_is_worded_as_the_web_ui_words_it() {
        let name = "a.rs";
        assert_eq!(
            refusal_message(&refusal(413, "", &[]), name, FileSource::Machine),
            "Cannot open a.rs: the file is over the 32 MiB limit"
        );
        assert_eq!(
            refusal_message(&refusal(403, "", &[]), name, FileSource::Machine),
            "Cannot open a.rs: the path is outside what the machine allows"
        );
        assert_eq!(
            refusal_message(&refusal(403, "", &["/home", "/srv"]), name, FileSource::Machine),
            "Cannot open a.rs: the path is outside the roots the machine allows (/home, /srv)"
        );
        assert_eq!(
            refusal_message(&refusal(404, "", &[]), name, FileSource::Machine),
            "Cannot open a.rs: the file is no longer on the machine"
        );
        for status in [503, 504] {
            assert_eq!(
                refusal_message(&refusal(status, "", &[]), name, FileSource::Machine),
                "Cannot open a.rs: the machine is offline"
            );
        }
        assert_eq!(
            refusal_message(&refusal(500, "", &[]), name, FileSource::Machine),
            "Could not open a.rs (500)"
        );
        assert_eq!(
            refusal_message(&refusal(0, "", &[]), name, FileSource::Machine),
            "Could not open a.rs (network)"
        );
    }

    #[test]
    fn a_blob_refusal_never_blames_the_machine() {
        assert_eq!(
            refusal_message(&refusal(404, "", &[]), "shot.png", FileSource::Blob),
            "Cannot open shot.png: the attachment is no longer stored on the server"
        );
        assert_eq!(
            refusal_message(&refusal(500, "", &[]), "shot.png", FileSource::Blob),
            "Could not open shot.png (500)"
        );
    }

    #[test]
    fn denied_roots_prefer_the_structured_list_then_the_prose() {
        assert_eq!(denied_roots("", &["/a".to_owned()]), ["/a"]);
        assert_eq!(denied_roots("outside allowed roots: /a, /b", &[]), ["/a", "/b"]);
        assert!(denied_roots("no roots named", &[]).is_empty());
    }

    #[test]
    fn only_a_denial_or_an_absence_might_live_elsewhere() {
        assert!(may_live_elsewhere(403));
        assert!(may_live_elsewhere(404));
        assert!(!may_live_elsewhere(413));
        assert!(!may_live_elsewhere(500));
    }

    #[test]
    fn gf_asks_for_the_path_on_the_cursor_line() {
        let mut app = app_with_line("edited /home/dev/a.rs just now");
        app.line_cursor = Some(0);
        let effects = reduce(&mut app, Action::FileView(FileViewAction::OpenUnderCursor));
        match effects.as_slice() {
            [Effect::OpenLinkedFile { session_id, machine_id, path }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(machine_id, "orion");
                assert_eq!(path, "/home/dev/a.rs");
            }
            other => panic!("expected one read effect, got {}", other.len()),
        }
    }

    #[test]
    fn gf_outside_line_select_mode_does_nothing() {
        let mut app = app_with_line("edited /home/dev/a.rs");
        assert!(app.line_cursor.is_none());
        assert!(reduce(&mut app, Action::FileView(FileViewAction::OpenUnderCursor)).is_empty());
    }

    #[test]
    fn a_line_with_no_path_says_so_instead_of_opening_nothing() {
        let mut app = app_with_line("nothing to open here");
        app.line_cursor = Some(0);
        assert!(reduce(&mut app, Action::FileView(FileViewAction::OpenUnderCursor)).is_empty());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn an_opened_text_file_lands_in_the_pager() {
        let mut app = app_with_line("x");
        reduce(
            &mut app,
            Action::FileView(FileViewAction::Opened {
                name: "a.rs".to_owned(),
                path: "/home/dev/a.rs".to_owned(),
                content_type: "text/plain".to_owned(),
                bytes: b"fn main() {}".to_vec(),
            }),
        );
        let view = app.file_view.as_ref().expect("an open file");
        assert_eq!(view.text, "fn main() {}");
        assert_eq!(view.extension(), "rs");
        assert_eq!(app.view(), crate::app::View::FileViewer);

        reduce(&mut app, Action::FileView(FileViewAction::Close));
        assert!(app.file_view.is_none());
        assert_eq!(app.view(), crate::app::View::Conversation);
    }

    #[test]
    fn an_unrenderable_type_goes_straight_to_the_os_viewer() {
        let mut app = app_with_line("x");
        let effects = reduce(
            &mut app,
            Action::FileView(FileViewAction::Opened {
                name: "a.pdf".to_owned(),
                path: "/home/dev/a.pdf".to_owned(),
                content_type: "application/pdf".to_owned(),
                bytes: vec![1, 2, 3],
            }),
        );
        assert!(matches!(effects.as_slice(), [Effect::OpenInOsViewer { .. }]));
        assert!(app.file_view.is_none(), "the pager cannot show it, so it does not open");
    }

    #[test]
    fn the_pager_scrolls_and_never_underflows() {
        let mut app = app_with_line("x");
        reduce(
            &mut app,
            Action::FileView(FileViewAction::Opened {
                name: "a.md".to_owned(),
                path: "/a.md".to_owned(),
                content_type: "text/markdown".to_owned(),
                bytes: b"# hi".to_vec(),
            }),
        );
        reduce(&mut app, Action::FileView(FileViewAction::Scroll(5)));
        assert_eq!(app.file_view.as_ref().expect("open").scroll, 5);
        reduce(&mut app, Action::FileView(FileViewAction::Scroll(-50)));
        assert_eq!(app.file_view.as_ref().expect("open").scroll, 0);
    }

    #[test]
    fn a_refusal_is_a_toast_not_an_open_pager() {
        let mut app = app_with_line("x");
        reduce(
            &mut app,
            Action::FileView(FileViewAction::Refused {
                name: "a.rs".to_owned(),
                refusal: Box::new(refusal(403, "", &["/home"])),
                source: FileSource::Machine,
            }),
        );
        assert!(app.file_view.is_none());
        assert_eq!(
            app.toasts.latest().expect("a toast").text,
            "Cannot open a.rs: the path is outside the roots the machine allows (/home)"
        );
    }
}
