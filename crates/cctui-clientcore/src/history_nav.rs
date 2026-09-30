/// ArrowUp/ArrowDown recall over a prompt history, shared by the conversation
/// composer and the spawn form.
///
/// Index -1 means editing the live draft; 0..n-1 browses newest-first. Recall
/// only starts with the caret at the very start (and ends at the very end) so
/// it never fights normal multiline cursor movement.
#[derive(Clone, Debug)]
pub struct HistoryNav {
    index: i64,
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
        Self { index: -1, stash: String::new() }
    }

    #[must_use]
    pub const fn browsing(&self) -> bool {
        self.index != -1
    }

    pub const fn reset(&mut self) {
        self.index = -1;
    }

    pub fn reset_all(&mut self) {
        self.index = -1;
        self.stash = String::new();
    }

    pub fn back(&mut self, list: &[String], value: &str) -> Option<String> {
        if list.is_empty() {
            return None;
        }
        if self.index == -1 {
            self.stash = value.to_string();
        }
        self.index = (self.index + 1).min(list.len() as i64 - 1);
        Some(list[list.len() - 1 - self.index as usize].clone())
    }

    pub fn forward(&mut self, list: &[String]) -> Option<String> {
        if self.index == -1 {
            return None;
        }
        let next = self.index - 1;
        if next < 0 {
            self.index = -1;
            return Some(self.stash.clone());
        }
        self.index = next;
        Some(list[list.len() - 1 - next as usize].clone())
    }

    /// Jump straight to an entry (menu pick), stashing the live draft first.
    pub fn recall(&mut self, list: &[String], value: &str, pick: &str) -> String {
        if self.index == -1 {
            self.stash = value.to_string();
        }
        if let Some(at) = list.iter().rposition(|e| e == pick) {
            self.index = (list.len() - 1 - at) as i64;
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
