//! Handing the terminal to `$EDITOR` and taking it back.
//!
//! The TUI owns the alternate screen, raw mode and stdin, so an editor can only
//! run once all three are given up and the input thread has parked. The pure
//! parts — which editor to run, and the temp-file round trip — are separated
//! from the terminal handoff so they can be tested.

use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Who asked for the editor, and so where the text goes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTarget {
    Composer,
    SpawnPrompt,
}

/// A pending handoff: the text to open, and where to put what comes back.
#[derive(Debug, Clone)]
pub struct EditorRequest {
    pub target: EditorTarget,
    pub text: String,
}

/// Lets the main loop park the input thread, so the editor has stdin to itself.
///
/// `park` is what the loop asks for; `parked` is the thread confirming it has
/// stopped reading. Both are needed: a thread that only sees the request could
/// still be inside a read when the editor starts.
#[derive(Debug, Clone, Default)]
pub struct InputGate {
    park: Arc<AtomicBool>,
    parked: Arc<AtomicBool>,
}

impl InputGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the thread to stop reading, and waits until it says it has.
    pub fn park(&self) {
        self.park.store(true, Ordering::SeqCst);
        for _ in 0..200 {
            if self.parked.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn unpark(&self) {
        self.park.store(false, Ordering::SeqCst);
    }

    /// Called by the input thread each time round: `true` means read nothing.
    #[must_use]
    pub fn should_park(&self) -> bool {
        let park = self.park.load(Ordering::SeqCst);
        self.parked.store(park, Ordering::SeqCst);
        park
    }
}

/// `$VISUAL`, then `$EDITOR`, then `vi`.
///
/// The value is a command line, not a path: `EDITOR="code -w"` is common, so
/// the first word is the program and the rest are arguments.
#[must_use]
pub fn editor_command(visual: Option<&str>, editor: Option<&str>) -> Vec<String> {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|v| !v.is_empty())
        .unwrap_or("vi");
    chosen.split_whitespace().map(str::to_owned).collect()
}

#[must_use]
pub fn editor_from_env() -> Vec<String> {
    editor_command(std::env::var("VISUAL").ok().as_deref(), std::env::var("EDITOR").ok().as_deref())
}

/// Writes `text` to a temp file, hands the path to `run`, and reads it back.
///
/// The trailing newline an editor adds is dropped: it is the editor's, not the
/// operator's.
pub fn edit_via_file<F>(text: &str, run: F) -> io::Result<String>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cctui-prompt.md");
    std::fs::write(&path, text)?;
    run(&path)?;
    let edited = std::fs::read_to_string(&path)?;
    Ok(edited.strip_suffix('\n').unwrap_or(&edited).to_owned())
}

/// Runs the editor against `path`, inheriting the terminal.
pub fn run_editor(argv: &[String], path: &Path) -> io::Result<()> {
    let Some((program, rest)) = argv.split_first() else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no editor to run"));
    };
    let status = Command::new(program).args(rest).arg(path).status()?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!("{program} exited with {status}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_wins_then_editor_then_vi() {
        assert_eq!(editor_command(Some("hx"), Some("nano")), ["hx"]);
        assert_eq!(editor_command(None, Some("nano")), ["nano"]);
        assert_eq!(editor_command(None, None), ["vi"]);
    }

    #[test]
    fn a_blank_setting_is_no_setting() {
        assert_eq!(editor_command(Some("  "), Some("nano")), ["nano"]);
        assert_eq!(editor_command(Some(""), Some("")), ["vi"]);
    }

    #[test]
    fn an_editor_with_arguments_keeps_them() {
        assert_eq!(editor_command(Some("code -w"), None), ["code", "-w"]);
    }

    #[test]
    fn the_file_round_trips_what_the_editor_left() {
        let out = edit_via_file("before", |path| {
            assert_eq!(std::fs::read_to_string(path)?, "before");
            std::fs::write(path, "after\nmore\n")
        })
        .expect("the round trip");
        assert_eq!(out, "after\nmore", "the editor's trailing newline is dropped");
    }

    #[test]
    fn an_untouched_file_comes_back_unchanged() {
        assert_eq!(edit_via_file("kept", |_| Ok(())).expect("the round trip"), "kept");
    }

    #[test]
    fn an_editor_that_fails_leaves_the_text_alone() {
        let err = edit_via_file("kept", |_| Err(io::Error::other("boom")));
        assert!(err.is_err());
    }

    #[test]
    fn a_real_editor_process_edits_the_file_it_is_handed() {
        let argv = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf 'edited by the editor\n' > \"$1\"".to_owned(),
            "sh".to_owned(),
        ];
        let out = edit_via_file("before", |path| run_editor(&argv, path)).expect("the editor ran");
        assert_eq!(out, "edited by the editor");
    }

    #[test]
    fn a_missing_editor_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let argv = vec!["cctui-no-such-editor".to_owned()];
        assert!(run_editor(&argv, &dir.path().join("x")).is_err());
        assert!(run_editor(&[], &dir.path().join("x")).is_err());
    }

    #[test]
    fn the_gate_parks_and_releases() {
        let gate = InputGate::new();
        assert!(!gate.should_park(), "nothing is asking yet");

        let thread_gate = gate.clone();
        let handle = std::thread::spawn(move || {
            while !thread_gate.should_park() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            while thread_gate.should_park() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        gate.park();
        assert!(gate.parked.load(Ordering::SeqCst), "the thread confirmed before the editor runs");
        gate.unpark();
        handle.join().expect("the thread leaves once released");
    }
}
