//! Terminal clipboard writes over OSC 52.
//!
//! The sequence is what makes a copy work over SSH, where the TUI cannot reach
//! the user's own clipboard daemon.

use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// An OSC 52 write with a `c` (clipboard) selection, `BEL`-terminated: `ST`
/// is the other legal terminator but fewer terminals accept it.
#[must_use]
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
}

/// Writes `text` to the terminal's clipboard. Whether the terminal honours the
/// sequence is not observable from here, so a successful write is all this
/// reports.
pub fn copy(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(osc52(text).as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::osc52;

    #[test]
    fn the_payload_is_base64_between_the_osc_introducer_and_bel() {
        let seq = osc52("s-a");
        assert_eq!(seq, "\x1b]52;c;cy1h\x07");
    }

    #[test]
    fn an_empty_copy_is_still_a_well_formed_sequence() {
        assert_eq!(osc52(""), "\x1b]52;c;\x07");
    }
}
