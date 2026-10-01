//! Which transcript lines are on screen. The category set and the quick-filter
//! groups mirror the webui's `conversation/filters.ts` and must stay in step.

use std::collections::BTreeSet;

use super::state::{ConversationLine, LineKind, ToolCategory};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Assistant,
    Thinking,
    Redacted,
    User,
    /// A monitor wake-up or a re-injected loop prompt. Only the export
    /// classifies one today; no rendered line maps here.
    Poll,
    Peer,
    System,
    Tool,
    Mcp,
    ServerTool,
    Result,
    Error,
    Marker,
    Summary,
    Compact,
    Reset,
}

/// In the webui's group order, which is what the `F` menu lists.
pub const CATEGORIES: &[Category] = &[
    Category::Assistant,
    Category::Thinking,
    Category::Redacted,
    Category::User,
    Category::Poll,
    Category::Peer,
    Category::System,
    Category::Tool,
    Category::Mcp,
    Category::ServerTool,
    Category::Result,
    Category::Error,
    Category::Marker,
    Category::Summary,
    Category::Compact,
    Category::Reset,
];

/// Hidden until asked for, as in the webui: `mcp` is noisy and a marker is
/// bookkeeping.
const HIDDEN_BY_DEFAULT: &[Category] = &[Category::Mcp, Category::Marker];

impl Category {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Thinking => "thinking",
            Self::Redacted => "redacted",
            Self::User => "user",
            Self::Poll => "poll",
            Self::Peer => "peer",
            Self::System => "system",
            Self::Tool => "tool",
            Self::Mcp => "mcp",
            Self::ServerTool => "server-tool",
            Self::Result => "result",
            Self::Error => "error",
            Self::Marker => "marker",
            Self::Summary => "summary",
            Self::Compact => "compact",
            Self::Reset => "reset",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Assistant => "Assistant prose",
            Self::Thinking => "Thinking",
            Self::Redacted => "Redacted thinking",
            Self::User => "Your messages",
            Self::Poll => "Loop wake-ups",
            Self::Peer => "Peer messages",
            Self::System => "Injected / system",
            Self::Tool => "Tool calls",
            Self::Mcp => "MCP tool calls",
            Self::ServerTool => "Provider tool calls",
            Self::Result => "Tool results",
            Self::Error => "Failed results",
            Self::Marker => "Markers",
            Self::Summary => "Turn footers",
            Self::Compact => "Compaction",
            Self::Reset => "Context resets",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        CATEGORIES.iter().copied().find(|c| c.as_str() == text)
    }
}

#[must_use]
pub const fn category_of(line: &ConversationLine) -> Category {
    match line.kind {
        LineKind::Assistant | LineKind::Image => Category::Assistant,
        LineKind::Thinking { redacted: false } => Category::Thinking,
        LineKind::Thinking { redacted: true } => Category::Redacted,
        LineKind::User | LineKind::Reply => Category::User,
        LineKind::Peer => Category::Peer,
        LineKind::System => Category::System,
        LineKind::Tool { category: ToolCategory::Mcp } => Category::Mcp,
        LineKind::Tool { category: ToolCategory::Server } => Category::ServerTool,
        LineKind::Tool { .. } => Category::Tool,
        LineKind::Result { error: false } => Category::Result,
        LineKind::Result { error: true } => Category::Error,
        LineKind::Marker => Category::Marker,
        LineKind::Summary => Category::Summary,
        LineKind::Compact => Category::Compact,
        LineKind::Reset => Category::Reset,
    }
}

/// What `f` steps through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Quick {
    #[default]
    All,
    Assistant,
    User,
    Tools,
}

const QUICK_CYCLE: &[Quick] = &[Quick::All, Quick::Assistant, Quick::User, Quick::Tools];

const TOOL_CATEGORIES: &[Category] =
    &[Category::Tool, Category::Mcp, Category::ServerTool, Category::Result, Category::Error];

impl Quick {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Assistant => "assistant",
            Self::User => "user",
            Self::Tools => "tools",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        QUICK_CYCLE.iter().copied().find(|q| q.as_str() == text)
    }

    #[must_use]
    pub fn next(self) -> Self {
        let at = QUICK_CYCLE.iter().position(|q| *q == self).unwrap_or(0);
        QUICK_CYCLE[(at + 1) % QUICK_CYCLE.len()]
    }

    /// `None` for [`Quick::All`]: every category passes.
    #[must_use]
    pub const fn categories(self) -> Option<&'static [Category]> {
        match self {
            Self::All => None,
            Self::Assistant => Some(&[Category::Assistant]),
            Self::User => Some(&[Category::User]),
            Self::Tools => Some(TOOL_CATEGORIES),
        }
    }
}

