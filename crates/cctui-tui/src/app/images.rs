//! Images in the terminal: the pasted screenshot, the inline preview, and the
//! placeholder that stands in when the terminal cannot draw a picture.
//!
//! Marker scanning and chip wording are `cctui_clientcore::images`, shared with
//! the webui. What lives here is terminal-only: which graphics protocol the
//! terminal answered to, and the encode cache the pager renders from.

use std::cell::RefCell;

use cctui_clientcore::images;
use ratatui::layout::Rect;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;

use super::action::Effect;
use super::attach::AttachAction;
use super::state::App;
use super::toast::Level;

/// How this terminal can show a picture. `Text` is the honest answer for a
/// terminal that answered no graphics query: the placeholder and `o`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Support {
    #[default]
    Text,
    Halfblocks,
    Sixel,
    Kitty,
    Iterm2,
}

impl Support {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Halfblocks => "halfblocks",
            Self::Sixel => "sixel",
            Self::Kitty => "kitty",
            Self::Iterm2 => "iterm2",
        }
    }

    /// Halfblocks is every terminal's unicode fallback, not a graphics
    /// protocol: it is only drawn when the user asked for inline previews.
    #[must_use]
    pub const fn is_graphics(self) -> bool {
        !matches!(self, Self::Text)
    }
}

const fn support_of(protocol: ProtocolType) -> Support {
    match protocol {
        ProtocolType::Halfblocks => Support::Halfblocks,
        ProtocolType::Sixel => Support::Sixel,
        ProtocolType::Kitty => Support::Kitty,
        ProtocolType::Iterm2 => Support::Iterm2,
    }
}

struct Encoded {
    key: (String, u16, u16),
    protocol: Option<Protocol>,
}

/// The terminal's graphics capability plus one encoded picture.
///
/// Only one image is ever on screen — the pager's — so the cache holds one
/// entry, re-encoded when the name or the area changes.
#[derive(Default)]
pub struct Images {
    picker: Option<Picker>,
    pub support: Support,
    /// `None` is "auto": inline when the terminal has a real protocol.
    pub inline_pref: Option<bool>,
    encoded: RefCell<Option<Encoded>>,
}

impl std::fmt::Debug for Images {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Images")
            .field("support", &self.support)
            .field("inline_pref", &self.inline_pref)
            .finish_non_exhaustive()
    }
}

impl Images {
    /// Ask the terminal what it can draw. Writes to and reads from the tty, so
    /// it runs once before the TUI takes the screen — never from a test, which
    /// keeps every snapshot on the text fallback.
    pub fn detect(&mut self) {
        match Picker::from_query_stdio() {
            Ok(picker) => {
                self.support = support_of(picker.protocol_type());
                self.picker = Some(picker);
            }
            Err(e) => {
                tracing::debug!(%e, "no terminal graphics protocol; images stay placeholders");
                self.support = Support::Text;
                self.picker = None;
            }
        }
    }

    /// Whether a picture is drawn rather than described.
    #[must_use]
    pub fn inline(&self) -> bool {
        self.picker.is_some() && self.inline_pref.unwrap_or_else(|| self.support.is_graphics())
    }

    /// The picture encoded for `area`, or `None` when it cannot be drawn there
    /// — an undecodable blob, an area too small, or no protocol at all. The
    /// caller falls back to the placeholder.
    pub fn encode(
        &self,
        name: &str,
        bytes: &[u8],
        area: Rect,
    ) -> Option<std::cell::Ref<'_, Protocol>> {
        if !self.inline() || area.width < 4 || area.height < 2 {
            return None;
        }
        let key = (name.to_owned(), area.width, area.height);
        let stale = self.encoded.borrow().as_ref().is_none_or(|e| e.key != key);
        if stale {
            let picker = self.picker.as_ref()?;
            let protocol = decode(bytes).and_then(|img| {
                picker
                    .new_protocol(
                        img,
                        ratatui::layout::Size::new(area.width, area.height),
                        Resize::Fit(None),
                    )
                    .inspect_err(
                        |e| tracing::debug!(%e, "cannot encode an image for this terminal"),
                    )
                    .ok()
            });
            *self.encoded.borrow_mut() = Some(Encoded { key, protocol });
        }
        let borrowed = self.encoded.borrow();
        std::cell::Ref::filter_map(borrowed, |e| e.as_ref()?.protocol.as_ref()).ok()
    }

    pub fn forget(&self) {
        *self.encoded.borrow_mut() = None;
    }
}

