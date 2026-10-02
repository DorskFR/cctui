//! Seq-ordered transcript store, one per session.
//!
//! `seq` is the server's per-session insert order and a strict total order, so
//! history pages and live events merge into one list whatever order they
//! arrive in, and a re-delivered event is dropped rather than rendered twice.

use std::collections::HashSet;

use super::state::{ConversationLine, LineKind, LineStatus};

pub const PAGE_LIMIT: i64 = 200;

/// How many transcripts stay buffered. A session left streaming in the
/// background keeps every line it ever produced otherwise, and a TUI open for
/// days on a busy fleet grows without bound. Pages are refetchable; memory is
/// not recoverable.
pub const MAX_STORES: usize = 24;

/// How many entries one transcript keeps. The subscribed store grows for as
/// long as the session streams, so the oldest rows are dropped once past this:
/// they are refetchable by older paging, which is what the user scrolling up
/// would do anyway.
pub const MAX_ENTRIES: usize = 5_000;

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
    /// Whether a collapsible line shows its body. Never read for a line that is
    /// not collapsible.
    pub expanded: bool,
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
    /// Token of the one older page whose reply is still welcome. `None` once
    /// nothing is outstanding — including after a reset, which voids whatever
    /// was in flight: its rows belong below a window that is gone.
    older_claim: Option<u64>,
    /// Minted per claim, so a reply identifies the request it answers and not
    /// merely the state the store was in when it was issued.
    next_claim: u64,
    /// Value of the app's access counter when this store was last read or
    /// written: the eviction order.
    pub touched: u64,
}