/// A quick filter narrows to one group; the hidden set is the `F` menu's own
/// state and applies on top of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub quick: Quick,
    hidden: BTreeSet<Category>,
}

impl Default for Filter {
    fn default() -> Self {
        Self { quick: Quick::default(), hidden: HIDDEN_BY_DEFAULT.iter().copied().collect() }
    }
}

impl Filter {
    #[must_use]
    pub fn visible(&self, line: &ConversationLine) -> bool {
        self.shows(category_of(line))
    }

    #[must_use]
    pub fn shows(&self, category: Category) -> bool {
        if self.hidden.contains(&category) {
            return false;
        }
        self.quick.categories().is_none_or(|allowed| allowed.contains(&category))
    }

    /// True while anything at all is being held back, so the view can say so.
    #[must_use]
    pub fn is_narrowed(&self) -> bool {
        self.quick != Quick::All || *self != Self::default()
    }

    pub fn cycle(&mut self) {
        self.quick = self.quick.next();
    }

    pub fn toggle(&mut self, category: Category) {
        if !self.hidden.remove(&category) {
            self.hidden.insert(category);
        }
    }

    /// Clears the quick filter and every per-category override.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Shows everything, including what is hidden by default.
    pub fn show_all(&mut self) {
        self.quick = Quick::All;
        self.hidden.clear();
    }

    /// Identifies the filter for the render cache: a change must rebuild, and
    /// the set is small enough that the name list is cheaper than a hash.
    #[must_use]
    pub fn cache_key(&self) -> String {
        let mut key = self.quick.as_str().to_owned();
        for category in &self.hidden {
            key.push('|');
            key.push_str(category.as_str());
        }
        key
    }

    #[must_use]
    pub fn hidden_names(&self) -> Vec<String> {
        self.hidden.iter().map(|c| c.as_str().to_owned()).collect()
    }

    #[must_use]
    pub fn from_persisted(quick: &str, hidden: &[String]) -> Self {
        Self {
            quick: Quick::parse(quick).unwrap_or_default(),
            hidden: hidden.iter().filter_map(|h| Category::parse(h)).collect(),
        }
    }

