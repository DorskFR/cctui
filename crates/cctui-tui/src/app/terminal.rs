//! The live terminal pane: relayed PTY bytes fed into a VT emulator.
//!
//! Read-only by construction — nothing here sends input back to the agent.

use std::fmt::Write as _;

use base64::Engine as _;

use super::action::Effect;
use super::state::{App, View};
use super::toast::Level;

/// The daemon attaches its viewer at a fixed geometry, so the relayed stream
/// is addressed to a screen of exactly this size whatever the pane can show.
pub const REMOTE_COLS: u16 = 120;
pub const REMOTE_ROWS: u16 = 40;

/// Rows of history the emulator keeps behind the live screen.
const SCROLLBACK: usize = 1_000;

/// One watched session's emulated screen.
pub struct TerminalPane {
    pub session_id: String,
    parser: vt100::Parser,
    /// Chunks fed in: the header shows it so a silent pane is distinguishable
    /// from a dead relay.
    pub chunks: u64,
    /// Chunks that were not valid base64, counted rather than swallowed.
    pub undecodable: u64,
    /// Rows scrolled back from the live screen; 0 follows it.
    pub scrollback: usize,
    /// Cell size of the last render, for the header.
    pub view: (u16, u16),
}

impl TerminalPane {
    #[must_use]
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            parser: vt100::Parser::new(REMOTE_ROWS, REMOTE_COLS, SCROLLBACK),
            chunks: 0,
            undecodable: 0,
            scrollback: 0,
            view: (0, 0),
        }
    }

    #[must_use]
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Decodes one `pty_chunk` payload into the emulator. False means the
    /// payload was not base64 and nothing was fed.
    pub fn feed(&mut self, data: &str) -> bool {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            self.undecodable += 1;
            return false;
        };
        self.parser.process(&bytes);
        self.chunks += 1;
        true
    }

    /// Drops the emulated screen. A fresh socket makes the daemon repaint from
    /// scratch, and the held screen is from a stream that no longer applies.
    pub fn reset(&mut self) {
        self.parser = vt100::Parser::new(REMOTE_ROWS, REMOTE_COLS, SCROLLBACK);
        self.scrollback = 0;
    }

    pub fn scroll_by(&mut self, lines: i32) {
        let next = if lines < 0 {
            self.scrollback.saturating_sub(lines.unsigned_abs() as usize)
        } else {
            self.scrollback.saturating_add(lines as usize).min(SCROLLBACK)
        };
        self.scrollback = next;
        self.parser.screen_mut().set_scrollback(next);
    }

    /// `remote 120x40 · view 100x22 · 12 chunks`, plus what is off-screen.
    #[must_use]
    pub fn header(&self) -> String {
        let (cols, rows) = self.view;
        let mut out = format!(
            "remote {REMOTE_COLS}x{REMOTE_ROWS} · view {cols}x{rows} · {} chunks",
            self.chunks
        );
        if cols < REMOTE_COLS {
            let _ = write!(out, " · {} cols clipped", REMOTE_COLS - cols);
        }
        if self.scrollback > 0 {
            let _ = write!(out, " · {} rows back", self.scrollback);
        }
        if self.undecodable > 0 {
            let _ = write!(out, " · {} undecodable", self.undecodable);
        }
        out
    }
}

pub enum TerminalAction {
    /// `T`: open the pane on the selected session, or close the open one.
    Toggle,
    Close,
    Scroll(i32),
    Chunk {
        session_id: String,
        data: String,
    },
}

pub fn reduce_terminal(app: &mut App, action: TerminalAction) -> Vec<Effect> {
    match action {
        TerminalAction::Toggle => {
            if app.terminal.is_some() {
                return close(app);
            }
            open(app)
        }
        TerminalAction::Close => close(app),
        TerminalAction::Scroll(lines) => {
            if let Some(pane) = app.terminal.as_mut() {
                pane.scroll_by(lines);
            }
            Vec::new()
        }
        TerminalAction::Chunk { session_id, data } => {
            let Some(pane) = app.terminal.as_mut().filter(|p| p.session_id == session_id) else {
                return Vec::new();
            };
            if !pane.feed(&data) {
                tracing::warn!(session_id, "dropping an undecodable pty chunk");
            }
            Vec::new()
        }
    }
}

fn open(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    app.terminal = Some(TerminalPane::new(session_id.clone()));
    app.router.push(View::Terminal);
    app.toast(Level::Info, "watching the terminal — read-only");
    vec![Effect::WatchTerminal { session_id, watch: true }]
}

/// Stops the relay server-side; nothing else does, so this is the one path out.
pub fn close(app: &mut App) -> Vec<Effect> {
    let Some(pane) = app.terminal.take() else { return Vec::new() };
    if app.view() == View::Terminal {
        app.router.pop();
    }
    vec![Effect::WatchTerminal { session_id: pane.session_id, watch: false }]
}

