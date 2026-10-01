//! `cctui open <id>` and the startup filter: what the command line asks the
//! TUI to be showing by the time the first frame is drawn.

use cctui_proto::api::SessionListItem;

use super::action::Effect;
use super::conversation;
use super::state::App;
use super::toast::Level;

/// What the command line asked for. Empty is the plain `cctui` launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Startup {
    /// A session id to land on, from `cctui open <id>`.
    pub open: Option<String>,
    /// Transcript position to land on, from `--seq`.
    pub seq: Option<i64>,
    /// Search to prefill, from `--filter`.
    pub filter: Option<String>,
}

pub enum DeepLinkAction {
    /// The session the list did not have, fetched by id.
    Fetched {
        session: Box<SessionListItem>,
        seq: Option<i64>,
    },
    Failed {
        session_id: String,
        error: String,
    },
}

/// Runs the command line's intent against the list already fetched.
///
/// A session the list does not carry — an archived one, or one on a machine
/// that is offline — is fetched by id rather than treated as missing.
pub fn apply(app: &mut App, startup: Startup) -> Vec<Effect> {
    let Startup { open, seq, filter } = startup;
    if let Some(filter) = filter {
        super::list_search::start_with(app, filter);
    }
    let Some(session_id) = open else { return Vec::new() };
    if app.sessions.iter().any(|s| s.id == session_id) {
        return land_on(app, &session_id, seq);
    }
    vec![Effect::FetchSession { session_id, seq }]
}

pub fn reduce_deeplink(app: &mut App, action: DeepLinkAction) -> Vec<Effect> {
    match action {
        DeepLinkAction::Fetched { session, seq } => {
            let session_id = session.id.clone();
            if !app.sessions.iter().any(|s| s.id == session_id) {
                app.sessions.push(*session);
                app.update_aggregates();
            }
            land_on(app, &session_id, seq)
        }
        DeepLinkAction::Failed { session_id, error } => {
            let short = session_id.get(..8).unwrap_or(&session_id);
            app.toast(Level::Error, format!("cannot open {short}: {error}"));
            Vec::new()
        }
    }
}

/// Select the session, open its conversation, and jump to `seq` when one was
/// asked for. Selection comes first: the conversation view reads the selected
/// row, so opening without it would land on whatever was selected before.
fn land_on(app: &mut App, session_id: &str, seq: Option<i64>) -> Vec<Effect> {
    if !app.select_session_id(session_id) {
        app.toast(Level::Warn, format!("{session_id} is not in the list"));
        return Vec::new();
    }
    let mut effects = conversation::open(app, session_id.to_owned());
    if let Some(seq) = seq {
        effects.extend(super::pins::jump_to_seq(app, seq));
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::{DeepLinkAction, Startup, apply};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn archived() -> cctui_proto::api::SessionListItem {
        let mut s = session("s-old", "archive", "inactive", "done");
        s.end_reason = Some(cctui_proto::models::SessionEndReason::Completed);
        s
    }

    #[test]
    fn a_plain_launch_asks_for_nothing() {
        let mut app = app();
        assert!(apply(&mut app, Startup::default()).is_empty());
        assert_eq!(app.view(), View::SessionList);
        assert!(!app.list_search.is_active());
    }

    #[test]
    fn opening_a_session_already_in_the_list_needs_no_fetch() {
        let mut app = app();
        let effects =
            apply(&mut app, Startup { open: Some("s-b".to_owned()), ..Startup::default() });
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Subscribe { .. })),
            "it opens straight away"
        );
        assert!(!effects.iter().any(|e| matches!(e, Effect::FetchSession { .. })));
        assert_eq!(app.view(), View::Conversation);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-b"));
    }

    #[test]
    fn a_session_missing_from_the_list_is_fetched_by_id() {
        let mut app = app();
        let effects =
            apply(&mut app, Startup { open: Some("s-old".to_owned()), ..Startup::default() });
        assert!(
            matches!(effects.as_slice(), [Effect::FetchSession { session_id, seq: None }] if session_id == "s-old")
        );
        assert_eq!(app.view(), View::SessionList, "nothing opens until the reply lands");
    }

    #[test]
    fn the_fetched_archived_session_joins_the_list_and_opens() {
        let mut app = app();
        apply(&mut app, Startup { open: Some("s-old".to_owned()), ..Startup::default() });
        reduce(
            &mut app,
            Action::DeepLink(DeepLinkAction::Fetched { session: Box::new(archived()), seq: None }),
        );
        assert_eq!(app.view(), View::Conversation);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-old"));
        assert_eq!(app.sessions.len(), 3, "the row is there for the list to show too");
    }

    #[test]
    fn a_second_reply_for_a_session_now_in_the_list_does_not_duplicate_the_row() {
        let mut app = app();
        for _ in 0..2 {
            reduce(
                &mut app,
                Action::DeepLink(DeepLinkAction::Fetched {
                    session: Box::new(archived()),
                    seq: None,
                }),
            );
        }
        assert_eq!(app.sessions.iter().filter(|s| s.id == "s-old").count(), 1);
    }

    #[test]
    fn a_seq_rides_along_to_the_fetch_and_then_lands_the_cursor() {
        let mut app = app();
        let effects = apply(
            &mut app,
            Startup { open: Some("s-old".to_owned()), seq: Some(2), ..Startup::default() },
        );
        assert!(matches!(effects.as_slice(), [Effect::FetchSession { seq: Some(2), .. }]));

        app.conversations.insert("s-old".to_owned(), crate::testsupport::conversation_store());
        reduce(
            &mut app,
            Action::DeepLink(DeepLinkAction::Fetched {
                session: Box::new(archived()),
                seq: Some(2),
            }),
        );
        assert_eq!(app.view(), View::Conversation);
        assert!(app.line_cursor.is_some(), "the cursor landed on the asked-for message");
        assert!(!app.follow_tail, "landing on a seq detaches from the tail");
    }

    #[test]
    fn a_seq_the_transcript_does_not_have_says_so_instead_of_hanging() {
        let mut app = app();
        app.conversations.insert("s-old".to_owned(), crate::testsupport::conversation_store());
        reduce(
            &mut app,
            Action::DeepLink(DeepLinkAction::Fetched {
                session: Box::new(archived()),
                seq: Some(9_999),
            }),
        );
        assert_eq!(app.view(), View::Conversation, "the session still opens");
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn a_failed_fetch_says_which_id_and_why() {
        let mut app = app();
        reduce(
            &mut app,
            Action::DeepLink(DeepLinkAction::Failed {
                session_id: "s-nope-1234".to_owned(),
                error: "404 Not Found".to_owned(),
            }),
        );
        assert_eq!(app.view(), View::SessionList);
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn a_startup_filter_starts_the_list_search_on_it() {
        let mut app = app();
        app.clock_ms = 1_000;
        apply(&mut app, Startup { filter: Some("tag:wave-5".to_owned()), ..Startup::default() });
        assert_eq!(app.list_search.query, "tag:wave-5");
        assert!(app.list_search.is_active(), "the list shows results, not just a chip");
        assert!(!app.list_search.open, "the prompt is not left waiting for a keystroke");
        assert_eq!(app.view(), View::SessionList, "a filter alone opens nothing");

        app.clock_ms += crate::app::list_search::DEBOUNCE_MS;
        let effects = crate::app::list_search::on_tick(&mut app);
        assert!(
            matches!(effects.as_slice(), [Effect::SearchSessions { q, offset: 0, .. }] if q == "tag:wave-5"),
            "the next tick sends it"
        );
    }
}
