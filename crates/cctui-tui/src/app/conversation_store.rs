//! Seq-ordered transcript store, one per session.
//!
//! `seq` is the server's per-session insert order and a strict total order, so
//! history pages and live events merge into one list whatever order they
//! arrive in, and a re-delivered event is dropped rather than rendered twice.

use std::collections::HashSet;

use super::state::{ConversationLine, LineKind, LineStatus};

pub const PAGE_LIMIT: i64 = 200;

/// A `before`/`after`/`limit` window over one session's transcript.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageRequest {
    /// Exclusive upper `seq` bound — page backwards.
    pub before: Option<i64>,
    /// Exclusive lower `seq` bound — catch up on a gap.
    pub after: Option<i64>,
    pub limit: Option<i64>,
}

impl PageRequest {
    #[must_use]
    pub const fn latest(limit: i64) -> Self {
        Self { before: None, after: None, limit: Some(limit) }
    }

    #[must_use]
    pub const fn before(seq: i64, limit: i64) -> Self {
        Self { before: Some(seq), after: None, limit: Some(limit) }
    }

    #[must_use]
    pub const fn after(seq: i64) -> Self {
        Self { before: None, after: Some(seq), limit: None }
    }
}

/// Which fetch a page answers. The store applies each differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    /// The newest page, fetched when the conversation is opened.
    Latest,
    /// Rows older than everything held, fetched at the top of the scrollback.
    Older,
    /// Rows missed while the socket was down.
    Gap,
}

/// One rendered transcript entry.
pub struct Entry {
    /// The server's `seq`. Events that carried none borrow the newest `seq`
    /// held when they arrived, so they keep their arrival position.
    pub seq: i64,
    /// False when the event carried no `seq`: those dedupe by content, not id.
    pub sequenced: bool,
    pub line: ConversationLine,
}

/// What a merge changed, for the caller's scroll and cache bookkeeping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Merge {
    pub inserted: usize,
    /// True when at least one entry landed before the end of the list: the
    /// render cache cannot be appended to and the viewport has moved.
    pub reordered: bool,
}

#[derive(Default)]
pub struct ConversationStore {
    entries: Vec<Entry>,
    seqs: HashSet<i64>,
    /// `ETag` of the latest-page response, replayed as `If-None-Match`.
    etag: Option<String>,
    /// Bumped whenever an entry lands anywhere but the end. A render cache that
    /// only appends is valid exactly while this does not change.
    epoch: u64,
    /// True once any page has landed — not merely once an event has. A live
    /// event arriving before the history fetch must not pass for a load.
    loaded: bool,
    has_more_older: bool,
    loading_older: bool,
}

