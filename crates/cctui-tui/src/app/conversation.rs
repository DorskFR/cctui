use super::action::Effect;
use super::conversation_store::PageKind;
use super::state::{App, ConversationLine, View};

pub enum ConversationAction {
    Loaded {
        session_id: String,
        kind: PageKind,
        rows: Vec<(i64, ConversationLine)>,
        etag: Option<String>,
        has_more: bool,
    },
    NotModified {
        session_id: String,
        kind: PageKind,
    },
    Failed {
        session_id: String,
        kind: PageKind,
    },
}

pub fn reduce(app: &mut App, action: ConversationAction) -> Vec<Effect> {
    match action {
        ConversationAction::Loaded { session_id, kind, rows, etag, has_more } => {
            let merge = app.conversation_mut(&session_id).merge_page(kind, rows, etag, has_more);
            if kind == PageKind::Older && merge.inserted > 0 && merge.reordered {
                app.pending_prepend = true;
            }
            Vec::new()
        }
        ConversationAction::NotModified { session_id, kind } => {
            app.conversation_mut(&session_id).page_not_modified(kind);
            Vec::new()
        }
        ConversationAction::Failed { session_id, kind } => {
            app.conversation_mut(&session_id).page_failed(kind);
            Vec::new()
        }
    }
}

/// Opening always refetches the newest page, whatever is already buffered: the
/// stored `ETag` makes the repeat a 304, and any cheaper gate loses the history
/// to a live event that happens to arrive first.
pub fn open(app: &mut App, session_id: String) -> Vec<Effect> {
    app.follow_tail = true;
    app.scroll_offset = 0;
    app.router.push(View::Conversation);

    let store = app.conversation_mut(&session_id);
    let page = store.latest_request();
    let etag = store.etag().map(str::to_owned);
    vec![
        Effect::LoadConversationPage {
            session_id: session_id.clone(),
            kind: PageKind::Latest,
            page,
            etag,
        },
        Effect::Subscribe { session_id },
    ]
}

pub fn load_older(app: &mut App) -> Vec<Effect> {
    if app.view() != View::Conversation || app.scroll_offset != 0 || app.follow_tail {
        return Vec::new();
    }
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let Some(page) = app.conversation_mut(&session_id).begin_older() else { return Vec::new() };
    vec![Effect::LoadConversationPage { session_id, kind: PageKind::Older, page, etag: None }]
}

pub fn stream(app: &mut App, session_id: &str, seq: Option<i64>, line: ConversationLine) {
    app.conversation_mut(session_id).push_live(seq, line);
}

#[cfg(test)]
mod tests {
    use super::{ConversationAction, PageKind, load_older, open};
    use crate::app::action::Effect;
    use crate::app::state::{App, ConversationLine, LineKind};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.show_all_sessions = true;
        app.update_aggregates();
        app
    }

    fn line(text: &str) -> ConversationLine {
        ConversationLine {
            timestamp: 0,
            kind: LineKind::Assistant,
            text: text.to_owned(),
            tool_input: None,
        }
    }

    fn page(app: &mut App, kind: PageKind, rows: &[(i64, &str)], has_more: bool) {
        let rows = rows.iter().map(|(seq, text)| (*seq, line(text))).collect();
        reduce(
            app,
            Action::Conversation(ConversationAction::Loaded {
                session_id: "s-a".to_owned(),
                kind,
                rows,
                etag: Some("etag-1".to_owned()),
                has_more,
            }),
        );
    }

    #[test]
    fn opening_fetches_the_latest_page_and_subscribes() {
        let mut app = app();
        let effects = open(&mut app, "s-a".to_owned());
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::LoadConversationPage { kind: PageKind::Latest, etag: None, .. },
                Effect::Subscribe { .. },
            ]
        ));
    }

    #[test]
    fn a_live_event_before_the_open_does_not_suppress_the_history_fetch() {
        let mut app = app();
        reduce(
            &mut app,
            Action::StreamLine {
                session_id: "s-a".to_owned(),
                seq: Some(9),
                line: line("live"),
                usage: None,
            },
        );
        let effects = open(&mut app, "s-a".to_owned());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::LoadConversationPage { kind: PageKind::Latest, .. })),
            "the history page must still be requested"
        );
    }

    #[test]
    fn reopening_replays_the_stored_etag() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "a")], false);
        reduce(&mut app, Action::LeaveConversation);

        let effects = open(&mut app, "s-a".to_owned());
        match effects.first() {
            Some(Effect::LoadConversationPage { etag, .. }) => {
                assert_eq!(etag.as_deref(), Some("etag-1"));
            }
            _ => panic!("expected a page load"),
        }
    }




    #[test]
    fn scrolling_to_the_top_loads_one_older_page_at_a_time() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a"), (11, "b")], true);

        app.total_display_lines = 200;
        app.viewport_height = 20;
        let effects = reduce(&mut app, Action::ScrollToTop);
        match effects.as_slice() {
            [Effect::LoadConversationPage { kind: PageKind::Older, page, .. }] => {
                assert_eq!(page.before, Some(10));
            }
            _ => panic!("expected an older page fetch"),
        }

        assert!(load_older(&mut app).is_empty(), "the in-flight fetch is not duplicated");

        page(&mut app, PageKind::Older, &[(8, "older")], false);
        assert!(app.pending_prepend, "the viewport must be re-anchored after a prepend");
        assert!(load_older(&mut app).is_empty(), "the start of the transcript is reached");
    }

    #[test]
    fn following_the_tail_never_pages_older() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a")], true);
        app.follow_tail = true;
        app.scroll_offset = 0;
        assert!(load_older(&mut app).is_empty());
    }

    #[test]
    fn a_failed_older_page_leaves_the_fetch_retryable() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a")], true);
        app.follow_tail = false;
        assert!(!load_older(&mut app).is_empty());

        reduce(
            &mut app,
            Action::Conversation(ConversationAction::Failed {
                session_id: "s-a".to_owned(),
                kind: PageKind::Older,
            }),
        );
        assert!(!load_older(&mut app).is_empty(), "the failed page can be asked for again");
    }

    #[test]
    fn a_not_modified_latest_page_keeps_the_transcript() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "a"), (2, "b")], false);
        reduce(
            &mut app,
            Action::Conversation(ConversationAction::NotModified {
                session_id: "s-a".to_owned(),
                kind: PageKind::Latest,
            }),
        );
        assert_eq!(
            app.conversation("s-a").map(crate::app::conversation_store::ConversationStore::len),
            Some(2)
        );
    }
}