/// Drop the coldest buffered transcripts once there are more than `max`.
/// `keep` is never evicted: the open conversation, the selection, and any
/// session whose send has not been acked (its pending line lives only here).
pub fn evict_cold(
    stores: &mut std::collections::HashMap<String, ConversationStore>,
    keep: &HashSet<String>,
    max: usize,
) -> Vec<String> {
    if stores.len() <= max {
        return Vec::new();
    }
    let mut candidates: Vec<(u64, String)> = stores
        .iter()
        .filter(|(id, _)| !keep.contains(*id))
        .map(|(id, store)| (store.touched, id.clone()))
        .collect();
    candidates.sort_unstable();
    let over = stores.len() - max;
    let doomed: Vec<String> = candidates.into_iter().take(over).map(|(_, id)| id).collect();
    for id in &doomed {
        stores.remove(id);
    }
    doomed
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

    /// The claim the store is waiting on, for a test that has to answer it.
    #[cfg(test)]
    #[must_use]
    pub const fn outstanding_claim(&self) -> Option<u64> {
        self.older_claim
    }

    /// Answer whatever older page is outstanding — what the production path
    /// does, since the claim travels with the request.
    #[cfg(test)]
    fn merge(
        &mut self,
        kind: PageKind,
        rows: Vec<(i64, ConversationLine)>,
        etag: Option<String>,
        has_more: bool,
    ) -> Merge {
        let claim = if kind == PageKind::Older { self.older_claim } else { None };
        self.merge_page(kind, claim, rows, etag, has_more)
    }

    #[cfg(test)]
    fn not_modified(&mut self, kind: PageKind) {
        let claim = if kind == PageKind::Older { self.older_claim } else { None };
        self.page_not_modified(kind, claim);
    }

    #[cfg(test)]
    fn failed(&mut self, kind: PageKind) {
        let claim = if kind == PageKind::Older { self.older_claim } else { None };
        self.page_failed(kind, claim);
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
    pub fn begin_older(&mut self) -> Option<(PageRequest, u64)> {
        if !self.loaded || self.loading_older || !self.has_more_older {
            return None;
        }
        let before = self.oldest_seq()?;
        self.loading_older = true;
        self.next_claim += 1;
        self.older_claim = Some(self.next_claim);
        Some((PageRequest::before(before, PAGE_LIMIT), self.next_claim))
    }

    /// Merges a fetched page. `rows` may arrive in any order and may overlap
    /// what is already held.
    pub fn merge_page(
        &mut self,
        kind: PageKind,
        claim: Option<u64>,
        rows: Vec<(i64, ConversationLine)>,
        etag: Option<String>,
        has_more: bool,
    ) -> Merge {
        if kind == PageKind::Older && self.older_page_is_stale(claim) {
            return Merge::default();
        }
        if kind == PageKind::Latest
            && let Some(page_oldest) = rows.iter().map(|(seq, _)| *seq).min()
            && !self.joins_onto_held_history(page_oldest)
        {
            self.drop_history_below(page_oldest);
        }
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

    /// The seq a newest page has to join onto: whether `page_oldest - 1` is
    /// held decides it, because older paging walks back from the *oldest* seq
    /// held and would step straight over anything missing above it.
    ///
    /// Judged against every held seq, not the newest: a live event above the
    /// gap (a background session that kept streaming, one event landing between
    /// the open and the page reply) says nothing about whether the history
    /// below the page is contiguous.
    fn joins_onto_held_history(&self, page_oldest: i64) -> bool {
        self.seqs.contains(&(page_oldest - 1))
    }

    /// Drop the held rows a newest page cannot be paged back to, keeping the
    /// ones at or above its oldest seq — those are the live events that arrived
    /// after the page was taken, and they are still reachable.
    ///
    /// The dropped rows are all refetchable, by exactly the older paging that
    /// now starts at `floor`; a hole is not.
    fn drop_history_below(&mut self, floor: i64) {
        let before = self.entries.len();
        self.entries.retain(|e| !e.sequenced || e.seq >= floor);
        self.seqs.retain(|seq| *seq >= floor);
        if self.entries.len() == before {
            return;
        }
        self.epoch += 1;
        self.has_more_older = false;
        self.older_claim = None;
        self.loading_older = false;
    }

    /// Whether the older page now arriving is not the one the store is waiting
    /// for. Only the live claim's own reply may land: a reply whose claim was
    /// voided by a reset — or superseded by a re-claim while it was still in
    /// flight — carries rows below a window that no longer exists, and merging
    /// them re-opens the hole the reset closed.
    ///
    /// Leaves the claim alone: refusing a reply must not retire the request
    /// still outstanding, or the genuine reply would be refused in its turn.
    const fn older_page_is_stale(&self, claim: Option<u64>) -> bool {
        match (claim, self.older_claim) {
            (Some(arriving), Some(outstanding)) => arriving != outstanding,
            (None, None) => false,
            // One side names a request the other does not: a reply to something
            // abandoned, or a page nothing asked for.
            _ => true,
        }
    }

    /// Drop the oldest entries once the transcript is over `MAX_ENTRIES`,
    /// returning how many went. The caller owns the viewport, so it has to
    /// shift a cursor that addresses entries by index.
    pub fn trim_to_cap(&mut self) -> usize {
        if self.entries.len() <= MAX_ENTRIES {
            return 0;
        }
        let over = self.entries.len() - MAX_ENTRIES;
        for entry in self.entries.drain(..over) {
            if entry.sequenced {
                self.seqs.remove(&entry.seq);
            }
        }
        self.epoch += 1;
        // There is demonstrably more older than is held now, and paging back
        // re-fetches exactly what was dropped.
        self.has_more_older = true;
        over
    }

    /// A 304: the page is unchanged, so only the in-flight bookkeeping moves.
    pub fn page_not_modified(&mut self, kind: PageKind, claim: Option<u64>) {
        if kind == PageKind::Older && self.older_page_is_stale(claim) {
            return;
        }
        self.finish_page(kind, self.etag.clone(), self.has_more_older);
    }

    /// A failed fetch releases the in-flight claim so a later scroll retries —
    /// but only its own: a stale request's failure must not release the claim
    /// of the one that replaced it.
    pub const fn page_failed(&mut self, kind: PageKind, claim: Option<u64>) {
        if matches!(kind, PageKind::Older) && !self.older_page_is_stale(claim) {
            self.loading_older = false;
            self.older_claim = None;
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
                self.older_claim = None;
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
        self.entries.insert(at, Entry { seq, sequenced, line, expanded: false });
        self.close_queued(at);
        self.pair_tool_lines();
        true
    }

    /// Ties every result that names its call to that call. A result directly
    /// under its call reads as its answer already; one that is not, because
    /// parallel calls interleave, is labelled with its call's tool. A result
    /// without an id keeps answering whatever sits above it.
    fn pair_tool_lines(&mut self) {
        let mut calls: std::collections::HashMap<String, (usize, String)> =
            std::collections::HashMap::new();
        let mut changed = false;
        for i in 0..self.entries.len() {
            let line = &self.entries[i].line;
            let Some(id) = line.tool_use_id.clone() else { continue };
            match line.kind {
                LineKind::Tool { .. } => {
                    calls.insert(id, (i, line.tool.clone().unwrap_or_default()));
                }
                LineKind::Result { .. } => {
                    let answers =
                        calls.get(&id).filter(|(at, _)| at + 1 != i).map(|(_, tool)| tool.clone());
                    let line = &mut self.entries[i].line;
                    if line.answers != answers {
                        line.answers = answers;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed {
            self.epoch += 1;
        }
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

    /// Flips one entry's collapse state. Bumping `epoch` is what invalidates the
    /// render cache: an expanded line occupies a different number of rows.
    pub fn toggle_expanded(&mut self, index: usize) -> bool {
        let Some(entry) = self.entries.get_mut(index).filter(|e| e.line.collapsible()) else {
            return false;
        };
        entry.expanded = !entry.expanded;
        self.epoch += 1;
        true
    }

    /// Sets every collapsible entry at once. Returns false when nothing moved.
    pub fn set_all_expanded(&mut self, expanded: bool) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if entry.line.collapsible() && entry.expanded != expanded {
                entry.expanded = expanded;
                changed = true;
            }
        }
        if changed {
            self.epoch += 1;
        }
        changed
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

    #[test]
    fn a_latest_page_above_everything_held_replaces_the_buffer_rather_than_leaving_a_hole() {
        let mut store = loaded();
        assert_eq!(store.oldest_seq(), Some(10));

        // Away long enough that the newest page starts far above seq 12.
        store.merge(PageKind::Latest, rows(&[(501, "x"), (502, "y")]), None, true);

        assert_eq!(texts(&store), ["x", "y"], "the stale half is dropped, not interleaved");
        assert_eq!(
            store.begin_older().map(|(page, _)| page),
            Some(PageRequest::before(501, PAGE_LIMIT)),
            "older paging walks back from the new page, so 13..500 is reachable"
        );
    }

    /// R6: the busy session. A live event above the gap used to make the page
    /// look contiguous, so the hole below it survived every refetch.
    #[test]
    fn a_live_row_above_the_gap_does_not_make_a_stale_page_look_contiguous() {
        let mut store = loaded();
        // The session kept streaming in the background and one event landed
        // live, above everything the next page will return.
        store.push_live(Some(701), line("live while away"));
        assert_eq!(store.newest_seq(), Some(701));

        store.merge(PageKind::Latest, rows(&[(521, "p1"), (522, "p2")]), None, true);

        assert_eq!(
            texts(&store),
            ["p1", "p2", "live while away"],
            "the unreachable history goes; the live row above the page stays"
        );
        assert_eq!(
            store.begin_older().map(|(page, _)| page),
            Some(PageRequest::before(521, PAGE_LIMIT)),
            "older paging now walks back from the page, so 13..520 is reachable"
        );
    }

    /// The same shape, with the live row arriving between the open and the page
    /// reply on a conversation whose history is still contiguous with it.
    #[test]
    fn a_live_row_does_not_trigger_a_reset_when_the_page_joins_the_history() {
        let mut store = loaded();
        store.push_live(Some(13), line("live"));
        store.merge(PageKind::Latest, rows(&[(11, "b"), (12, "c"), (13, "live")]), None, true);
        assert_eq!(texts(&store), ["a", "b", "c", "live"], "nothing is dropped");
    }

    /// R6, second half: the older page claimed before the reset must not land
    /// back under the new window.
    #[test]
    fn an_older_page_claimed_before_a_reset_is_dropped() {
        let mut store = loaded();
        let (claimed, stale_claim) = store.begin_older().expect("a claim");
        assert_eq!(claimed, PageRequest::before(10, PAGE_LIMIT));

        store.merge(PageKind::Latest, rows(&[(501, "x")]), None, true);
        // The reply to the pre-reset claim arrives now.
        store.merge_page(
            PageKind::Older,
            Some(stale_claim),
            rows(&[(8, "stale"), (9, "stale-2")]),
            None,
            true,
        );

        assert_eq!(texts(&store), ["x"], "the stale rows are refused");
        assert_eq!(
            store.begin_older().map(|(page, _)| page),
            Some(PageRequest::before(501, PAGE_LIMIT)),
            "and the window is still the new one"
        );
    }

    /// N5: the reply to a request the store abandoned must stay refused even
    /// once a *new* older page is outstanding — the stamp identifies the
    /// request, so a fresh claim cannot vouch for a stale reply.
    #[test]
    fn a_stale_older_reply_is_still_refused_after_a_re_claim() {
        let mut store = loaded();
        let (stale_request, stale_claim) = store.begin_older().expect("a claim");
        assert_eq!(stale_request, PageRequest::before(10, PAGE_LIMIT));

        // Leave and reopen: the newest page is far above, so history is reset.
        store.merge(PageKind::Latest, rows(&[(301, "p1"), (302, "p2")]), None, true);
        // The user scrolls to the top again before the slow reply returns.
        let (fresh_request, fresh_claim) = store.begin_older().expect("a fresh claim");
        assert_eq!(fresh_request, PageRequest::before(301, PAGE_LIMIT));
        assert_ne!(stale_claim, fresh_claim, "each request has its own stamp");

        // Now the pre-reset reply lands.
        store.merge_page(
            PageKind::Older,
            Some(stale_claim),
            rows(&[(8, "stale"), (9, "stale-2")]),
            None,
            true,
        );

        assert_eq!(texts(&store), ["p1", "p2"], "the abandoned window's rows are refused");
        assert_eq!(store.oldest_seq(), Some(301), "so no hole opens below the new window");

        // And the request that is genuinely outstanding still lands.
        store.merge_page(
            PageKind::Older,
            Some(fresh_claim),
            rows(&[(299, "real"), (300, "real-2")]),
            None,
            true,
        );
        assert_eq!(texts(&store), ["real", "real-2", "p1", "p2"]);
    }

    /// Same shape for the failure path: a stale request failing must not
    /// release the claim of the one that replaced it.
    #[test]
    fn a_stale_older_failure_does_not_release_the_live_claim() {
        let mut store = loaded();
        let (_, stale_claim) = store.begin_older().expect("a claim");
        store.merge(PageKind::Latest, rows(&[(301, "p1")]), None, true);
        let (_, fresh_claim) = store.begin_older().expect("a fresh claim");

        store.page_failed(PageKind::Older, Some(stale_claim));

        assert!(
            store.begin_older().is_none(),
            "the live request is still in flight, so nothing may be claimed over it"
        );
        store.merge_page(PageKind::Older, Some(fresh_claim), rows(&[(300, "real")]), None, false);
        assert_eq!(texts(&store), ["real", "p1"]);
    }

    /// A 304 for an abandoned request must not retire the live one either.
    #[test]
    fn a_stale_not_modified_leaves_the_live_claim_outstanding() {
        let mut store = loaded();
        let (_, stale_claim) = store.begin_older().expect("a claim");
        store.merge(PageKind::Latest, rows(&[(301, "p1")]), None, true);
        let (_, fresh_claim) = store.begin_older().expect("a fresh claim");

        store.page_not_modified(PageKind::Older, Some(stale_claim));
        assert!(store.begin_older().is_none(), "still in flight");

        store.merge_page(PageKind::Older, Some(fresh_claim), rows(&[(300, "real")]), None, false);
        assert_eq!(texts(&store), ["real", "p1"]);
    }

    #[test]
    fn an_older_page_claimed_after_a_reset_still_lands() {
        let mut store = loaded();
        store.merge(PageKind::Latest, rows(&[(501, "x")]), None, true);
        store.begin_older().expect("a fresh claim");
        store.merge(PageKind::Older, rows(&[(499, "older"), (500, "older-2")]), None, false);
        assert_eq!(texts(&store), ["older", "older-2", "x"]);
    }

    #[test]
    fn a_transcript_is_capped_and_what_was_dropped_can_be_paged_back() {
        let mut store = ConversationStore::new();
        store.merge(PageKind::Latest, rows(&[(1, "first")]), None, false);
        assert!(!store.has_more_older, "the whole conversation is held");

        let cap = i64::try_from(super::MAX_ENTRIES).expect("the cap fits");
        for seq in 2..=(cap + 101) {
            store.push_live(Some(seq), line("chatter"));
        }
        let trimmed = store.trim_to_cap();

        assert_eq!(trimmed, 101, "only the overflow goes");
        assert_eq!(store.len(), super::MAX_ENTRIES);
        assert_eq!(store.oldest_seq(), Some(102), "the oldest rows were dropped");
        assert!(store.has_more_older, "and paging back can fetch them again");

        // The dropped seqs must not be deduped away when they are refetched.
        store.begin_older().expect("a claim");
        let merge =
            store.merge(PageKind::Older, rows(&[(100, "back"), (101, "back-2")]), None, true);
        assert_eq!(merge.inserted, 2, "a refetched row is not mistaken for a duplicate");
        assert_eq!(store.oldest_seq(), Some(100));
    }

    #[test]
    fn trimming_is_a_no_op_under_the_cap() {
        let mut store = loaded();
        assert_eq!(store.trim_to_cap(), 0);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn a_latest_page_that_touches_what_is_held_merges_as_before() {
        let mut store = loaded();
        store.merge(PageKind::Latest, rows(&[(13, "d"), (14, "e")]), None, true);
        assert_eq!(texts(&store), ["a", "b", "c", "d", "e"], "contiguous pages still merge");

        let mut overlapping = loaded();
        overlapping.merge(PageKind::Latest, rows(&[(12, "c"), (13, "d")]), None, true);
        assert_eq!(texts(&overlapping), ["a", "b", "c", "d"]);
    }

    #[test]
    fn eviction_drops_the_coldest_transcripts_and_spares_the_ones_in_use() {
        use std::collections::{HashMap, HashSet};

        let mut stores: HashMap<String, ConversationStore> = HashMap::new();
        for i in 0..6 {
            let mut store = ConversationStore::new();
            store.touched = i;
            stores.insert(format!("s-{i}"), store);
        }
        let keep = HashSet::from(["s-0".to_owned()]);

        let dropped = super::evict_cold(&mut stores, &keep, 4);

        assert_eq!(dropped, ["s-1", "s-2"], "coldest first, s-0 spared despite being coldest");
        assert_eq!(stores.len(), 4);
        assert!(stores.contains_key("s-0"), "an in-use transcript is never dropped");
        assert!(stores.contains_key("s-5"));
    }

    #[test]
    fn eviction_is_a_no_op_under_the_cap() {
        use std::collections::{HashMap, HashSet};

        let mut stores: HashMap<String, ConversationStore> = HashMap::new();
        stores.insert("s-a".to_owned(), ConversationStore::new());
        assert!(super::evict_cold(&mut stores, &HashSet::new(), 4).is_empty());
        assert_eq!(stores.len(), 1);
    }

    fn loaded() -> ConversationStore {
        let mut store = ConversationStore::new();
        store.merge(
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
        let merge =
            store.merge(PageKind::Latest, rows(&[(12, "c"), (10, "a"), (11, "b")]), None, false);
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

        let merge = store.merge(PageKind::Gap, rows(&[(11, "b"), (13, "d")]), None, false);
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

        store.merge(PageKind::Latest, rows(&[(4, "history")]), None, false);
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
        store.merge(PageKind::Gap, rows(&[(13, "d"), (14, "e")]), None, false);
        assert!(store.has_more_older, "a gap page says nothing about older rows");
        assert_eq!(texts(&store), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn older_paging_walks_back_and_stops_at_the_start() {
        let mut store = loaded();
        let (request, _) = store.begin_older().expect("an older page is available");
        assert_eq!(request, PageRequest::before(10, PAGE_LIMIT));

        assert!(store.begin_older().is_none(), "a fetch is already in flight");

        let merge =
            store.merge(PageKind::Older, rows(&[(8, "older-1"), (9, "older-2")]), None, false);
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
        store.merge(PageKind::Older, rows(&[(9, "older")]), None, true);
        assert!(store.has_more_older);
        assert_eq!(
            store.begin_older().map(|(page, _)| page),
            Some(PageRequest::before(9, PAGE_LIMIT))
        );
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
        store.failed(PageKind::Older);
        assert_eq!(
            store.begin_older().map(|(page, _)| page),
            Some(PageRequest::before(10, PAGE_LIMIT))
        );
    }

    #[test]
    fn not_modified_keeps_the_etag_and_the_entries() {
        let mut store = loaded();
        store.not_modified(PageKind::Latest);
        assert_eq!(store.etag(), Some("etag-1"));
        assert_eq!(texts(&store), ["a", "b", "c"]);
        assert!(store.loaded);

        store.begin_older().expect("a request");
        store.not_modified(PageKind::Older);
        assert!(!store.loading_older);
        assert!(store.has_more_older, "a 304 must not claim the transcript ended");
    }

    #[test]
    fn a_refetched_latest_page_replaces_the_etag_and_keeps_the_history() {
        let mut store = loaded();
        store.begin_older().expect("a request");
        store.merge(PageKind::Older, rows(&[(9, "older")]), None, false);
        assert!(!store.has_more_older);

        store.merge(
            PageKind::Latest,
            rows(&[(12, "c"), (13, "d")]),
            Some("etag-2".to_owned()),
            false,
        );
        assert_eq!(store.etag(), Some("etag-2"));
        assert!(!store.has_more_older, "a later page already proved the start was reached");
        assert_eq!(texts(&store), ["older", "a", "b", "c", "d"]);
    }

    fn call(id: &str, tool: &str) -> ConversationLine {
        let mut ln = ConversationLine::new(
            LineKind::Tool { category: crate::app::state::ToolCategory::Other },
            tool,
            0,
        );
        ln.tool = Some(tool.to_owned());
        ln.tool_use_id = Some(id.to_owned());
        ln
    }

    fn result(id: Option<&str>, text: &str) -> ConversationLine {
        let mut ln = ConversationLine::new(LineKind::Result { error: false }, text, 0);
        ln.tool_use_id = id.map(str::to_owned);
        ln
    }

    fn answers(store: &ConversationStore) -> Vec<Option<String>> {
        store.entries().iter().map(|e| e.line.answers.clone()).collect()
    }

    #[test]
    fn interleaved_parallel_results_name_the_call_they_answer() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), call("a", "Read"));
        store.push_live(Some(2), call("b", "Bash"));
        store.push_live(Some(3), result(Some("a"), "file"));
        store.push_live(Some(4), result(Some("b"), "ok"));
        assert_eq!(answers(&store), [None, None, Some("Read".to_owned()), Some("Bash".to_owned())]);
    }

    #[test]
    fn a_result_right_under_its_call_needs_no_label() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), call("a", "Read"));
        store.push_live(Some(2), result(Some("a"), "file"));
        assert_eq!(answers(&store), [None, None]);
    }

    #[test]
    fn an_out_of_order_page_still_pairs_by_id() {
        let mut store = ConversationStore::new();
        store.merge(
            PageKind::Latest,
            vec![
                (4, result(Some("a"), "file")),
                (3, result(Some("b"), "ok")),
                (2, call("b", "Bash")),
                (1, call("a", "Read")),
            ],
            None,
            false,
        );
        assert_eq!(answers(&store), [None, None, None, Some("Read".to_owned())]);
    }

    #[test]
    fn a_result_without_an_id_falls_back_to_position() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), call("a", "Read"));
        store.push_live(Some(2), call("b", "Bash"));
        store.push_live(Some(3), result(None, "ok"));
        assert_eq!(answers(&store), [None, None, None]);
    }

    #[test]
    fn expanding_a_line_invalidates_the_render_cache() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), line("prose"));
        let out = ConversationLine::new(LineKind::Result { error: false }, "out", 0);
        store.push_live(Some(2), out);
        let epoch = store.epoch();

        assert!(!store.toggle_expanded(0), "an assistant line has nothing to collapse");
        assert_eq!(store.epoch(), epoch);

        assert!(store.toggle_expanded(1));
        assert!(store.entries()[1].expanded);
        assert!(store.epoch() > epoch, "the cached row count changed");

        assert!(store.toggle_expanded(1));
        assert!(!store.entries()[1].expanded);
        assert!(!store.toggle_expanded(9), "an index past the end does nothing");
    }

    #[test]
    fn a_bulk_toggle_only_moves_collapsible_lines() {
        let mut store = ConversationStore::new();
        store.push_live(Some(1), line("prose"));
        store.push_live(
            Some(2),
            ConversationLine::new(LineKind::Thinking { redacted: false }, "hmm", 0),
        );
        assert!(store.set_all_expanded(true));
        assert!(!store.entries()[0].expanded);
        assert!(store.entries()[1].expanded);
        assert!(!store.set_all_expanded(true), "a repeat toggle changes nothing");
        assert!(store.set_all_expanded(false));
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
