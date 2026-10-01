use super::action::Effect;
use super::conversation_store::{ConversationStore, PageKind};
use super::state::{App, ConversationLine, View};

pub enum ConversationAction {
    Loaded {
        session_id: String,
        kind: PageKind,
        /// Which older-page request this answers; `None` for a page nothing
        /// claimed (the newest page, a gap refetch).
        claim: Option<u64>,
        rows: Vec<(i64, ConversationLine)>,
        etag: Option<String>,
        has_more: bool,
    },
    NotModified {
        session_id: String,
        kind: PageKind,
        claim: Option<u64>,
    },
    Failed {
        session_id: String,
        kind: PageKind,
        claim: Option<u64>,
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
        ConversationAction::Loaded { session_id, kind, claim, rows, etag, has_more } => {
            let merge =
                app.conversation_mut(&session_id).merge_page(kind, claim, rows, etag, has_more);
            // The cursor, the viewport and the anchor all belong to whatever is
            // on screen; a background session's page must not move them.
            let on_screen = app.subscribed.as_deref() == Some(session_id.as_str());
            if on_screen && kind == PageKind::Older && merge.inserted > 0 && merge.reordered {
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
        ConversationAction::NotModified { session_id, kind, claim } => {
            app.conversation_mut(&session_id).page_not_modified(kind, claim);
            Vec::new()
        }
        ConversationAction::Failed { session_id, kind, claim } => {
            app.conversation_mut(&session_id).page_failed(kind, claim);
            super::pins::page_failed(app);
            Vec::new()
        }
    }
}

/// Opening always refetches the newest page, whatever is already buffered: the
/// stored `ETag` makes the repeat a 304, and any cheaper gate loses the history
/// to a live event that happens to arrive first.
pub fn open(app: &mut App, session_id: String) -> Vec<Effect> {
    // An anchor for another session would otherwise fire on this one's first
    // page, at whatever line happens to carry that seq.
    if app.pending_seq_anchor.as_ref().is_some_and(|(id, _)| *id != session_id) {
        app.pending_seq_anchor = None;
    }
    app.follow_tail = true;
    app.scroll_offset = 0;
    app.line_cursor = None;
    app.router.push(View::Conversation);
    app.subscribed = Some(session_id.clone());
    super::unread::opened(app, &session_id);
    evict_cold_stores(app);

    let store = app.conversation_mut(&session_id);
    let page = ConversationStore::latest_request();
    let etag = store.etag().map(str::to_owned);
    vec![
        Effect::LoadConversationPage {
            session_id: session_id.clone(),
            kind: PageKind::Latest,
            claim: None,
            page,
            etag,
        },
        Effect::Subscribe { session_id: session_id.clone() },
        super::pins::on_open(&session_id),
        super::spend::on_conversation_open(&session_id),
        Effect::MarkSeen { session_id },
    ]
}

/// Move the whole view to another session: leave whatever is subscribed, put
/// the selection on the target and open it.
pub fn switch_to(app: &mut App, session_id: String) -> Vec<Effect> {
    if !app.select_session_id(&session_id) {
        return Vec::new();
    }
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
        Effect::LoadConversationPage {
            session_id,
            kind: PageKind::Gap,
            claim: None,
            page,
            etag: None,
        },
    ]
}

pub fn load_older(app: &mut App) -> Vec<Effect> {
    if app.view() != View::Conversation || app.scroll_offset != 0 || app.follow_tail {
        return Vec::new();
    }
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let Some((page, claim)) = app.conversation_mut(&session_id).begin_older() else {
        return Vec::new();
    };
    vec![Effect::LoadConversationPage {
        session_id,
        kind: PageKind::Older,
        claim: Some(claim),
        page,
        etag: None,
    }]
}

pub fn stream(app: &mut App, session_id: &str, seq: Option<i64>, line: ConversationLine) {
    let trimmed = {
        let store = app.conversation_mut(session_id);
        store.push_live(seq, line);
        store.trim_to_cap()
    };
    // Entries are addressed by index, so dropping the oldest moves the cursor
    // of whatever is on screen the same way a prepend does, in reverse.
    if trimmed > 0 && app.subscribed.as_deref() == Some(session_id) {
        app.pending_prepend = true;
        if let Some(cursor) = app.line_cursor.as_mut() {
            *cursor = cursor.saturating_sub(trimmed);
        }
    }
    evict_cold_stores(app);
}

/// Bound the buffered transcripts. Runs where stores are created — opening a
/// conversation and streaming into a background one — and is a length check
/// until the cap is actually exceeded.
fn evict_cold_stores(app: &mut App) {
    if app.conversations.len() <= super::conversation_store::MAX_STORES {
        return;
    }
    let keep = app.conversations_in_use();
    let dropped = super::conversation_store::evict_cold(
        &mut app.conversations,
        &keep,
        super::conversation_store::MAX_STORES,
    );
    if !dropped.is_empty() {
        tracing::debug!(count = dropped.len(), "evicted cold transcripts");
    }
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
    let Some((anchored, seq)) = app.pending_seq_anchor.clone() else { return };
    if anchored != session_id {
        return;
    }
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
    /// The claim the store is waiting on, which is what the reply to the live
    /// request carries.
    fn outstanding(app: &mut App, session_id: &str, kind: PageKind) -> Option<u64> {
        if kind == PageKind::Older {
            app.conversation_mut(session_id).outstanding_claim()
        } else {
            None
        }
    }

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
        let claim = outstanding(app, "s-a", kind);
        reduce(
            app,
            Action::Conversation(ConversationAction::Loaded {
                session_id: "s-a".to_owned(),
                kind,
                claim,
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
                Effect::FetchSessionLangfuse { .. },
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

        let claim = outstanding(&mut app, "s-a", PageKind::Older);
        reduce(
            &mut app,
            Action::Conversation(ConversationAction::Failed {
                session_id: "s-a".to_owned(),
                kind: PageKind::Older,
                claim,
            }),
        );
        assert!(!load_older(&mut app).is_empty(), "the failed page can be asked for again");
    }

    /// A page for a session other than `s-a`, which the helper above hardcodes.
    fn page_for(app: &mut App, session_id: &str, kind: PageKind, rows: &[(i64, &str)]) {
        let rows = rows.iter().map(|(seq, text)| (*seq, line(text))).collect();
        let claim = outstanding(app, session_id, kind);
        reduce(
            app,
            Action::Conversation(ConversationAction::Loaded {
                session_id: session_id.to_owned(),
                kind,
                claim,
                rows,
                etag: None,
                has_more: false,
            }),
        );
    }

    #[test]
    fn a_background_sessions_older_page_leaves_the_visible_cursor_alone() {
        let mut app = app();
        app.sessions.push(session("s-b", "beta", "active", "working"));
        app.update_aggregates();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a"), (11, "b")], true);
        line_select(&mut app);
        assert_eq!(app.line_cursor, Some(1));

        // s-b holds newer rows, so its older page really prepends — the case
        // that used to shift whatever cursor was on screen.
        page_for(&mut app, "s-b", PageKind::Latest, &[(20, "b-new")]);
        page_for(&mut app, "s-b", PageKind::Older, &[(1, "x"), (2, "y"), (3, "z")]);
        assert_eq!(app.conversation_mut("s-b").entries().len(), 4, "the page did land");

        assert_eq!(app.line_cursor, Some(1), "the cursor still points at the line it was on");
        assert!(!app.pending_prepend, "and the viewport is not re-anchored");
    }

    #[test]
    fn another_sessions_page_cannot_consume_the_search_anchor() {
        let mut app = app();
        app.sessions.push(session("s-b", "beta", "active", "working"));
        app.update_aggregates();
        app.pending_seq_anchor = Some(("s-a".to_owned(), 11));
        open(&mut app, "s-a".to_owned());

        // s-b happens to hold seq 11 too — seqs are per session.
        page_for(&mut app, "s-b", PageKind::Latest, &[(11, "someone else's line")]);
        assert_eq!(
            app.pending_seq_anchor,
            Some(("s-a".to_owned(), 11)),
            "the anchor waits for its own session"
        );
        assert!(app.line_cursor.is_none(), "and nothing is focused in the open conversation");

        page(&mut app, PageKind::Latest, &[(10, "a"), (11, "the hit"), (12, "c")], false);
        assert_eq!(app.line_cursor, Some(1), "the real page lands on the hit");
        assert!(app.pending_seq_anchor.is_none());
    }

    #[test]
    fn opening_a_different_conversation_drops_a_stale_anchor() {
        let mut app = app();
        app.sessions.push(session("s-b", "beta", "active", "working"));
        app.update_aggregates();
        app.pending_seq_anchor = Some(("s-a".to_owned(), 11));

        open(&mut app, "s-b".to_owned());
        assert!(app.pending_seq_anchor.is_none(), "an anchor for elsewhere must not fire here");
    }

    /// F12 residual: the subscribed store is capped too, and the cursor follows
    /// the lines it pointed at rather than the indices they used to have.
    #[test]
    fn capping_the_open_transcript_carries_the_cursor_with_its_line() {
        use super::super::conversation_store::MAX_ENTRIES;

        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "oldest")], false);
        let cap = i64::try_from(MAX_ENTRIES).expect("the cap fits");
        for seq in 2..=cap {
            super::stream(&mut app, "s-a", Some(seq), line("chatter"));
        }
        line_select(&mut app);
        let before = app.line_cursor.expect("a cursor");
        assert_eq!(app.conversation_mut("s-a").entries().len(), MAX_ENTRIES);

        super::stream(&mut app, "s-a", Some(cap + 1), line("one too many"));

        let store = app.conversation_mut("s-a");
        assert_eq!(store.entries().len(), MAX_ENTRIES, "the transcript is bounded");
        assert_eq!(store.oldest_seq(), Some(2), "the oldest line went");
        assert_eq!(
            app.line_cursor,
            Some(before - 1),
            "the cursor moved down one with the line it was on"
        );
        assert!(app.pending_prepend, "and the viewport is re-anchored");
    }