fn decode(bytes: &[u8]) -> Option<image::DynamicImage> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .inspect_err(|e| tracing::debug!(%e, "cannot decode an image"))
        .ok()
}

#[must_use]
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

pub enum ImagesAction {
    /// The paste key: bracketed paste cannot carry a picture, so the clipboard
    /// is read directly.
    PasteClipboard,
    /// A clipboard image, already PNG-encoded by the effect.
    Pasted {
        png: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// The clipboard held no image; the text path stays untouched.
    NoImage,
    /// Draw pictures inline, or describe them.
    ToggleInline,
    /// Open the image the transcript cursor sits on.
    OpenUnderCursor,
    Fetched {
        name: String,
        content_type: String,
        bytes: Vec<u8>,
    },
}

pub fn reduce_images(app: &mut App, action: ImagesAction) -> Vec<Effect> {
    match action {
        ImagesAction::PasteClipboard => vec![Effect::ReadClipboardImage],
        ImagesAction::Pasted { png, width, height } => pasted(app, png, width, height),
        ImagesAction::NoImage => {
            app.toast(Level::Info, "no image on the clipboard");
            Vec::new()
        }
        ImagesAction::ToggleInline => {
            let on = !app.images.inline_pref.unwrap_or_else(|| app.images.support.is_graphics());
            app.images.inline_pref = Some(on);
            app.images.forget();
            let message = if on && app.images.picker.is_none() {
                "this terminal cannot draw images — o opens them in the desktop viewer".to_owned()
            } else if on {
                format!("inline images on ({})", app.images.support.label())
            } else {
                "inline images off".to_owned()
            };
            app.toast(Level::Info, message);
            Vec::new()
        }
        ImagesAction::OpenUnderCursor => open_under_cursor(app),
        ImagesAction::Fetched { name, content_type, bytes } => {
            app.images.forget();
            super::fileview::reduce_fileview(
                app,
                super::fileview::FileViewAction::Opened {
                    name: name.clone(),
                    path: name,
                    content_type,
                    bytes,
                },
            )
        }
    }
}

/// A pasted screenshot is a staged attachment like any other, under the same
/// `clipboard-N.<ext>` name the webui synthesizes.
fn pasted(app: &mut App, png: Vec<u8>, width: u32, height: u32) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let taken: Vec<String> =
        app.attachments.items(&session_id).iter().map(|a| a.name.clone()).collect();
    let name = clipboard_name(&taken, "image/png");
    super::attach::reduce_attach(
        app,
        AttachAction::Read {
            session_id,
            name,
            bytes: png,
            content_type: "image/png".to_owned(),
            dimensions: Some((width, height)),
        },
    )
}

/// `clipboard-N.<ext>`, counting past whatever is already staged.
#[must_use]
pub fn clipboard_name(taken: &[String], content_type: &str) -> String {
    let ext = cctui_clientcore::uploads::ext_for_type(content_type);
    let mut n = 1;
    while taken.iter().any(|t| *t == format!("clipboard-{n}.{ext}")) {
        n += 1;
    }
    format!("clipboard-{n}.{ext}")
}

/// Every marker on the focused line, for the key that opens one.
#[must_use]
pub fn markers_under_cursor(app: &App) -> Vec<String> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let Some(cursor) = app.line_cursor else { return Vec::new() };
    let Some(store) = app.conversations.get(&session_id) else { return Vec::new() };
    store.entries().get(cursor).map(|e| e.line.image_ids.clone()).unwrap_or_default()
}

fn open_under_cursor(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let ids = markers_under_cursor(app);
    let Some(image_id) = ids.into_iter().next() else {
        app.toast(Level::Info, "no image on this line");
        return Vec::new();
    };
    vec![Effect::FetchSessionImage { session_id, image_id }]
}

