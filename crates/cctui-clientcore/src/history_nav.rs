/// How many sent prompts one session keeps for recall.
pub const SESSION_HISTORY_MAX: usize = 5;

/// Decode a stored history blob: a JSON array of strings, most-recent-last.
///
/// Anything else reads as no history rather than as an error.
#[must_use]
pub fn parse_history(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<serde_json::Value>>(raw).map_or_else(
        |_| Vec::new(),
        |values| values.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
    )
}

/// The stored form of a history list.
#[must_use]
pub fn encode_history(list: &[String]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".to_owned())
}

/// Record `value` as the newest entry: trimmed, de-duped against the older
/// entries, and capped at `max` by dropping the oldest. Blank is not history.
pub fn push_history(list: &mut Vec<String>, value: &str, max: usize) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    list.retain(|entry| entry != value);
    list.push(value.to_owned());
    if list.len() > max {
        list.drain(..list.len() - max);
    }
}

/// `ArrowUp`/`ArrowDown` recall over a prompt history, shared by the conversation
/// composer and the spawn form.
///
/// `index` is `None` while editing the live draft; `Some(0..n-1)` browses
/// newest-first. Recall only starts with the caret at the very start (and ends
/// at the very end) so it never fights normal multiline cursor movement.
#[derive(Clone, Debug)]
pub struct HistoryNav {
    index: Option<usize>,
    stash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOutcome {
    pub handled: bool,
    pub value: Option<String>,
}

impl Default for HistoryNav {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryNav {
    #[must_use]
    pub const fn new() -> Self {
        Self { index: None, stash: String::new() }
    }

    #[must_use]
    pub const fn browsing(&self) -> bool {
        self.index.is_some()
    }

    pub const fn reset(&mut self) {
        self.index = None;
    }

    pub fn reset_all(&mut self) {
        self.index = None;
        self.stash = String::new();
    }

    pub fn back(&mut self, list: &[String], value: &str) -> Option<String> {
        if list.is_empty() {
            return None;
        }
        let next = match self.index {
            None => {
                self.stash = value.to_string();
                0
            }
            Some(i) => (i + 1).min(list.len() - 1),
        };
        self.index = Some(next);
        Some(list[list.len() - 1 - next].clone())
    }

    pub fn forward(&mut self, list: &[String]) -> Option<String> {
        let current = self.index?;
        let Some(next) = current.checked_sub(1) else {
            self.index = None;
            return Some(self.stash.clone());
        };
        self.index = Some(next);
        Some(list[list.len() - 1 - next].clone())
    }

    /// Jump straight to an entry (menu pick), stashing the live draft first.
    pub fn recall(&mut self, list: &[String], value: &str, pick: &str) -> String {
        if self.index.is_none() {
            self.stash = value.to_string();
        }
        if let Some(at) = list.iter().rposition(|e| e == pick) {
            self.index = Some(list.len() - 1 - at);
        }
        pick.to_string()
    }

    pub fn handle_key(
        &mut self,
        key: &str,
        list: &[String],
        value: &str,
        caret_start: usize,
        caret_end: usize,
    ) -> KeyOutcome {
        let at_start = caret_start == 0 && caret_end == 0;
        let len = value.chars().count();
        let at_end = caret_start == len && caret_end == len;
        if key == "ArrowUp" && (self.browsing() || at_start) {
            return KeyOutcome { handled: true, value: self.back(list, value) };
        }
        if key == "ArrowDown" && self.browsing() && at_end {
            return KeyOutcome { handled: true, value: self.forward(list) };
        }
        KeyOutcome { handled: false, value: None }
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryNav, encode_history, parse_history, push_history};

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| (*e).to_owned()).collect()
    }

    #[test]
    fn a_history_blob_round_trips_and_junk_reads_as_empty() {
        let stored = encode_history(&list(&["one", "two"]));
        assert_eq!(parse_history(&stored), list(&["one", "two"]));
        assert_eq!(encode_history(&[]), "[]");
        for junk in ["", "not json", "{}", "\"one\"", "[1, 2]"] {
            assert!(parse_history(junk).is_empty(), "{junk} should read as no history");
        }
        assert_eq!(parse_history("[\"keep\", 7, null]"), list(&["keep"]));
    }

    #[test]
    fn pushing_trims_dedupes_and_caps() {
        let mut entries = Vec::new();
        push_history(&mut entries, "  ", 3);
        push_history(&mut entries, "", 3);
        assert!(entries.is_empty(), "blank is not history");

        push_history(&mut entries, "  first  ", 3);
        assert_eq!(entries, list(&["first"]));

        push_history(&mut entries, "second", 3);
        push_history(&mut entries, "first", 3);
        assert_eq!(entries, list(&["second", "first"]), "a repeat moves to the newest slot");

        push_history(&mut entries, "third", 3);
        push_history(&mut entries, "fourth", 3);
        assert_eq!(entries, list(&["first", "third", "fourth"]), "the oldest is dropped");
    }

