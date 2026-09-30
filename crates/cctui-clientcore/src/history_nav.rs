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
