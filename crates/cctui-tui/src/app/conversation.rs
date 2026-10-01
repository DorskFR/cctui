use super::action::Effect;
use super::conversation_store::{ConversationStore, PageKind};
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
    ToggleLineCursor,
    MoveCursor {
        delta: i32,
    },
    ToggleExpand,
    ToggleExpandAll,
}

pub fn reduce(app: &mut App, action: ConversationAction) -> Vec<Effect> {
    match action {
        ConversationAction::Loaded { session_id, kind, rows, etag, has_more } => {
            let merge = app.conversation_mut(&session_id).merge_page(kind, rows, etag, has_more);
            if kind == PageKind::Older && merge.inserted > 0 && merge.reordered {
                app.pending_prepend = true;
                // The cursor addresses an entry by index, so a prepend moves it.
                if let Some(cursor) = app.line_cursor.as_mut() {
                    *cursor += merge.inserted;
                }
            }
            anchor_pending_seq(app, &session_id);
            super::pins::after_page(app, &session_id)
        }
        ConversationAction::ToggleLineCursor => {
            toggle_line_cursor(app);
            Vec::new()
        }
        ConversationAction::MoveCursor { delta } => {
            move_cursor(app, delta);
            Vec::new()
        }
        ConversationAction::ToggleExpand => {
            if let Some(cursor) = app.line_cursor
                && let Some(session_id) = app.selected_session_id()
            {
                app.conversation_mut(&session_id).toggle_expanded(cursor);
            }
            Vec::new()
        }
        ConversationAction::ToggleExpandAll => {
            let expand = !app.expand_all;
            app.expand_all = expand;
            if let Some(session_id) = app.selected_session_id() {
                app.conversation_mut(&session_id).set_all_expanded(expand);
            }
            Vec::new()
        }
        ConversationAction::NotModified { session_id, kind } => {
            app.conversation_mut(&session_id).page_not_modified(kind);
            Vec::new()
        }
        ConversationAction::Failed { session_id, kind } => {
            app.conversation_mut(&session_id).page_failed(kind);
            super::pins::page_failed(app);
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
    app.line_cursor = None;
    app.router.push(View::Conversation);
    app.subscribed = Some(session_id.clone());

    let store = app.conversation_mut(&session_id);
    let page = ConversationStore::latest_request();
    let etag = store.etag().map(str::to_owned);
    vec![
        Effect::LoadConversationPage {
            session_id: session_id.clone(),
            kind: PageKind::Latest,
            page,
            etag,
        },
        Effect::Subscribe { session_id: session_id.clone() },
        super::pins::on_open(&session_id),
        Effect::MarkSeen { session_id },
    ]
}

/// Move the whole view to another session: leave whatever is subscribed, put
/// the selection on the target and open it.
pub fn switch_to(app: &mut App, session_id: String) -> Vec<Effect> {
    let Some(index) = app.flattened_sessions().iter().position(|s| s.id == session_id) else {
        return Vec::new();
    };
    app.selected_index = index;
    let mut effects = leave(app);
    effects.extend(open(app, session_id));
    effects
}

pub fn leave(app: &mut App) -> Vec<Effect> {
    app.subscribed
        .take()
        .map(|session_id| vec![Effect::Unsubscribe { session_id }])
        .unwrap_or_default()
}

pub fn reconnect(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.subscribed.clone() else { return Vec::new() };
    let page = app.conversation_mut(&session_id).gap_request();
    vec![
        Effect::Subscribe { session_id: session_id.clone() },
        Effect::LoadConversationPage { session_id, kind: PageKind::Gap, page, etag: None },
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

/// Line-select starts at the newest line and holds the viewport there: the
/// cursor and `follow_tail` would otherwise fight over the scroll offset.
fn toggle_line_cursor(app: &mut App) {
    if app.line_cursor.take().is_some() {
        return;
    }
    if let Some(last) = selectable(app).last().copied() {
        app.line_cursor = Some(last);
        app.follow_tail = false;
    }
}

/// Lands the viewport on the seq whatever opened this conversation asked for —
/// a search hit, a pin — by focusing that entry, so line-select's own scroll
/// carries it on screen. A seq not in this page is left pending for the next.
fn anchor_pending_seq(app: &mut App, session_id: &str) {
    let Some(seq) = app.pending_seq_anchor else { return };
    let Some(at) =
        app.conversation_mut(session_id).entries().iter().position(|e| e.sequenced && e.seq == seq)
    else {
        return;
    };
    app.pending_seq_anchor = None;
    app.line_cursor = Some(at);
    app.follow_tail = false;
}

/// Entry indices the filter is letting through, which is what the cursor may
/// land on: a hidden line renders no rows, so stopping on it looks like a hang.
fn selectable(app: &mut App) -> Vec<usize> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let filter = app.filter.clone();
    app.conversation_mut(&session_id)
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, entry)| filter.visible(&entry.line))
        .map(|(index, _)| index)
        .collect()
}

fn move_cursor(app: &mut App, delta: i32) {
    let Some(current) = app.line_cursor else { return };
    let rows = selectable(app);
    if rows.is_empty() {
        app.line_cursor = None;
        return;
    }
    let at = rows.iter().position(|i| *i >= current).unwrap_or(rows.len() - 1);
    let next = if delta < 0 {
        at.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (at + delta as usize).min(rows.len() - 1)
    };
    app.line_cursor = Some(rows[next]);
}

/// True while line-select owns `j`/`k`.
pub const fn line_select_active(app: &App) -> bool {
    app.line_cursor.is_some()
}

#[cfg(test)]
mod tests {
    use super::{ConversationAction, PageKind, load_older, open, reconnect};
    use crate::app::action::Effect;
    use crate::app::state::{App, ConversationLine, LineKind};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    fn line(text: &str) -> ConversationLine {
        ConversationLine::new(LineKind::Assistant, text, 0)
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
    fn opening_fetches_subscribes_and_marks_seen() {
        let mut app = app();
        let effects = open(&mut app, "s-a".to_owned());
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::LoadConversationPage { kind: PageKind::Latest, etag: None, .. },
                Effect::Subscribe { .. },
                Effect::LoadPins { .. },
                Effect::MarkSeen { .. },
            ]
        ));
        assert_eq!(app.subscribed.as_deref(), Some("s-a"));
    }

    #[test]
    fn a_live_event_before_the_open_does_not_suppress_the_history_fetch() {
        let mut app = app();
        reduce(
            &mut app,
            Action::StreamLine {
                session_id: "s-a".to_owned(),
                seq: Some(9),
                line: Some(Box::new(line("live"))),
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
    fn leaving_unsubscribes_exactly_once() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        let effects = reduce(&mut app, Action::LeaveConversation);
        assert!(
            matches!(effects.as_slice(), [Effect::Unsubscribe { session_id }] if session_id == "s-a")
        );
        assert!(app.subscribed.is_none());
        assert!(reduce(&mut app, Action::LeaveConversation).is_empty());
    }

    #[test]
    fn reconnecting_refetches_the_gap_after_the_newest_seq() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(4, "a"), (5, "b")], false);

        let effects = reconnect(&mut app);
        match effects.as_slice() {
            [Effect::Subscribe { .. }, Effect::LoadConversationPage { kind, page, .. }] => {
                assert_eq!(*kind, PageKind::Gap);
                assert_eq!(page.after, Some(5));
            }
            _ => panic!("expected a resubscribe and a gap fetch"),
        }
    }

    #[test]
    fn reconnecting_outside_a_conversation_fetches_nothing() {
        let mut app = app();
        assert!(reconnect(&mut app).is_empty());
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

    fn line_select(app: &mut App) -> Vec<Effect> {
        reduce(app, Action::Conversation(ConversationAction::ToggleLineCursor))
    }

    #[test]
    fn line_select_starts_on_the_newest_line_and_detaches_from_the_tail() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "a"), (2, "b"), (3, "c")], false);

        line_select(&mut app);
        assert_eq!(app.line_cursor, Some(2));
        assert!(!app.follow_tail);

        line_select(&mut app);
        assert_eq!(app.line_cursor, None, "the same key leaves the mode");
    }

    #[test]
    fn line_select_does_nothing_on_an_empty_transcript() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        line_select(&mut app);
        assert_eq!(app.line_cursor, None);
    }

    #[test]
    fn the_cursor_moves_with_the_line_keys_and_clamps_at_both_ends() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "a"), (2, "b"), (3, "c")], false);
        line_select(&mut app);

        reduce(&mut app, Action::Scroll { lines: -1, release_follow: true });
        assert_eq!(app.line_cursor, Some(1));
        reduce(&mut app, Action::Scroll { lines: -9, release_follow: true });
        assert_eq!(app.line_cursor, Some(1), "a page key still scrolls the viewport");

        reduce(&mut app, Action::Scroll { lines: -1, release_follow: true });
        reduce(&mut app, Action::Scroll { lines: -1, release_follow: true });
        assert_eq!(app.line_cursor, Some(0));
        for _ in 0..5 {
            reduce(&mut app, Action::Scroll { lines: 1, release_follow: false });
        }
        assert_eq!(app.line_cursor, Some(2));
    }

    #[test]
    fn leaving_exits_line_select_before_it_closes_the_conversation() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "a")], false);
        line_select(&mut app);

        assert!(reduce(&mut app, Action::LeaveConversation).is_empty());
        assert_eq!(app.view(), crate::app::View::Conversation);
        assert!(app.line_cursor.is_none());

        assert!(!reduce(&mut app, Action::LeaveConversation).is_empty());
        assert_eq!(app.view(), crate::app::View::SessionList);
    }

    #[test]
    fn an_older_page_carries_the_cursor_with_the_line_it_pointed_at() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a"), (11, "b")], true);
        line_select(&mut app);
        assert_eq!(app.line_cursor, Some(1));

        app.conversation_mut("s-a").begin_older().expect("a request");
        page(&mut app, PageKind::Older, &[(8, "older-1"), (9, "older-2")], false);
        assert_eq!(app.line_cursor, Some(3), "still the newest line");
    }

    #[test]
    fn expanding_only_touches_the_focused_collapsible_line() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        let rows = vec![
            (1_i64, line("prose")),
            (2, ConversationLine::new(LineKind::Result { error: false }, "120 lines", 0)),
        ];
        reduce(
            &mut app,
            Action::Conversation(ConversationAction::Loaded {
                session_id: "s-a".to_owned(),
                kind: PageKind::Latest,
                rows,
                etag: None,
                has_more: false,
            }),
        );
        line_select(&mut app);
        reduce(&mut app, Action::Conversation(ConversationAction::ToggleExpand));
        assert!(app.conversation_mut("s-a").entries()[1].expanded);

        reduce(&mut app, Action::Conversation(ConversationAction::MoveCursor { delta: -1 }));
        reduce(&mut app, Action::Conversation(ConversationAction::ToggleExpand));
        assert!(!app.conversation_mut("s-a").entries()[0].expanded, "prose has no body to hide");
    }

    #[test]
    fn the_bulk_toggle_alternates_without_a_cursor() {
        let mut app = app();
        open(&mut app, "s-a".to_owned());
        reduce(
            &mut app,
            Action::Conversation(ConversationAction::Loaded {
                session_id: "s-a".to_owned(),
                kind: PageKind::Latest,
                rows: vec![(
                    1,
                    ConversationLine::new(LineKind::Thinking { redacted: false }, "hmm", 0),
                )],
                etag: None,
                has_more: false,
            }),
        );
        reduce(&mut app, Action::Conversation(ConversationAction::ToggleExpandAll));
        assert!(app.expand_all);
        assert!(app.conversation_mut("s-a").entries()[0].expanded);

        reduce(&mut app, Action::Conversation(ConversationAction::ToggleExpandAll));
        assert!(!app.expand_all);
        assert!(!app.conversation_mut("s-a").entries()[0].expanded);
    }

    #[test]
    fn a_page_that_carries_the_pending_seq_lands_on_it() {
        let mut app = app();
        app.pending_seq_anchor = Some(11);
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a"), (11, "b"), (12, "c")], false);
        assert_eq!(app.line_cursor, Some(1), "the matched line takes the focus");
        assert!(!app.follow_tail, "and the viewport stops chasing the tail");
        assert!(app.pending_seq_anchor.is_none());
    }

    #[test]
    fn a_seq_this_page_does_not_hold_stays_pending_for_the_next() {
        let mut app = app();
        app.pending_seq_anchor = Some(5);
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a")], true);
        assert_eq!(app.pending_seq_anchor, Some(5));
        assert!(app.line_cursor.is_none());

        page(&mut app, PageKind::Older, &[(5, "the one")], false);
        assert!(app.pending_seq_anchor.is_none());
        assert_eq!(app.line_cursor, Some(0));
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
