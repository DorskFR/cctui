//! Reading the worker's TUI composer out of a raw PTY frame.
//!
//! Mid-turn a submit has no transcript signal (a queued message is only written
//! once the turn picks it up) and a repaint proves nothing (a busy claude
//! repaints its spinner several times a second). The composer itself is
//! observable: a fresh attach repaints the full screen, and a landed submit
//! leaves the prompt row empty.

/// What a screen frame says about the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposerState {
    /// The prompt row is empty (or shows only the placeholder hint): our draft
    /// left the composer, i.e. the submit landed.
    Empty,
    /// The prompt row still holds a draft.
    Holding,
    /// No prompt row in the frame — nothing can be concluded.
    Unknown,
}

/// What to do after probing the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubmitStep {
    /// The submit landed; stop.
    Done,
    /// The draft is still there; press Enter again.
    Press,
    /// Out of attempts, or the screen stopped being readable: stop without
    /// pressing, so a landed submit can never be doubled.
    GiveUp,
}

/// Decide the next step of the probe loop. `attempt` is how many Enters have
/// been sent so far (1 after the first), `unknown` how many consecutive
/// unreadable frames we have seen.
pub(super) const fn next_submit_step(
    state: ComposerState,
    attempt: u32,
    unknown: u32,
    max_attempts: u32,
    max_unknown: u32,
) -> SubmitStep {
    match state {
        ComposerState::Empty => SubmitStep::Done,
        ComposerState::Unknown if unknown >= max_unknown => SubmitStep::GiveUp,
        ComposerState::Unknown | ComposerState::Holding => {
            if attempt >= max_attempts {
                SubmitStep::GiveUp
            } else {
                SubmitStep::Press
            }
        }
    }
}

/// Drop ANSI escape sequences so a frame can be matched as text. A repaint may
/// place every row with a cursor-position sequence instead of a newline, so
/// cursor-move and erase-display sequences become newlines — without that the
/// whole screen collapses onto one unsplittable line.
pub(super) fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        if matches!(c, 'H' | 'f' | 'J' | 'A' | 'B' | 'E' | 'F') {
                            out.push('\n');
                        }
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Markers claude substitutes for a large paste or an attached image. They can
/// only be in the composer or in an already-submitted message, so finding one
/// on a continuation row of the prompt box means the draft is still pending.
const ATTACHMENT_MARKERS: [&str; 2] = ["[Pasted text #", "[Image #"];

/// Strip box drawing, trailing padding and the block cursor from a frame row.
fn row_content(line: &str) -> &str {
    line.trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '│' | '┃' | '|' | '█' | '▌' | '\u{0}')
    })
}

/// True for the composer's own row: `> ` or `❯ ` once the box border is gone.
fn prompt_row(content: &str) -> Option<&str> {
    let rest = content.strip_prefix('>').or_else(|| content.strip_prefix('❯'))?;
    if rest.is_empty() || rest.starts_with(' ') { Some(rest.trim()) } else { None }
}

/// Claude's empty-composer placeholder, rendered inside the prompt row.
fn is_placeholder(draft: &str) -> bool {
    draft.starts_with("Try ")
}

