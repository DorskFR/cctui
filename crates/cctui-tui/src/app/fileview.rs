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
    // Markup that a browser would execute is shown as source, never rendered and
    // never handed out: SVG carries script just as HTML does.
    if MARKUP_AS_TEXT.contains(&base.as_str()) {
        return FileKind::Text;
    }
    if base.starts_with("image/") {
        return FileKind::Image;
    }
    match base.as_str() {
        "text/markdown" => FileKind::Markdown,
        "text/plain" | "application/json" => FileKind::Text,
        _ => FileKind::Download,
    }
}

/// Shown as source in the pager instead of being rendered or opened.
const MARKUP_AS_TEXT: [&str; 4] =
    ["text/html", "application/xhtml+xml", "image/svg+xml", "application/xml"];

/// Types a handler would run rather than show. The bytes come from a session,
/// so handing one to the desktop is handing it execution.
const NEVER_EXTERNAL: [&str; 14] = [
    "application/x-sh",
    "application/x-shellscript",
    "text/x-shellscript",
    "application/x-executable",
    "application/x-msdownload",
    "application/x-desktop",
    "application/vnd.microsoft.portable-executable",
    // A browser runs script in these from a file:// origin.
    "text/html",
    "application/xhtml+xml",
    "image/svg+xml",
    "application/java-archive",
    "text/x-python",
    "application/x-python-code",
    "application/x-ms-shortcut",
];

/// Extensions the type alone would not catch. A served `content_type` is the
/// sender's claim; the name is what the handler will dispatch on.
const NEVER_EXTERNAL_SUFFIX: [&str; 21] = [
    ".sh",
    ".bash",
    ".zsh",
    ".desktop",
    ".exe",
    ".msi",
    ".bat",
    ".cmd",
    ".command",
    ".html",
    ".htm",
    ".xhtml",
    ".svg",
    ".jar",
    ".py",
    ".ps1",
    ".appimage",
    ".url",
    ".webloc",
    ".lnk",
    ".scpt",
];

/// Names that mean markup whatever the served type claims.
const MARKUP_SUFFIX: [&str; 4] = [".html", ".htm", ".xhtml", ".svg"];