impl ConversationStore {
    #[cfg(test)]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn lines(&self) -> impl Iterator<Item = &ConversationLine> {
        self.entries.iter().map(|e| &e.line)
    }

    #[cfg(test)]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Lowest `seq` held: the `before` cursor for the next older page.
    #[must_use]
    pub fn oldest_seq(&self) -> Option<i64> {
        self.entries.iter().find(|e| e.sequenced).map(|e| e.seq)
    }

    /// Highest `seq` held: the `after` cursor for a gap refetch.
    #[must_use]
    pub fn newest_seq(&self) -> Option<i64> {
        self.entries.iter().rev().find(|e| e.sequenced).map(|e| e.seq)
    }

    /// The page to ask for when the conversation is opened. Always issued, even
    /// for an already-loaded conversation: the stored `ETag` makes the repeat
    /// cheap, and gating the fetch on "do we hold any lines" is what let a
    /// single live event suppress the history load.
    #[must_use]
    pub const fn latest_request() -> PageRequest {
        PageRequest::latest(PAGE_LIMIT)
    }

    /// The page that closes a reconnect gap: everything after the newest `seq`
    /// held, or the latest page when nothing is held to anchor on.
    #[must_use]
    pub fn gap_request(&self) -> PageRequest {
        self.newest_seq().map_or_else(Self::latest_request, PageRequest::after)
    }

    /// Claims the older-page fetch, returning the window to ask for. `None`
    /// when there is nothing older, a fetch is already in flight, or no page
    /// has landed yet — so holding the scroll at the top cannot stack requests.
    pub fn begin_older(&mut self) -> Option<PageRequest> {
        if !self.loaded || self.loading_older || !self.has_more_older {
            return None;
        }
        let before = self.oldest_seq()?;
        self.loading_older = true;
        Some(PageRequest::before(before, PAGE_LIMIT))
    }

    /// Merges a fetched page. `rows` may arrive in any order and may overlap
    /// what is already held.
    pub fn merge_page(
        &mut self,
        kind: PageKind,
        rows: Vec<(i64, ConversationLine)>,
        etag: Option<String>,
        has_more: bool,
    ) -> Merge {
        let epoch_before = self.epoch;
        let mut inserted = 0;
        for (seq, line) in rows {
            if self.insert(seq, true, line) {
                inserted += 1;
            }
        }
        self.finish_page(kind, etag, has_more);
        Merge { inserted, reordered: self.epoch != epoch_before }
    }

    /// A 304: the page is unchanged, so only the in-flight bookkeeping moves.
    pub fn page_not_modified(&mut self, kind: PageKind) {
        self.finish_page(kind, self.etag.clone(), self.has_more_older);
    }

    /// A failed fetch releases the in-flight claim so a later scroll retries.
    pub const fn page_failed(&mut self, kind: PageKind) {
        if matches!(kind, PageKind::Older) {
            self.loading_older = false;
        }
    }

    fn finish_page(&mut self, kind: PageKind, etag: Option<String>, has_more: bool) {
        match kind {
            PageKind::Latest => {
                self.loaded = true;
                self.etag = etag;
                // A full latest page means rows exist beyond its oldest row —
                // but a later page already proved that, so never walk it back.
                self.has_more_older |= has_more;
            }
            PageKind::Older => {
                self.loading_older = false;
                self.has_more_older = has_more;
            }
            PageKind::Gap => self.loaded = true,
        }
    }

    /// A live stream event. Returns false when it was a duplicate.
    pub fn push_live(&mut self, seq: Option<i64>, line: ConversationLine) -> bool {
        if let Some(seq) = seq {
            return self.insert(seq, true, line);
        }
        // No `seq` to dedupe on (an older server, or an event type the daemon
        // does not stamp): suppress an immediate repeat, the only duplicate
        // seen in practice.
        let repeat = self
            .entries
            .last()
            .is_some_and(|last| last.line.kind == line.kind && last.line.text == line.text);
        if repeat {
            return false;
        }
        let seq = self.entries.last().map_or(0, |e| e.seq);
        self.insert(seq, false, line)
    }

    fn insert(&mut self, seq: i64, sequenced: bool, line: ConversationLine) -> bool {
        if sequenced && !self.seqs.insert(seq) {
            return false;
        }
        // Equal `seq` sorts after what is already held, so an unsequenced event
        // that borrowed the newest `seq` stays behind the entry it followed.
        let at = self.entries.partition_point(|e| e.seq <= seq);
        if at < self.entries.len() {
            self.epoch += 1;
        }
        self.entries.insert(at, Entry { seq, sequenced, line });
        self.close_queued(at);
        true
    }

    /// The prompt the agent finally ran, or the human withdrew, retires the
    /// placeholder that was standing in for it.
    fn close_queued(&mut self, at: usize) {
        let line = &self.entries[at].line;
        let (keys, exact) = match (&line.kind, &line.status) {
            (LineKind::User, None) => (delivered_keys(&line.text), true),
            (LineKind::System, Some(LineStatus::Removed)) => {
                (vec![queue_key(&line.text).to_owned()], false)
            }
            _ => return,
        };
        let placeholder = self.entries.iter().position(|e| {
            e.line.status == Some(LineStatus::Queued)
                && keys.iter().any(|delivered| matches(&e.line.text, delivered, exact))
        });
        let Some(placeholder) = placeholder else { return };
        self.entries.remove(placeholder);
        self.epoch += 1;
    }
}

/// Queue records carry no id, so a placeholder is matched to its prompt by
/// text. Only the first line survives truncation on the server side.
fn queue_key(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim().trim_end_matches('…')
}

/// A single turn can deliver several queued prompts stacked line by line.
fn delivered_keys(text: &str) -> Vec<String> {
    text.lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect()
}