/// Read the composer out of one screen frame.
pub(super) fn composer_state(screen: &str) -> ComposerState {
    let normalized = screen.replace('\r', "\n");
    let rows: Vec<&str> = normalized.lines().map(row_content).collect();
    let Some(idx) = rows.iter().rposition(|row| prompt_row(row).is_some()) else {
        return ComposerState::Unknown;
    };
    let draft = prompt_row(rows[idx]).unwrap_or("");
    if !draft.is_empty() && !is_placeholder(draft) {
        return ComposerState::Holding;
    }
    // A draft whose first line is blank puts its content on the continuation
    // rows of the same box; attachment markers there are still our draft.
    if rows[idx + 1..]
        .iter()
        .any(|row| ATTACHMENT_MARKERS.iter().any(|marker| row.contains(marker)))
    {
        return ComposerState::Holding;
    }
    ComposerState::Empty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_and_osc() {
        let raw = b"\x1b[38;5;240mhello\x1b[0m\x1b]0;title\x07 world";
        assert_eq!(strip_ansi(raw), "hello world");
    }

    #[test]
    fn strip_ansi_turns_cursor_placement_into_rows() {
        let raw = b"\x1b[2J\x1b[1;1Hfirst\x1b[2;1Hsecond";
        assert_eq!(
            strip_ansi(raw).lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn a_cursor_placed_repaint_still_finds_the_composer() {
        let raw = b"\x1b[2J\x1b[1;1H\xc2\xb7 Pollinating\xe2\x80\xa6 (2m 53s)\
            \x1b[3;1H\xe2\x94\x82 \xe2\x9d\xaf [Pasted text #7 +3 lines] \xe2\x94\x82";
        assert_eq!(composer_state(&strip_ansi(raw)), ComposerState::Holding);
    }

    #[test]
    fn composer_holding_the_reported_draft() {
        // The frame the owner screenshotted: the paste and image are sitting in
        // the prompt box while the turn spins.
        let screen = "· Pollinating… (2m 53s)\n\
             ╭────────────────────────────────╮\n\
             │ ❯ [Image #6][Pasted text #7 +3 lines]█ │\n\
             ╰────────────────────────────────╯\n";
        assert_eq!(composer_state(screen), ComposerState::Holding);
    }

    #[test]
    fn composer_empty_after_the_submit_queues_the_message() {
        let screen = "· Pollinating… (2m 55s)\n\
             > [Image #6][Pasted text #7 +3 lines]\n\
             ╭────────────────────────────────╮\n\
             │ >                              │\n\
             ╰────────────────────────────────╯\n\
             ? for shortcuts\n";
        assert_eq!(composer_state(screen), ComposerState::Empty);
    }

    #[test]
    fn composer_placeholder_row_is_empty() {
        let screen = "│ > Try \"edit <filepath> to…\" │\n";
        assert_eq!(composer_state(screen), ComposerState::Empty);
    }

    #[test]
    fn composer_holding_a_draft_whose_first_line_is_blank() {
        let screen = "╭──────╮\n│ >              │\n│ [Pasted text #2 +9 lines] │\n╰──────╯\n";
        assert_eq!(composer_state(screen), ComposerState::Holding);
    }

    #[test]
    fn composer_unknown_without_a_prompt_row() {
        assert_eq!(
            composer_state("· Pollinating… (2m 53s)\n  Read(foo.rs)\n"),
            ComposerState::Unknown
        );
    }

    #[test]
    fn a_frame_of_raw_bytes_is_readable_after_stripping() {
        let raw = b"\x1b[2J\x1b[38;5;240m\xe2\x95\xad\xe2\x94\x80\xe2\x95\xae\x1b[0m\n\
            \x1b[7m\xe2\x94\x82\x1b[0m \xe2\x9d\xaf [Image #6]\x1b[0m \xe2\x94\x82\n";
        assert_eq!(composer_state(&strip_ansi(raw)), ComposerState::Holding);
    }

    #[test]
    fn next_step_stops_on_an_empty_composer() {
        assert_eq!(next_submit_step(ComposerState::Empty, 1, 0, 6, 2), SubmitStep::Done);
        // Even out of attempts, Empty is success, never a give-up.
        assert_eq!(next_submit_step(ComposerState::Empty, 9, 9, 6, 2), SubmitStep::Done);
    }

    #[test]
    fn next_step_re_presses_while_the_draft_is_held() {
        assert_eq!(next_submit_step(ComposerState::Holding, 1, 0, 6, 2), SubmitStep::Press);
        assert_eq!(next_submit_step(ComposerState::Holding, 5, 0, 6, 2), SubmitStep::Press);
        assert_eq!(next_submit_step(ComposerState::Holding, 6, 0, 6, 2), SubmitStep::GiveUp);
    }

    #[test]
    fn next_step_gives_up_on_repeatedly_unreadable_frames() {
        assert_eq!(next_submit_step(ComposerState::Unknown, 1, 1, 6, 2), SubmitStep::Press);
        assert_eq!(next_submit_step(ComposerState::Unknown, 1, 2, 6, 2), SubmitStep::GiveUp);
    }
}