/// Why this file must not be handed to the desktop, or `None` to allow it.
#[must_use]
pub fn refuse_external_open(name: &str, content_type: &str) -> Option<String> {
    let base = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let lower = name.to_ascii_lowercase();
    if !NEVER_EXTERNAL.contains(&base.as_str())
        && !NEVER_EXTERNAL_SUFFIX.iter().any(|ext| lower.ends_with(ext))
    {
        return None;
    }
    if MARKUP_AS_TEXT.contains(&base.as_str()) || MARKUP_SUFFIX.iter().any(|e| lower.ends_with(e)) {
        return Some(format!("{name} can run script in a browser — showing the source instead"));
    }
    Some(format!("{name} is a program, not a document — refusing to open it"))
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
    /// As served, so `o` can refuse a type the desktop would run.
    pub content_type: String,
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
            let text = if kind == FileKind::Image {
                String::new()
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            if kind == FileKind::Image {
                app.images.prepare(&name, &bytes);
            }
            app.file_view =
                Some(FileView { name, path, kind, content_type, text, bytes, scroll: 0 });
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
            if let Some(why) = refuse_external_open(&view.name, &view.content_type) {
                app.toast(Level::Warn, why);
                return Vec::new();
            }
            vec![Effect::OpenInOsViewer { name: view.name.clone(), bytes: view.bytes.clone() }]
        }
        FileViewAction::Close => {
            app.images.forget();
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
        refusal_message, refuse_external_open,
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

    /// Markup a browser would execute is shown as source. SVG is `image/*` but
    /// must not take the image path, or it renders as a broken picture and still
    /// counts as something to hand out.
    #[test]
    fn markup_that_can_run_script_is_shown_as_source() {
        assert_eq!(classify("text/html"), FileKind::Text);
        assert_eq!(classify("text/html; charset=utf-8"), FileKind::Text);
        assert_eq!(classify("application/xhtml+xml"), FileKind::Text);
        assert_eq!(classify("image/svg+xml"), FileKind::Text);
    }

    /// The should-fix: `o` handed HTML and SVG to the browser, which runs their
    /// script from a file:// origin.
    #[test]
    fn html_and_svg_are_never_handed_to_the_desktop() {
        for (name, content_type) in [
            ("report.html", "text/html"),
            ("report.htm", "text/html"),
            ("page.xhtml", "application/xhtml+xml"),
            ("chart.svg", "image/svg+xml"),
            // The served type is the sender's claim; the name is what a handler
            // dispatches on, so either one alone is enough to refuse.
            ("report.html", "application/octet-stream"),
            ("chart.svg", "text/plain"),
            ("x.bin", "text/html"),
        ] {
            let why = refuse_external_open(name, content_type)
                .unwrap_or_else(|| panic!("{name} ({content_type}) must be refused"));
            assert!(why.contains("script"), "{name}: {why}");
        }
    }

    #[test]
    fn the_other_runnable_types_are_refused_too() {
        for name in ["app.jar", "run.py", "go.ps1", "tool.appimage", "link.url", "s.lnk"] {
            assert!(
                refuse_external_open(name, "application/octet-stream").is_some(),
                "{name} must be refused"
            );
        }
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

    /// It opens on a placeholder naming the type, and waits for `o`: handing a
    /// session's bytes to the desktop is the user's call.
    #[test]
    fn an_unrenderable_type_waits_for_the_user_to_ask() {
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
        assert!(effects.is_empty(), "nothing is launched on its own");
        assert_eq!(app.file_view.as_ref().expect("the placeholder").kind, FileKind::Download);
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

    fn opened(name: &str, content_type: &str) -> FileViewAction {
        FileViewAction::Opened {
            name: name.to_owned(),
            path: format!("/tmp/{name}"),
            content_type: content_type.to_owned(),
            bytes: b"\x7fELF payload".to_vec(),
        }
    }

    /// F9: an unknown type used to be written to /tmp and handed to xdg-open the
    /// moment it arrived, with no key press in between.
    #[test]
    fn an_unknown_type_is_shown_not_launched() {
        let mut app = App::new();
        let effects = reduce(&mut app, Action::FileView(opened("thing.bin", "application/pdf")));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::OpenInOsViewer { .. })),
            "opening a file must not hand it to the desktop on its own"
        );
        let view = app.file_view.as_ref().expect("the viewer is open on it");
        assert_eq!(view.kind, FileKind::Download);
        assert_eq!(view.content_type, "application/pdf");
        assert_eq!(app.view(), crate::app::View::FileViewer);

        // `o` is the explicit action, and it is what reaches the desktop.
        let effects = reduce(&mut app, Action::FileView(FileViewAction::OpenInOsViewer));
        assert!(effects.iter().any(|e| matches!(e, Effect::OpenInOsViewer { .. })));
    }

    /// F9: handing a session-controlled script to the desktop is handing it
    /// execution, so `o` refuses rather than asking the handler.
    #[test]
    fn a_program_is_never_handed_to_the_desktop() {
        for (name, content_type) in [
            ("install.sh", "text/plain"),
            ("payload.desktop", "application/x-desktop"),
            ("thing.exe", "application/octet-stream"),
            ("x.bin", "application/x-shellscript"),
        ] {
            assert!(refuse_external_open(name, content_type).is_some(), "{name} must be refused");

            let mut app = App::new();
            reduce(&mut app, Action::FileView(opened(name, content_type)));
            let effects = reduce(&mut app, Action::FileView(FileViewAction::OpenInOsViewer));
            assert!(
                !effects.iter().any(|e| matches!(e, Effect::OpenInOsViewer { .. })),
                "{name} reached the desktop anyway"
            );
            assert!(app.toasts.latest().expect("a toast").text.contains("program"));
        }
    }

    #[test]
    fn a_document_is_still_openable_on_request() {
        for (name, content_type) in [("report.pdf", "application/pdf"), ("a.bin", "")] {
            assert_eq!(refuse_external_open(name, content_type), None, "{name} is not a program");
        }
    }
}