fn matches(placeholder: &str, other: &str, exact: bool) -> bool {
    let key = queue_key(placeholder);
    if key.is_empty() {
        return !exact && other.is_empty();
    }
    other.starts_with(key) || (!exact && key.starts_with(other) && !other.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{ConversationStore, PAGE_LIMIT, PageKind, PageRequest};
    use crate::app::state::{ConversationLine, LineKind, LineStatus};

    fn line(text: &str) -> ConversationLine {
        ConversationLine::new(LineKind::Assistant, text, 0)
    }

    fn queued(text: &str) -> ConversationLine {
        ConversationLine::new(LineKind::User, text, 0).with_status(LineStatus::Queued)
    }

    fn rows(specs: &[(i64, &str)]) -> Vec<(i64, ConversationLine)> {
        specs.iter().map(|(seq, text)| (*seq, line(text))).collect()
    }

    fn texts(store: &ConversationStore) -> Vec<String> {
        store.lines().map(|l| l.text.clone()).collect()
    }

    fn loaded() -> ConversationStore {
        let mut store = ConversationStore::new();
        store.merge_page(
            PageKind::Latest,
            rows(&[(10, "a"), (11, "b"), (12, "c")]),
            Some("etag-1".to_owned()),
            true,
        );
        store
    }

    #[test]
    fn a_page_is_ordered_by_seq_however_it_arrives() {
        let mut store = ConversationStore::new();
        let merge = store.merge_page(
            PageKind::Latest,
            rows(&[(12, "c"), (10, "a"), (11, "b")]),
            None,
            false,
        );
        assert_eq!(merge.inserted, 3);
        assert_eq!(texts(&store), ["a", "b", "c"]);
        assert_eq!(store.oldest_seq(), Some(10));
        assert_eq!(store.newest_seq(), Some(12));
    }

    #[test]
    fn a_live_event_out_of_order_lands_in_seq_position() {
        let mut store = loaded();
        assert!(store.push_live(Some(14), line("e")));
        assert!(store.push_live(Some(13), line("d")));
        assert_eq!(texts(&store), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn a_duplicate_seq_is_dropped_whichever_side_it_comes_from() {
        let mut store = loaded();
        assert!(!store.push_live(Some(11), line("b again")));
        assert_eq!(store.len(), 3);

        let merge = store.merge_page(PageKind::Gap, rows(&[(11, "b"), (13, "d")]), None, false);
        assert_eq!(merge.inserted, 1);
        assert_eq!(texts(&store), ["a", "b", "c", "d"]);
    }

    #[test]
    fn appending_at_the_tail_keeps_the_render_cache_valid() {
        let mut store = loaded();
        let epoch = store.epoch();
        assert!(store.push_live(Some(13), line("d")));
        assert_eq!(store.epoch(), epoch, "a tail append must not invalidate the cache");

        assert!(store.push_live(Some(9), line("earlier")));
        assert!(store.epoch() > epoch, "an out-of-order insert must invalidate it");
    }

    #[test]
    fn a_live_event_does_not_count_as_a_loaded_conversation() {
        let mut store = ConversationStore::new();
        store.push_live(Some(5), line("live"));
        assert!(!store.loaded, "the history fetch must still go out");
        assert!(!store.is_empty());

        store.merge_page(PageKind::Latest, rows(&[(4, "history")]), None, false);
        assert!(store.loaded);
        assert_eq!(texts(&store), ["history", "live"]);
    }

    #[test]
    fn unsequenced_events_append_and_dedupe_on_content() {
        let mut store = loaded();
        assert!(store.push_live(None, line("tail")));
        assert!(!store.push_live(None, line("tail")), "an immediate repeat is dropped");
        assert!(store.push_live(None, line("other")));
        assert_eq!(texts(&store), ["a", "b", "c", "tail", "other"]);
        assert_eq!(store.newest_seq(), Some(12), "unsequenced entries are not paging anchors");
    }

    #[test]
    fn an_unsequenced_event_stays_behind_the_entry_it_followed() {
        let mut store = loaded();
        store.push_live(None, line("after c"));
        store.push_live(Some(13), line("d"));
        assert_eq!(texts(&store), ["a", "b", "c", "after c", "d"]);
    }

    #[test]
    fn the_gap_request_starts_after_the_newest_seq_held() {
        let store = loaded();
        assert_eq!(store.gap_request(), PageRequest::after(12));

        let empty = ConversationStore::new();
        assert_eq!(empty.gap_request(), PageRequest::latest(PAGE_LIMIT));
    }

    #[test]
    fn a_gap_page_merges_without_disturbing_the_paging_cursor() {
        let mut store = loaded();
        assert!(store.has_more_older);
        store.merge_page(PageKind::Gap, rows(&[(13, "d"), (14, "e")]), None, false);
        assert!(store.has_more_older, "a gap page says nothing about older rows");
        assert_eq!(texts(&store), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn older_paging_walks_back_and_stops_at_the_start() {
        let mut store = loaded();
        let request = store.begin_older().expect("an older page is available");
        assert_eq!(request, PageRequest::before(10, PAGE_LIMIT));

        assert!(store.begin_older().is_none(), "a fetch is already in flight");

        let merge =
            store.merge_page(PageKind::Older, rows(&[(8, "older-1"), (9, "older-2")]), None, false);
        assert_eq!(merge.inserted, 2);
        assert!(merge.reordered, "prepending invalidates the render cache");
        assert_eq!(texts(&store), ["older-1", "older-2", "a", "b", "c"]);

        assert!(!store.has_more_older);
        assert!(store.begin_older().is_none(), "the transcript start is reached");
    }

    #[test]
    fn an_older_page_that_fills_the_limit_leaves_more_to_load() {
        let mut store = loaded();
        store.begin_older().expect("a request");
        store.merge_page(PageKind::Older, rows(&[(9, "older")]), None, true);
        assert!(store.has_more_older);
        assert_eq!(store.begin_older(), Some(PageRequest::before(9, PAGE_LIMIT)));
    }

    #[test]
    fn nothing_pages_before_the_first_page_lands() {
        let mut store = ConversationStore::new();
        store.push_live(Some(5), line("live"));
        assert!(store.begin_older().is_none());
    }

    #[test]
    fn a_failed_older_page_can_be_retried() {
        let mut store = loaded();
        store.begin_older().expect("a request");
        store.page_failed(PageKind::Older);
        assert_eq!(store.begin_older(), Some(PageRequest::before(10, PAGE_LIMIT)));
    }

    #[test]
    fn not_modified_keeps_the_etag_and_the_entries() {
        let mut store = loaded();
        store.page_not_modified(PageKind::Latest);
        assert_eq!(store.etag(), Some("etag-1"));
        assert_eq!(texts(&store), ["a", "b", "c"]);
        assert!(store.loaded);

        store.begin_older().expect("a request");
        store.page_not_modified(PageKind::Older);
        assert!(!store.loading_older);
        assert!(store.has_more_older, "a 304 must not claim the transcript ended");
    }

    #[test]
    fn a_refetched_latest_page_replaces_the_etag_and_keeps_the_history() {
        let mut store = loaded();
        store.begin_older().expect("a request");
        store.merge_page(PageKind::Older, rows(&[(9, "older")]), None, false);
        assert!(!store.has_more_older);

        store.merge_page(
            PageKind::Latest,
            rows(&[(12, "c"), (13, "d")]),
            Some("etag-2".to_owned()),
            false,
        );
        assert_eq!(store.etag(), Some("etag-2"));
        assert!(!store.has_more_older, "a later page already proved the start was reached");
        assert_eq!(texts(&store), ["older", "a", "b", "c", "d"]);
    }

    #[test]
    fn an_empty_store_has_no_cursors() {
        let store = ConversationStore::new();
        assert!(store.is_empty());
        assert_eq!(store.oldest_seq(), None);
        assert_eq!(store.newest_seq(), None);
        assert_eq!(store.etag(), None);
        assert!(!store.loaded);
    }

    #[test]
    fn a_queued_prompt_clears_when_its_own_turn_is_delivered() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), queued("ship the thing"));
        store.push_live(Some(2), queued("then tag it"));
        assert_eq!(texts(&store), ["ship the thing", "then tag it"]);

        let epoch = store.epoch();
        store.push_live(Some(3), ConversationLine::new(LineKind::User, "ship the thing", 0));
        assert_eq!(texts(&store), ["then tag it", "ship the thing"]);
        assert!(store.epoch() > epoch, "retiring a placeholder invalidates the render cache");

        store.push_live(Some(4), ConversationLine::new(LineKind::User, "then tag it", 0));
        assert_eq!(texts(&store), ["ship the thing", "then tag it"]);
        assert!(store.lines().all(|l| l.status.is_none()), "nothing is still queued");
    }

    #[test]
    fn one_delivered_turn_absorbs_the_prompt_it_stacked() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), queued("ship the thing"));
        store.push_live(
            Some(2),
            ConversationLine::new(LineKind::User, "and now\nship the thing please", 0),
        );
        assert_eq!(texts(&store), ["and now\nship the thing please"]);
    }

    #[test]
    fn a_withdrawn_prompt_leaves_the_removal_note_and_no_placeholder() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), queued("ship the thing"));
        store.push_live(
            Some(2),
            ConversationLine::new(LineKind::System, "ship the thing", 0)
                .with_status(LineStatus::Removed),
        );
        assert_eq!(texts(&store), ["ship the thing"]);
        assert_eq!(store.entries()[0].line.status, Some(LineStatus::Removed));
    }

    #[test]
    fn an_unrelated_user_message_leaves_the_queue_alone() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), queued("ship the thing"));
        store.push_live(Some(2), ConversationLine::new(LineKind::User, "what is the status", 0));
        assert_eq!(store.len(), 2);
        assert_eq!(store.entries()[0].line.status, Some(LineStatus::Queued));
    }
}
