//! The `:` command line's verbs, parsed away from the reducer so the grammar is
//! testable on its own.

use std::path::PathBuf;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Attach {
        path: PathBuf,
    },
    /// A whole-session fork: every dial inherited from the parent.
    Fork,
    /// Resume an ended session.
    Resume,
    /// `None` opens the picker; a named mode is applied outright.
    Harness(Option<super::harness_mode::HarnessMode>),
}

/// `attach <path>`, `fork`, `resume` or `harness [bg|oneshot|sdk]`, without the
/// leading colon.
pub fn parse(input: &str) -> Result<Command, String> {
    let mut words = input.split_whitespace();
    let verb = words.next().ok_or_else(|| "type a command".to_owned())?;
    match verb {
        "attach" | "a" => {
            let rest: Vec<&str> = words.collect();
            if rest.is_empty() {
                return Err("usage: :attach <path>".to_owned());
            }
            Ok(Command::Attach { path: PathBuf::from(rest.join(" ")) })
        }
        // `:fork` takes no options yet; refusing them beats silently dropping them.
        "fork" => {
            if words.next().is_some() {
                return Err("usage: :fork".to_owned());
            }
            Ok(Command::Fork)
        }
        "resume" => {
            if words.next().is_some() {
                return Err("usage: :resume".to_owned());
            }
            Ok(Command::Resume)
        }
        // Session-independent: the harness mode is a user setting, so unlike
        // the others this one works with nothing selected.
        "harness" | "h" => {
            let Some(word) = words.next() else { return Ok(Command::Harness(None)) };
            if words.next().is_some() {
                return Err("usage: :harness [bg|oneshot|sdk]".to_owned());
            }
            let mode = super::harness_mode::MODES
                .into_iter()
                .find(|m| m.as_str() == word)
                .ok_or_else(|| format!("`{word}` is not bg, oneshot or sdk"))?;
            Ok(Command::Harness(Some(mode)))
        }
        other => Err(format!("`{other}` is not a command")),
    }
}

pub fn run(app: &mut App, input: &str) -> Vec<Effect> {
    let command = match parse(input) {
        Ok(command) => command,
        Err(problem) => {
            app.toast(Level::Warn, problem);
            return Vec::new();
        }
    };
    match command {
        Command::Fork => return super::controls::fork_now(app),
        Command::Resume => {
            return super::controls::reduce_controls(app, super::controls::ControlsAction::Resume);
        }
        _ => {}
    }
    if let Command::Harness(mode) = command {
        let action = mode.map_or(super::harness_mode::HarnessModeAction::Open, |m| {
            super::harness_mode::HarnessModeAction::Set(m)
        });
        return super::harness_mode::reduce_harness_mode(app, action);
    }
    let Some(session) = app.selected_session().cloned() else { return Vec::new() };
    match command {
        Command::Fork | Command::Resume | Command::Harness(_) => Vec::new(),
        Command::Attach { path } => {
            let path = super::cmdline::expand_home(&path.to_string_lossy());
            vec![Effect::ReadAttachment { session_id: session.id, path }]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, parse, run};
    use crate::app::action::Effect;
    use crate::app::state::App;
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    /// `:fork` needs no confirmation — typing it is already deliberate — and
    /// fires the same effect `Ctrl-f` does once confirmed.
    #[test]
    fn fork_takes_no_arguments_and_forks_the_selected_session() {
        assert_eq!(parse("fork"), Ok(Command::Fork));
        assert_eq!(parse("harness"), Ok(Command::Harness(None)));
        assert_eq!(
            parse("harness sdk"),
            Ok(Command::Harness(Some(super::super::harness_mode::HarnessMode::Sdk)))
        );
        assert!(parse("h oneshot").is_ok());
        assert!(parse("harness nope").is_err());
        assert!(parse("harness sdk extra").is_err());
        assert!(parse("fork now").is_err(), "no arguments to mistype");

        let mut app = app();
        match run(&mut app, "fork").as_slice() {
            [Effect::Fork { session_id, .. }] => assert_eq!(session_id, "s-a"),
            other => panic!("expected one fork effect, got {} effects", other.len()),
        }
        assert!(app.controls.armed.is_none(), "the command line does not arm anything");
    }

    #[test]
    fn every_way_of_getting_it_wrong_explains_itself() {
        assert!(parse("").is_err());
        assert!(parse("resume now").unwrap_err().contains("usage"));
        assert!(parse("frobnicate").unwrap_err().contains("not a command"));
    }

    #[test]
    fn a_bad_command_toasts_instead_of_running() {
        let mut app = app();
        assert!(run(&mut app, "export md").is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("not a command"));
    }
}

#[cfg(test)]
mod attach_tests {
    use std::path::PathBuf;

    use super::{Command, parse, run};
    use crate::app::action::Effect;
    use crate::app::state::App;
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    #[test]
    fn attach_takes_a_path_under_either_spelling() {
        assert_eq!(
            parse("attach /tmp/a.txt"),
            Ok(Command::Attach { path: PathBuf::from("/tmp/a.txt") })
        );
        assert_eq!(
            parse("a /tmp/a.txt"),
            Ok(Command::Attach { path: PathBuf::from("/tmp/a.txt") })
        );
    }

    #[test]
    fn a_path_with_spaces_survives_as_one_path() {
        assert_eq!(
            parse("attach /tmp/my notes.txt"),
            Ok(Command::Attach { path: PathBuf::from("/tmp/my notes.txt") })
        );
    }

    #[test]
    fn attach_without_a_path_says_how_to_use_it() {
        assert_eq!(parse("attach"), Err("usage: :attach <path>".to_owned()));
    }

    #[test]
    fn running_attach_asks_for_the_file_with_home_expanded() {
        let home = std::env::var("HOME").expect("HOME");
        let mut app = app();
        match run(&mut app, "attach ~/x.txt").as_slice() {
            [Effect::ReadAttachment { session_id, path }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(*path, format!("{home}/x.txt"));
            }
            other => panic!("expected one read effect, got {}", other.len()),
        }
    }
}