/// A fresh socket replays the watch itself (the client holds the terminal
/// subscription), so only the stale screen has to go.
pub fn reconnect(app: &mut App) {
    if let Some(pane) = app.terminal.as_mut() {
        pane.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::{REMOTE_COLS, REMOTE_ROWS, TerminalAction, TerminalPane, reconnect};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app.router.push(View::Conversation);
        app
    }

    fn encode(text: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    }

    fn chunk(app: &mut App, session_id: &str, text: &str) {
        reduce(
            app,
            Action::Terminal(TerminalAction::Chunk {
                session_id: session_id.to_owned(),
                data: encode(text),
            }),
        );
    }

    fn row(app: &App, row: u16) -> String {
        let screen = app.terminal.as_ref().expect("an open pane").screen();
        screen.rows(0, REMOTE_COLS).nth(row as usize).unwrap_or_default()
    }

    #[test]
    fn toggling_opens_the_pane_and_starts_the_relay() {
        let mut app = app();
        let effects = reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        assert!(matches!(
            effects.as_slice(),
            [Effect::WatchTerminal { session_id, watch: true }] if session_id == "s-a"
        ));
        assert_eq!(app.view(), View::Terminal);
        assert_eq!(app.terminal.as_ref().expect("a pane").session_id, "s-a");
    }

    #[test]
    fn closing_stops_the_relay_and_returns_to_the_conversation() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        let effects = reduce(&mut app, Action::Terminal(TerminalAction::Close));
        assert!(matches!(
            effects.as_slice(),
            [Effect::WatchTerminal { session_id, watch: false }] if session_id == "s-a"
        ));
        assert_eq!(app.view(), View::Conversation);
        assert!(app.terminal.is_none());
        assert!(
            reduce(&mut app, Action::Terminal(TerminalAction::Close)).is_empty(),
            "the watch is stopped exactly once"
        );
    }

    #[test]
    fn toggling_twice_is_open_then_close() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        let effects = reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        assert!(matches!(effects.as_slice(), [Effect::WatchTerminal { watch: false, .. }]));
        assert_eq!(app.view(), View::Conversation);
    }

    #[test]
    fn leaving_the_conversation_stops_the_relay() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        let effects = reduce(&mut app, Action::LeaveConversation);
        assert!(
            effects.iter().any(|e| matches!(e, Effect::WatchTerminal { watch: false, .. })),
            "an open pane must not keep the relay running from the session list"
        );
        assert!(app.terminal.is_none());
    }

    #[test]
    fn chunks_are_decoded_into_the_emulated_screen() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        chunk(&mut app, "s-a", "hello");
        chunk(&mut app, "s-a", " world\r\nsecond line");
        assert_eq!(row(&app, 0), "hello world");
        assert_eq!(row(&app, 1), "second line");
        assert_eq!(app.terminal.as_ref().expect("a pane").chunks, 2);
    }

    #[test]
    fn escape_sequences_reach_the_emulator_rather_than_the_screen() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        chunk(&mut app, "s-a", "\x1b[2J\x1b[3;5Hplaced\x1b[1mbold");
        assert_eq!(row(&app, 2), "    placedbold");
        let screen = app.terminal.as_ref().expect("a pane").screen();
        assert!(screen.cell(2, 10).expect("a cell").bold(), "SGR is applied, not printed");
    }

    #[test]
    fn a_chunk_for_another_session_is_ignored() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        chunk(&mut app, "s-other", "nope");
        assert_eq!(row(&app, 0), "");
        assert_eq!(app.terminal.as_ref().expect("a pane").chunks, 0);
    }

    #[test]
    fn a_chunk_with_no_pane_open_is_dropped() {
        let mut app = app();
        chunk(&mut app, "s-a", "nobody is watching");
        assert!(app.terminal.is_none());
    }

    #[test]
    fn an_undecodable_payload_is_counted_not_fed() {
        let mut pane = TerminalPane::new("s-a".to_owned());
        assert!(!pane.feed("not base64 !!"));
        assert_eq!(pane.undecodable, 1);
        assert_eq!(pane.chunks, 0);
        assert!(pane.header().contains("1 undecodable"));
    }

    #[test]
    fn a_reconnect_drops_the_screen_the_old_stream_painted() {
        let mut app = app();
        reduce(&mut app, Action::Terminal(TerminalAction::Toggle));
        chunk(&mut app, "s-a", "stale");
        reconnect(&mut app);
        assert_eq!(row(&app, 0), "", "the daemon repaints on the new socket");
        assert!(app.terminal.is_some(), "the pane stays open across the drop");
    }

    #[test]
    fn scrolling_back_is_clamped_at_the_live_screen() {
        let mut pane = TerminalPane::new("s-a".to_owned());
        pane.scroll_by(-5);
        assert_eq!(pane.scrollback, 0);
        pane.scroll_by(3);
        assert_eq!(pane.scrollback, 3);
        pane.scroll_by(-1);
        assert_eq!(pane.scrollback, 2);
    }

    #[test]
    fn the_header_names_both_geometries_and_what_is_clipped() {
        let mut pane = TerminalPane::new("s-a".to_owned());
        pane.view = (REMOTE_COLS, REMOTE_ROWS);
        assert_eq!(pane.header(), "remote 120x40 · view 120x40 · 0 chunks");

        pane.view = (100, 22);
        pane.feed(&encode("hi"));
        pane.scroll_by(4);
        assert_eq!(
            pane.header(),
            "remote 120x40 · view 100x22 · 1 chunks · 20 cols clipped · 4 rows back"
        );
    }
}