/// What a transcript image line reads as, and what the pager shows when the
/// picture cannot be drawn.
#[must_use]
pub fn line_placeholder(line: &super::state::ConversationLine) -> String {
    if line.text.trim().is_empty() {
        images::inline_placeholder("", None, None)
    } else {
        line.text.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{Support, clipboard_name, markers_under_cursor};
    use crate::app::action::Effect;
    use crate::app::images::ImagesAction;
    use crate::app::{Action, App, ConversationLine, LineKind, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app.router.push(crate::app::View::Conversation);
        let _ = crate::app::drafts::sync_composer(&mut app);
        app
    }

    #[test]
    fn a_fresh_app_describes_images_rather_than_drawing_them() {
        let app = App::new();
        assert_eq!(app.images.support, Support::Text);
        assert!(!app.images.inline(), "no protocol was ever queried");
        assert!(!Support::Text.is_graphics());
        assert!(Support::Kitty.is_graphics());
    }

    #[test]
    fn the_paste_key_reads_the_clipboard_rather_than_the_terminal() {
        let mut app = app();
        let effects = reduce(&mut app, Action::Images(ImagesAction::PasteClipboard));
        assert!(matches!(effects.as_slice(), [Effect::ReadClipboardImage]));
    }

    #[test]
    fn a_pasted_screenshot_is_staged_as_a_named_image_with_its_size() {
        let mut app = app();
        reduce(
            &mut app,
            Action::Images(ImagesAction::Pasted { png: vec![0; 2048], width: 1280, height: 720 }),
        );
        let staged = app.attachments.items("s-a");
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].name, "clipboard-1.png");
        assert_eq!(staged[0].chip_label(), "image: clipboard-1.png 1280x720 2 KB");
        assert_eq!(app.message_input.lines().join("\n"), "[clipboard-1.png]");

        reduce(
            &mut app,
            Action::Images(ImagesAction::Pasted { png: vec![0; 10], width: 2, height: 2 }),
        );
        assert_eq!(app.attachments.items("s-a")[1].name, "clipboard-2.png");
    }

    #[test]
    fn an_empty_clipboard_says_so_and_stages_nothing() {
        let mut app = app();
        reduce(&mut app, Action::Images(ImagesAction::NoImage));
        assert!(app.attachments.is_empty("s-a"));
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn clipboard_names_follow_the_mime_type_and_skip_what_is_taken() {
        assert_eq!(clipboard_name(&[], "image/png"), "clipboard-1.png");
        assert_eq!(clipboard_name(&[], "image/jpeg"), "clipboard-1.jpg");
        assert_eq!(clipboard_name(&["clipboard-1.png".to_owned()], "image/png"), "clipboard-2.png");
    }

    #[test]
    fn toggling_inline_previews_is_sticky_and_says_what_it_did() {
        let mut app = app();
        reduce(&mut app, Action::Images(ImagesAction::ToggleInline));
        assert_eq!(app.images.inline_pref, Some(true));
        assert!(
            app.toasts.latest().is_some_and(|t| t.text.contains("cannot draw")),
            "a terminal with no protocol must say so rather than pretend"
        );
        reduce(&mut app, Action::Images(ImagesAction::ToggleInline));
        assert_eq!(app.images.inline_pref, Some(false));
    }

    #[test]
    fn the_image_key_fetches_the_marker_under_the_cursor() {
        let mut app = app();
        let mut line = ConversationLine::new(LineKind::Image, "[image: shot.png]", 0);
        line.image_ids = vec!["img-7".to_owned()];
        app.conversation_mut("s-a").push_live(Some(1), line);
        app.line_cursor = Some(0);
        assert_eq!(markers_under_cursor(&app), ["img-7"]);

        let effects = reduce(&mut app, Action::Images(ImagesAction::OpenUnderCursor));
        match effects.as_slice() {
            [Effect::FetchSessionImage { session_id, image_id }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(image_id, "img-7");
            }
            other => panic!("expected one image fetch, got {}", other.len()),
        }
    }

    #[test]
    fn a_line_with_no_marker_says_there_is_nothing_to_open() {
        let mut app = app();
        app.conversation_mut("s-a")
            .push_live(Some(1), ConversationLine::new(LineKind::Assistant, "prose", 0));
        app.line_cursor = Some(0);
        assert!(reduce(&mut app, Action::Images(ImagesAction::OpenUnderCursor)).is_empty());
        assert!(app.toasts.latest().is_some());
    }
}
