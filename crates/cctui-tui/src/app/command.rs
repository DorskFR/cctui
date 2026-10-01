//! The `:` command line's verbs, parsed away from the reducer so the grammar is
//! testable on its own.

use std::path::PathBuf;

use super::action::Effect;
use super::export::Format;
use super::state::App;
use super::toast::Level;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Export {
        format: Format,
        path: Option<PathBuf>,
    },
    Attach {
        path: PathBuf,
    },
    /// A whole-session fork: every dial inherited from the parent.
    Fork,
    /// The dispatch tab. Its own verb until the spawn dialog carries it.
    Dispatch,
}

/// `export md|html [path]`, `attach <path>` or `fork`, without the leading
/// colon.
pub fn parse(input: &str) -> Result<Command, String> {
    let mut words = input.split_whitespace();
    let verb = words.next().ok_or_else(|| "type a command".to_owned())?;
    match verb {
        "export" | "e" => {
            let word = words.next().ok_or_else(|| "usage: :export md|html [path]".to_owned())?;
            let format =
                Format::parse(word).ok_or_else(|| format!("`{word}` is not md or html"))?;
            let rest: Vec<&str> = words.collect();
            let path = (!rest.is_empty()).then(|| PathBuf::from(rest.join(" ")));
            Ok(Command::Export { format, path })
        }
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
        "dispatch" => {
            if words.next().is_some() {
                return Err("usage: :dispatch".to_owned());
            }
            Ok(Command::Dispatch)
        }
        other => Err(format!("`{other}` is not a command")),
    }
}

/// `~/Downloads/cctui-<slug>-<date>.<ext>`, the webui's download name.
#[must_use]
pub fn default_path(stem: &str, format: Format, clock_ms: i64) -> PathBuf {
    let date = chrono::DateTime::from_timestamp_millis(clock_ms)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string();
    let name = format!("cctui-{stem}-{date}.{}", format.extension());
    let Some(home) = dirs::home_dir() else { return PathBuf::from(name) };
    home.join("Downloads").join(name)
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
        Command::Dispatch => {
            return super::dispatch::reduce_dispatch(app, super::dispatch::DispatchAction::Open);
        }
        _ => {}
    }
    let Some(session) = app.selected_session().cloned() else { return Vec::new() };
    match command {
        Command::Fork | Command::Dispatch => Vec::new(),
        Command::Attach { path } => {
            let path = super::cmdline::expand_home(&path.to_string_lossy());
            vec![Effect::ReadAttachment { session_id: session.id, path }]
        }
        Command::Export { format, path } => {
            let meta = super::export::Meta::of(&session);
            let path =
                path.unwrap_or_else(|| default_path(&meta.file_stem(), format, app.clock_ms));
            vec![Effect::ExportConversation {
                session_id: session.id,
                meta: Box::new(meta),
                filter: Box::new(app.filter.clone()),
                format,
                path,
            }]
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Command, default_path, parse, run};
    use crate::app::action::Effect;
    use crate::app::export::Format;
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
        assert!(parse("fork now").is_err(), "no arguments to mistype");

        let mut app = app();
        match run(&mut app, "fork").as_slice() {
            [Effect::Fork { session_id }] => assert_eq!(session_id, "s-a"),
            other => panic!("expected one fork effect, got {} effects", other.len()),
        }
        assert!(app.controls.armed.is_none(), "the command line does not arm anything");
    }

    #[test]
    fn export_takes_a_format_and_an_optional_path() {
        assert_eq!(
            parse("export md"),
            Ok(Command::Export { format: Format::Markdown, path: None })
        );
        assert_eq!(
            parse("  export html /tmp/out.html "),
            Ok(Command::Export {
                format: Format::Html,
                path: Some(PathBuf::from("/tmp/out.html")),
            })
        );
        assert_eq!(parse("e md"), Ok(Command::Export { format: Format::Markdown, path: None }));
    }

    #[test]
    fn a_path_with_spaces_survives_the_split() {
        let Ok(Command::Export { path, .. }) = parse("export md /tmp/my notes.md") else {
            panic!("expected an export");
        };
        assert_eq!(path, Some(PathBuf::from("/tmp/my notes.md")));
    }

    #[test]
    fn every_way_of_getting_it_wrong_explains_itself() {
        assert!(parse("").is_err());
        assert!(parse("export").unwrap_err().contains("usage"));
        assert!(parse("export pdf").unwrap_err().contains("not md or html"));
        assert!(parse("frobnicate").unwrap_err().contains("not a command"));
    }

    #[test]
    fn the_default_path_lands_in_downloads_with_the_date() {
        let path = default_path("cctui", Format::Markdown, 1_700_000_000_000);
        let name = path.file_name().expect("a file name").to_string_lossy().to_string();
        assert_eq!(name, "cctui-cctui-2023-11-14.md");
        assert!(path.to_string_lossy().contains("Downloads"));
    }

    #[test]
    fn a_good_command_becomes_one_export_effect() {
        let mut app = app();
        let effects = run(&mut app, "export md /tmp/t.md");
        match effects.as_slice() {
            [Effect::ExportConversation { session_id, format, path, .. }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(*format, Format::Markdown);
                assert_eq!(path, &PathBuf::from("/tmp/t.md"));
            }
            _ => panic!("expected one export effect"),
        }
        assert!(app.toasts.latest().is_none(), "a good command says nothing");
    }

    #[test]
    fn a_bad_command_toasts_instead_of_exporting() {
        let mut app = app();
        assert!(run(&mut app, "export pdf").is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("not md or html"));
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
