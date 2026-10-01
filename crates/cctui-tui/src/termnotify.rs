//! Terminal attention signals: the bell, a desktop notification and the
//! window title, as escape sequences.
//!
//! The sequences are built here and written from one place in the render loop
//! — straight after a draw, where nothing else is mid-frame. Writing them from
//! anywhere else interleaves with ratatui's own output and corrupts the screen.

use std::io::{self, Write};

/// What the terminal is asked to do when a session starts waiting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Touch nothing: no bell, no notification, no title.
    Off,
    #[default]
    Bell,
    /// The bell plus a desktop notification, for a terminal that forwards one.
    Osc,
}

impl Mode {
    /// Accepts the words the preference takes, and the booleans it used to.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "off" | "false" | "none" => Some(Self::Off),
            "bell" | "true" | "on" => Some(Self::Bell),
            "osc" | "notify" | "desktop" => Some(Self::Osc),
            _ => None,
        }
    }
}

/// `BEL`. Every terminal does something with it, even if only a visual flash.
#[must_use]
pub const fn bel() -> &'static str {
    "\x07"
}

/// `OSC 9` — a one-string notification, as iTerm2, `WezTerm` and kitty take it.
#[must_use]
pub fn osc9(body: &str) -> String {
    format!("\x1b]9;{}\x07", sanitise(body))
}

/// `OSC 777` — the `notify` flavour urxvt and others take, with a title.
#[must_use]
pub fn osc777(title: &str, body: &str) -> String {
    format!("\x1b]777;notify;{};{}\x07", sanitise(title), sanitise(body))
}

/// `OSC 2` — the window title, which tmux also shows as the window name.
#[must_use]
pub fn osc2(title: &str) -> String {
    format!("\x1b]2;{}\x07", sanitise(title))
}

/// An OSC payload is terminated by `BEL` or `ST`, so a caller's text must not
/// carry either, nor anything else that would end the frame early and leave
/// the rest to be printed over the screen.
fn sanitise(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(TEXT_CAP).collect()
}

/// Long enough for a session name and a count, short enough that a pathological
/// one cannot flood the terminal.
const TEXT_CAP: usize = 160;

/// The title for `count` sessions waiting, matching the web tab title.
#[must_use]
pub fn title_for(count: usize) -> String {
    if count == 0 { "cctui".to_owned() } else { format!("({count}) cctui") }
}

/// What one newly-waiting session is announced as.
#[must_use]
pub fn alert_sequences(mode: Mode, body: &str) -> String {
    match mode {
        Mode::Off => String::new(),
        Mode::Bell => bel().to_owned(),
        // Both flavours go out: a terminal understands at most one of them and
        // silently drops the other, and which one it is cannot be probed.
        Mode::Osc => format!("{}{}{}", bel(), osc9(body), osc777("cctui", body)),
    }
}

/// Writes `sequences` to the terminal. Called from the render loop only.
pub fn emit(sequences: &str) -> io::Result<()> {
    if sequences.is_empty() {
        return Ok(());
    }
    let mut out = io::stdout().lock();
    out.write_all(sequences.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::{Mode, alert_sequences, osc2, osc9, osc777, title_for};

    #[test]
    fn the_title_counts_the_waiting_sessions_and_resets_at_zero() {
        assert_eq!(title_for(0), "cctui");
        assert_eq!(title_for(1), "(1) cctui");
        assert_eq!(title_for(12), "(12) cctui");
    }

    #[test]
    fn each_sequence_is_introduced_and_terminated() {
        assert_eq!(osc2("(2) cctui"), "\x1b]2;(2) cctui\x07");
        assert_eq!(osc9("alpha needs input"), "\x1b]9;alpha needs input\x07");
        assert_eq!(osc777("cctui", "alpha"), "\x1b]777;notify;cctui;alpha\x07");
    }

    /// A session name is server data: it must not be able to end the frame and
    /// have the rest printed over the screen.
    #[test]
    fn a_payload_cannot_terminate_its_own_frame() {
        let nasty = "alpha\x07\x1b]2;pwned\x07\nand more";
        for seq in [osc9(nasty), osc2(nasty), osc777(nasty, nasty)] {
            assert_eq!(seq.matches('\x07').count(), 1, "one terminator: {seq:?}");
            assert_eq!(seq.matches('\x1b').count(), 1, "one introducer: {seq:?}");
            assert!(!seq.contains('\n'), "nothing that breaks the line: {seq:?}");
        }
    }

    #[test]
    fn a_runaway_payload_is_capped() {
        let huge = "x".repeat(10_000);
        assert!(osc9(&huge).len() < 400);
    }

    #[test]
    fn off_writes_nothing_at_all() {
        assert_eq!(alert_sequences(Mode::Off, "alpha"), "");
    }

    #[test]
    fn bell_is_only_the_bell_and_osc_carries_both_flavours() {
        assert_eq!(alert_sequences(Mode::Bell, "alpha"), "\x07");
        let osc = alert_sequences(Mode::Osc, "alpha");
        assert!(osc.starts_with('\x07'));
        assert!(osc.contains("\x1b]9;alpha\x07"));
        assert!(osc.contains("\x1b]777;notify;cctui;alpha\x07"));
    }

    #[test]
    fn the_mode_takes_its_words_and_the_booleans_it_replaced() {
        assert_eq!(Mode::parse("off"), Some(Mode::Off));
        assert_eq!(Mode::parse("false"), Some(Mode::Off));
        assert_eq!(Mode::parse("bell"), Some(Mode::Bell));
        assert_eq!(Mode::parse(" true "), Some(Mode::Bell));
        assert_eq!(Mode::parse("osc"), Some(Mode::Osc));
        assert_eq!(Mode::parse("sparkles"), None);
        assert_eq!(Mode::default(), Mode::Bell);
    }
}