    #[test]
    fn recall_walks_newest_first_and_returns_to_the_live_draft() {
        let entries = list(&["oldest", "middle", "newest"]);
        let mut nav = HistoryNav::new();
        assert!(!nav.browsing());

        assert_eq!(nav.back(&entries, "typing"), Some("newest".to_owned()));
        assert_eq!(nav.back(&entries, "newest"), Some("middle".to_owned()));
        assert_eq!(nav.back(&entries, "middle"), Some("oldest".to_owned()));
        assert_eq!(nav.back(&entries, "oldest"), Some("oldest".to_owned()), "clamped at the top");

        assert_eq!(nav.forward(&entries), Some("middle".to_owned()));
        assert_eq!(nav.forward(&entries), Some("newest".to_owned()));
        assert_eq!(nav.forward(&entries), Some("typing".to_owned()), "the stash comes back");
        assert!(!nav.browsing());
        assert_eq!(nav.forward(&entries), None, "not browsing: nothing to walk back to");
    }

    #[test]
    fn an_empty_history_never_starts_a_walk() {
        let mut nav = HistoryNav::new();
        assert_eq!(nav.back(&[], "typing"), None);
        assert!(!nav.browsing());
    }

    #[test]
    fn a_caret_inside_the_text_keeps_the_arrow_keys() {
        let entries = list(&["recalled"]);
        let mut nav = HistoryNav::new();

        let mid = nav.handle_key("ArrowUp", &entries, "two\nlines", 4, 4);
        assert!(!mid.handled, "the caret is not at the start: the textarea moves the cursor");

        let start = nav.handle_key("ArrowUp", &entries, "two\nlines", 0, 0);
        assert!(start.handled);
        assert_eq!(start.value, Some("recalled".to_owned()));

        let browsing = nav.handle_key("ArrowUp", &entries, "recalled", 3, 3);
        assert!(browsing.handled, "once browsing, the caret no longer matters");

        let not_at_end = nav.handle_key("ArrowDown", &entries, "recalled", 2, 2);
        assert!(!not_at_end.handled);
        let at_end = nav.handle_key("ArrowDown", &entries, "recalled", 8, 8);
        assert!(at_end.handled);
        assert_eq!(at_end.value, Some("two\nlines".to_owned()));
    }

    #[test]
    fn a_selection_is_not_a_caret_at_the_boundary() {
        let entries = list(&["recalled"]);
        let mut nav = HistoryNav::new();
        assert!(!nav.handle_key("ArrowUp", &entries, "picked", 0, 6).handled);
    }

    #[test]
    fn an_unrelated_key_is_never_consumed() {
        let mut nav = HistoryNav::new();
        let outcome = nav.handle_key("Enter", &list(&["one"]), "", 0, 0);
        assert!(!outcome.handled);
        assert_eq!(outcome.value, None);
    }

    #[test]
    fn a_menu_pick_stashes_the_draft_and_resumes_the_walk_from_it() {
        let entries = list(&["oldest", "middle", "newest"]);
        let mut nav = HistoryNav::new();
        assert_eq!(nav.recall(&entries, "typing", "middle"), "middle");
        assert!(nav.browsing());
        assert_eq!(nav.back(&entries, "middle"), Some("oldest".to_owned()));
        assert_eq!(nav.forward(&entries), Some("middle".to_owned()));
        assert_eq!(nav.forward(&entries), Some("newest".to_owned()));
        assert_eq!(nav.forward(&entries), Some("typing".to_owned()));
    }

    #[test]
    fn a_pick_absent_from_the_history_still_lands_in_the_composer() {
        let entries = list(&["one"]);
        let mut nav = HistoryNav::new();
        assert_eq!(nav.recall(&entries, "typing", "elsewhere"), "elsewhere");
        assert!(!nav.browsing(), "an unknown pick leaves the index alone");
    }

    #[test]
    fn resetting_keeps_the_stash_and_reset_all_drops_it() {
        let entries = list(&["one", "two"]);
        let mut nav = HistoryNav::new();
        let _ = nav.back(&entries, "typing");
        nav.reset();
        assert!(!nav.browsing());
        let _ = nav.back(&entries, "two");
        assert_eq!(nav.forward(&entries), Some("two".to_owned()), "the stash was the walk's start");

        nav.reset_all();
        let _ = nav.back(&entries, "two");
        assert_eq!(nav.forward(&entries), Some("two".to_owned()));
    }
}