    /// One line for the header: what is being held back, or nothing at all.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        if !self.is_narrowed() {
            return None;
        }
        let hidden = self.hidden.len();
        match (self.quick, hidden) {
            (Quick::All, 0) => None,
            (Quick::All, n) => Some(format!("{n} hidden")),
            (quick, 0) => Some(quick.as_str().to_owned()),
            (quick, n) => Some(format!("{} · {n} hidden", quick.as_str())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CATEGORIES, Category, Filter, Quick, category_of};
    use crate::app::state::{ConversationLine, LineKind, LineStatus, ToolCategory};

    fn line(kind: LineKind) -> ConversationLine {
        ConversationLine::new(kind, "body", 0)
    }

    #[test]
    fn every_line_kind_maps_to_a_category_in_the_menu() {
        let kinds = [
            LineKind::Assistant,
            LineKind::Thinking { redacted: false },
            LineKind::Thinking { redacted: true },
            LineKind::User,
            LineKind::Reply,
            LineKind::Peer,
            LineKind::System,
            LineKind::Tool { category: ToolCategory::Read },
            LineKind::Tool { category: ToolCategory::Write },
            LineKind::Tool { category: ToolCategory::Other },
            LineKind::Tool { category: ToolCategory::Mcp },
            LineKind::Tool { category: ToolCategory::Server },
            LineKind::Result { error: false },
            LineKind::Result { error: true },
            LineKind::Marker,
            LineKind::Summary,
            LineKind::Compact,
            LineKind::Reset,
        ];
        for kind in kinds {
            let category = category_of(&line(kind));
            assert!(CATEGORIES.contains(&category), "{category:?} is missing from the menu");
        }
        assert_eq!(category_of(&line(LineKind::Reply)), Category::User);
        assert_eq!(
            category_of(&line(LineKind::Tool { category: ToolCategory::Mcp })),
            Category::Mcp
        );
        assert_eq!(category_of(&line(LineKind::Result { error: true })), Category::Error);
    }

    #[test]
    fn the_default_filter_hides_only_mcp_and_markers() {
        let filter = Filter::default();
        assert!(filter.shows(Category::Assistant));
        assert!(filter.shows(Category::Result));
        assert!(!filter.shows(Category::Mcp));
        assert!(!filter.shows(Category::Marker));
        assert!(!filter.is_narrowed(), "the default is not a narrowing");
        assert!(filter.summary().is_none());
    }

    #[test]
    fn the_quick_cycle_returns_to_everything() {
        let mut filter = Filter::default();
        filter.cycle();
        assert_eq!(filter.quick, Quick::Assistant);
        assert!(filter.shows(Category::Assistant));
        assert!(!filter.shows(Category::User));
        assert!(!filter.shows(Category::Tool));

        filter.cycle();
        assert_eq!(filter.quick, Quick::User);
        assert!(filter.shows(Category::User));
        assert!(!filter.shows(Category::Assistant));

        filter.cycle();
        assert_eq!(filter.quick, Quick::Tools);
        assert!(filter.shows(Category::Tool));
        assert!(filter.shows(Category::Error));
        assert!(!filter.shows(Category::Mcp), "the hidden set still applies on top");

        filter.cycle();
        assert_eq!(filter.quick, Quick::All);
        assert!(filter.shows(Category::Assistant));
    }

    #[test]
    fn a_reply_rides_along_with_the_user_quick_filter() {
        let mut filter = Filter::default();
        filter.cycle();
        filter.cycle();
        assert_eq!(filter.quick, Quick::User);
        assert!(filter.visible(&line(LineKind::Reply)));
        assert!(filter.visible(&line(LineKind::User)));
    }

    #[test]
    fn the_menu_toggles_one_category_at_a_time() {
        let mut filter = Filter::default();
        filter.toggle(Category::Assistant);
        assert!(!filter.shows(Category::Assistant));
        assert!(filter.is_narrowed());
        filter.toggle(Category::Assistant);
        assert!(filter.shows(Category::Assistant));

        filter.toggle(Category::Mcp);
        assert!(filter.shows(Category::Mcp), "toggling a default-hidden category shows it");
    }

    #[test]
    fn show_all_reveals_what_the_default_hides_and_reset_puts_it_back() {
        let mut filter = Filter::default();
        filter.show_all();
        assert!(filter.shows(Category::Mcp));
        assert!(filter.shows(Category::Marker));
        assert!(filter.is_narrowed(), "showing more than the default is still a deviation");

        filter.reset();
        assert_eq!(filter, Filter::default());
        assert!(!filter.is_narrowed());
    }

    #[test]
    fn the_cache_key_changes_with_every_visible_difference() {
        let mut filter = Filter::default();
        let base = filter.cache_key();
        filter.cycle();
        assert_ne!(filter.cache_key(), base);
        let quick_only = filter.cache_key();
        filter.toggle(Category::Assistant);
        assert_ne!(filter.cache_key(), quick_only);
    }

    #[test]
    fn a_persisted_filter_round_trips_and_ignores_junk() {
        let mut filter = Filter::default();
        filter.cycle();
        filter.toggle(Category::Summary);
        let restored = Filter::from_persisted(filter.quick.as_str(), &filter.hidden_names());
        assert_eq!(restored, filter);

        let junk = Filter::from_persisted("nonsense", &["not-a-category".to_owned()]);
        assert_eq!(junk.quick, Quick::All);
        assert!(junk.shows(Category::Mcp), "an unreadable name drops out of the hidden set");
    }

    #[test]
    fn the_summary_names_what_is_holding_lines_back() {
        let mut filter = Filter::default();
        filter.cycle();
        assert_eq!(filter.summary().as_deref(), Some("assistant · 2 hidden"));

        let mut only_hidden = Filter::default();
        only_hidden.toggle(Category::Summary);
        assert_eq!(only_hidden.summary().as_deref(), Some("3 hidden"));

        let mut quick_only = Filter::default();
        quick_only.show_all();
        quick_only.cycle();
        assert_eq!(quick_only.summary().as_deref(), Some("assistant"));
    }

    #[test]
    fn a_queued_line_is_still_one_of_the_users_own() {
        let queued =
            ConversationLine::new(LineKind::User, "later", 0).with_status(LineStatus::Queued);
        assert_eq!(category_of(&queued), Category::User);
    }
}