    #[test]
    fn cold_transcripts_are_evicted_but_the_open_one_survives() {
        use super::super::conversation_store::MAX_STORES;

        let mut app = app();
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(1, "mine")], false);

        for i in 0..MAX_STORES + 8 {
            super::stream(&mut app, &format!("bg-{i}"), Some(1), line("background chatter"));
        }

        assert!(
            app.conversations.len() <= MAX_STORES,
            "stores are bounded, got {}",
            app.conversations.len()
        );
        assert!(app.conversations.contains_key("s-a"), "the open conversation is never evicted");
        assert_eq!(app.conversation_mut("s-a").entries().len(), 1, "and it keeps the lines it had");
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
                claim: None,
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
                claim: None,
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
        app.pending_seq_anchor = Some(("s-a".to_owned(), 11));
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a"), (11, "b"), (12, "c")], false);
        assert_eq!(app.line_cursor, Some(1), "the matched line takes the focus");
        assert!(!app.follow_tail, "and the viewport stops chasing the tail");
        assert!(app.pending_seq_anchor.is_none());
    }

    #[test]
    fn a_seq_this_page_does_not_hold_stays_pending_for_the_next() {
        let mut app = app();
        app.pending_seq_anchor = Some(("s-a".to_owned(), 5));
        open(&mut app, "s-a".to_owned());
        page(&mut app, PageKind::Latest, &[(10, "a")], true);
        assert_eq!(app.pending_seq_anchor, Some(("s-a".to_owned(), 5)));
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
                claim: None,
            }),
        );
        assert_eq!(
            app.conversation("s-a").map(crate::app::conversation_store::ConversationStore::len),
            Some(2)
        );
    }
}
