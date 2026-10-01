//! Which sections the list shows, how rows are sorted and what they group by.
//!
//! Ported from the webui's `routes/sessions/sessions.logic.ts` and persisted to
//! the same `sessionList` settings blob, so the two front ends agree.

use std::collections::BTreeSet;

use cctui_proto::api::SessionListItem;

use super::session_list::{group_of, is_dispatched};

/// An ownership bucket the list can show, plus `Unread`, which narrows whatever
/// the others let through rather than owning rows of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Starred,
    Live,
    Dispatched,
    Drafts,
    Archived,
    Unread,
}

pub const SECTIONS: &[Section] = &[
    Section::Starred,
    Section::Live,
    Section::Dispatched,
    Section::Drafts,
    Section::Archived,
    Section::Unread,
];

pub const DEFAULT_SECTIONS: &[Section] = &[Section::Starred, Section::Live, Section::Dispatched];

impl Section {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starred => "starred",
            Self::Live => "live",
            Self::Dispatched => "dispatched",
            Self::Drafts => "drafts",
            Self::Archived => "archived",
            Self::Unread => "unread",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Starred => "Starred",
            Self::Live => "Live",
            Self::Dispatched => "Dispatched",
            Self::Drafts => "Drafts",
            Self::Archived => "Archived",
            Self::Unread => "Unread only",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        SECTIONS.iter().copied().find(|s| s.as_str() == text)
    }
}

/// The chosen sections. Never empty: an empty set renders nothing, so parsing
/// falls back to the default rather than stranding the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sections(BTreeSet<Section>);

impl Default for Sections {
    fn default() -> Self {
        Self(DEFAULT_SECTIONS.iter().copied().collect())
    }
}

impl Sections {
    #[must_use]
    pub fn has(&self, section: Section) -> bool {
        self.0.contains(&section)
    }

    /// Toggling the last ownership bucket off would empty the list, so the
    /// default comes back instead.
    pub fn toggle(&mut self, section: Section) {
        if !self.0.remove(&section) {
            self.0.insert(section);
        }
        if self.0.iter().all(|s| *s == Section::Unread) {
            *self = Self::default();
        }
    }

    /// `starred,live,dispatched` — the webui's `sessionList.section` string.
    #[must_use]
    pub fn serialize(&self) -> String {
        self.0.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let set: BTreeSet<Section> = raw.split(',').filter_map(Section::parse).collect();
        if set.is_empty() { Self::default() } else { Self(set) }
    }

    /// Whether a session survives the section choice.
    #[must_use]
    pub fn shows(&self, s: &SessionListItem) -> bool {
        if self.has(Section::Unread) && s.unread_count == 0 {
            return false;
        }
        self.owns(s)
    }

    fn owns(&self, s: &SessionListItem) -> bool {
        use cctui_proto::models::SessionStatus;
        if s.status == SessionStatus::Archived {
            return self.has(Section::Archived);
        }
        if s.status == SessionStatus::Draft {
            return self.has(Section::Drafts);
        }
        if s.pinned {
            return self.has(Section::Starred);
        }
        if is_dispatched(s) {
            return self.has(Section::Dispatched);
        }
        self.has(Section::Live)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Activity,
    Created,
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

const SORT_CYCLE: &[Sort] = &[Sort::Activity, Sort::Created, Sort::Name];

impl Sort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activity => "activity",
            Self::Created => "created",
            Self::Name => "name",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        SORT_CYCLE.iter().copied().find(|s| s.as_str() == text)
    }

    #[must_use]
    pub fn next(self) -> Self {
        let at = SORT_CYCLE.iter().position(|s| *s == self).unwrap_or(0);
        SORT_CYCLE[(at + 1) % SORT_CYCLE.len()]
    }

    /// Newest-first for the date fields, A→Z for names.
    #[must_use]
    pub const fn natural_dir(self) -> SortDir {
        match self {
            Self::Name => SortDir::Asc,
            Self::Activity | Self::Created => SortDir::Desc,
        }
    }
}

impl SortDir {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    pub const fn arrow(self) -> &'static str {
        match self {
            Self::Asc => "↑",
            Self::Desc => "↓",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "asc" => Some(Self::Asc),
            "desc" => Some(Self::Desc),
            _ => None,
        }
    }

    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

/// Picking a new field resets to that field's natural direction; picking the
/// one already active flips it.
#[must_use]
pub fn next_sort(current: Sort, dir: SortDir, chosen: Sort) -> (Sort, SortDir) {
    if current == chosen { (chosen, dir.flipped()) } else { (chosen, chosen.natural_dir()) }
}

/// The dimension rows are bucketed by. `Status` is the bucketed list the TUI
/// has always shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    Status,
    Label,
    WorkingDir,
    Machine,
    Room,
}

const GROUP_CYCLE: &[GroupBy] =
    &[GroupBy::Status, GroupBy::Label, GroupBy::WorkingDir, GroupBy::Machine, GroupBy::Room];

impl GroupBy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Label => "label",
            Self::WorkingDir => "working_dir",
            Self::Machine => "machine",
            Self::Room => "room",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Label => "label",
            Self::WorkingDir => "directory",
            Self::Machine => "machine",
            Self::Room => "room",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        GROUP_CYCLE.iter().copied().find(|g| g.as_str() == text)
    }

    #[must_use]
    pub fn next(self) -> Self {
        let at = GROUP_CYCLE.iter().position(|g| *g == self).unwrap_or(0);
        GROUP_CYCLE[(at + 1) % GROUP_CYCLE.len()]
    }
}

/// Which dimension tints a row, or nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorBy {
    #[default]
    None,
    Label,
    WorkingDir,
    Machine,
    Room,
}

impl ColorBy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Label => "label",
            Self::WorkingDir => "working_dir",
            Self::Machine => "machine",
            Self::Room => "room",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        [Self::None, Self::Label, Self::WorkingDir, Self::Machine, Self::Room]
            .into_iter()
            .find(|c| c.as_str() == text)
    }

    /// The key whose hue tints the row, or `None` when the dimension is off or
    /// the session carries nothing for it.
    #[must_use]
    pub fn key_of(self, s: &SessionListItem) -> Option<String> {
        match self {
            Self::None => None,
            Self::Label => s.labels.first().map(|l| l.id.clone()),
            Self::WorkingDir => Some(s.working_dir.clone()).filter(|d| !d.is_empty()),
            Self::Machine => Some(s.machine_id.clone()).filter(|m| !m.is_empty()),
            Self::Room => s.room_id.clone(),
        }
    }
}

/// The bucket label a session falls in for the chosen dimension. `None` means
/// the status grouping, which [`super::session_list::rows`] owns.
#[must_use]
pub fn group_key_of(s: &SessionListItem, by: GroupBy) -> Option<String> {
    match by {
        GroupBy::Status => None,
        GroupBy::Label => {
            Some(s.labels.first().map_or_else(|| "no label".to_owned(), |l| l.name.clone()))
        }
        GroupBy::WorkingDir => Some(if s.working_dir.is_empty() {
            "no directory".to_owned()
        } else {
            s.working_dir.clone()
        }),
        GroupBy::Machine => Some(
            s.machine_name
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| s.machine_id.clone()),
        ),
        GroupBy::Room => Some(s.room_name.clone().unwrap_or_else(|| "no room".to_owned())),
    }
}

/// Name a row sorts under: the session's own name, else its directory's last
/// segment, else its id — the webui's label order.
fn sort_label(s: &SessionListItem) -> String {
    s.name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| s.working_dir.rsplit('/').find(|seg| !seg.is_empty()).map(str::to_owned))
        .unwrap_or_else(|| s.id.clone())
        .to_lowercase()
}

/// `activity`/`desc` is the server's own order, so it leaves the input alone;
/// everything else reorders in place.
pub fn sort_refs(out: &mut Vec<&SessionListItem>, sort: Sort, dir: SortDir) {
    match sort {
        Sort::Activity => {
            if dir == SortDir::Asc {
                out.reverse();
            }
        }
        Sort::Created => {
            out.sort_by_key(|s: &&SessionListItem| {
                s.registered_at.map_or(0, |r| r.timestamp_millis())
            });
            if dir == SortDir::Desc {
                out.reverse();
            }
        }
        Sort::Name => {
            out.sort_by_key(|s| sort_label(s));
            if dir == SortDir::Desc {
                out.reverse();
            }
        }
    }
}

/// `sort_refs` over owned rows. Test-only: the list sorts borrowed rows.
#[cfg(test)]
#[must_use]
pub fn sort_sessions(rows: &[SessionListItem], sort: Sort, dir: SortDir) -> Vec<SessionListItem> {
    let mut refs: Vec<&SessionListItem> = rows.iter().collect();
    sort_refs(&mut refs, sort, dir);
    refs.into_iter().cloned().collect()
}

/// A `sessionList` key this view owns, so a write can name just the one that
/// changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKey {
    Section,
    Sort,
    SortDir,
    GroupBy,
    ColorBy,
}

impl ShapeKey {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::Sort => "sort",
            Self::SortDir => "sortDir",
            Self::GroupBy => "groupBy",
            Self::ColorBy => "colorBy",
        }
    }
}

/// Every key this view owns. Only a full round trip wants all of them: a
/// write names the keys the user changed.
#[cfg(test)]
pub const SHAPE_KEYS: [ShapeKey; 5] =
    [ShapeKey::Section, ShapeKey::Sort, ShapeKey::SortDir, ShapeKey::GroupBy, ShapeKey::ColorBy];

/// Everything the list's shape is made of, restored from the server settings at
/// startup and written back on every change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListShape {
    pub sections: Sections,
    pub sort: Sort,
    pub sort_dir: SortDir,
    pub group_by: GroupBy,
    pub color_by: ColorBy,
}

impl Default for ListShape {
    fn default() -> Self {
        Self {
            sections: Sections::default(),
            sort: Sort::default(),
            sort_dir: Sort::default().natural_dir(),
            group_by: GroupBy::default(),
            color_by: ColorBy::default(),
        }
    }
}

impl ListShape {
    /// Reads the webui's `sessionList` blob, keeping the default for anything
    /// missing or unreadable.
    #[must_use]
    pub fn from_settings(data: &serde_json::Value) -> Self {
        let list = data.get("sessionList");
        let text = |key: &str| {
            list.and_then(|l| l.get(key)).and_then(serde_json::Value::as_str).map(str::to_owned)
        };
        let mut shape = Self::default();
        if let Some(raw) = text("section") {
            shape.sections = Sections::parse(&raw);
        }
        if let Some(sort) = text("sort").and_then(|s| Sort::parse(&s)) {
            shape.sort = sort;
            shape.sort_dir = sort.natural_dir();
        }
        if let Some(dir) = text("sortDir").and_then(|d| SortDir::parse(&d)) {
            shape.sort_dir = dir;
        }
        if let Some(by) = text("groupBy").and_then(|g| GroupBy::parse(&g)) {
            shape.group_by = by;
        }
        if let Some(by) = text("colorBy").and_then(|c| ColorBy::parse(&c)) {
            shape.color_by = by;
        }
        shape
    }

    /// This key's current value, as the `sessionList` blob stores it.
    #[must_use]
    pub fn key_value(&self, key: ShapeKey) -> serde_json::Value {
        let text = match key {
            ShapeKey::Section => self.sections.serialize(),
            ShapeKey::Sort => self.sort.as_str().to_owned(),
            ShapeKey::SortDir => self.sort_dir.as_str().to_owned(),
            ShapeKey::GroupBy => self.group_by.as_str().to_owned(),
            ShapeKey::ColorBy => self.color_by.as_str().to_owned(),
        };
        serde_json::Value::String(text)
    }

    /// The `sessionList` patch for exactly the keys that changed.
    ///
    /// A write must not name a key the user did not touch: the value here came
    /// from a read taken at startup, so re-sending it would revert whatever the
    /// web UI stored for that key since.
    #[must_use]
    pub fn settings_patch_for(&self, keys: &[ShapeKey]) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for key in keys {
            map.insert(key.as_str().to_owned(), self.key_value(*key));
        }
        serde_json::Value::Object(map)
    }

    #[cfg(test)]
    #[must_use]
    pub fn settings_patch(&self) -> serde_json::Value {
        self.settings_patch_for(&SHAPE_KEYS)
    }

    /// `sort: activity ↓  group: machine` for the status line.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "sort: {} {}  group: {}",
            self.sort.as_str(),
            self.sort_dir.arrow(),
            self.group_by.title()
        )
    }
}

/// The rows the list shows: borrowed, never cloned, so an in-place edit to a
/// session is visible the moment it happens.
#[must_use]
pub fn visible_refs<'a>(
    sessions: &'a [SessionListItem],
    shape: &ListShape,
) -> Vec<&'a SessionListItem> {
    let mut kept: Vec<&'a SessionListItem> =
        sessions.iter().filter(|s| shape.sections.shows(s)).collect();
    sort_refs(&mut kept, shape.sort, shape.sort_dir);
    kept
}

/// Rows bucketed by the chosen dimension, in first-seen order. The one place
/// the bucketing lives: [`super::session_list::rows_by`] renders these.
#[must_use]
pub fn dimension_groups<'a>(
    sessions: &[&'a SessionListItem],
    by: GroupBy,
) -> Vec<(String, Vec<&'a SessionListItem>)> {
    let mut out: Vec<(String, Vec<&'a SessionListItem>)> = Vec::new();
    for s in sessions {
        let key = group_key_of(s, by).unwrap_or_else(|| group_of(s).label().to_owned());
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, rows)) => rows.push(s),
            None => out.push((key, vec![s])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        ColorBy, GroupBy, ListShape, Section, Sections, Sort, SortDir, dimension_groups,
        group_key_of, next_sort, sort_sessions, visible_refs,
    };
    use crate::testsupport::session;

    fn archived(id: &str) -> cctui_proto::api::SessionListItem {
        let mut s = session(id, "old", "archived", "done");
        s.status = cctui_proto::models::SessionStatus::Archived;
        s
    }

    #[test]
    fn the_default_sections_are_the_webuis_three() {
        let sections = Sections::default();
        assert!(sections.has(Section::Starred));
        assert!(sections.has(Section::Live));
        assert!(sections.has(Section::Dispatched));
        assert!(!sections.has(Section::Drafts));
        assert!(!sections.has(Section::Archived));
        assert_eq!(sections.serialize(), "starred,live,dispatched");
    }

    #[test]
    fn sections_round_trip_through_the_settings_string() {
        let restored = Sections::parse("live,archived");
        assert!(restored.has(Section::Live));
        assert!(restored.has(Section::Archived));
        assert!(!restored.has(Section::Starred));
        assert_eq!(Sections::parse(&restored.serialize()), restored);
    }

    #[test]
    fn an_unreadable_or_empty_section_string_falls_back_to_the_default() {
        assert_eq!(Sections::parse(""), Sections::default());
        assert_eq!(Sections::parse("nonsense,,"), Sections::default());
    }

    #[test]
    fn turning_off_the_last_bucket_restores_the_default_rather_than_emptying_the_list() {
        let mut sections = Sections::parse("live");
        sections.toggle(Section::Live);
        assert_eq!(sections, Sections::default(), "an empty list would render nothing");

        let mut only_unread = Sections::parse("live,unread");
        only_unread.toggle(Section::Live);
        assert_eq!(only_unread, Sections::default(), "unread alone owns no rows");
    }

    #[test]
    fn archived_and_draft_rows_need_their_own_section() {
        let sections = Sections::default();
        assert!(!sections.shows(&archived("s-old")));
        let with_archived = Sections::parse("live,archived");
        assert!(with_archived.shows(&archived("s-old")));
    }

    #[test]
    fn unread_narrows_whatever_the_buckets_let_through() {
        let mut read = session("s-a", "alpha", "active", "working");
        read.unread_count = 0;
        let mut unread = session("s-b", "beta", "active", "working");
        unread.unread_count = 3;

        let plain = Sections::parse("live");
        assert!(plain.shows(&read));
        assert!(plain.shows(&unread));

        let only_unread = Sections::parse("live,unread");
        assert!(!only_unread.shows(&read));
        assert!(only_unread.shows(&unread));
    }

    #[test]
    fn a_pinned_session_belongs_to_starred_not_live() {
        let mut pinned = session("s-p", "pin", "active", "working");
        pinned.pinned = true;
        assert!(Sections::parse("starred").shows(&pinned));
        assert!(!Sections::parse("live").shows(&pinned));
    }

    #[test]
    fn the_sort_cycle_wraps_and_each_field_has_a_natural_direction() {
        assert_eq!(Sort::Activity.next(), Sort::Created);
        assert_eq!(Sort::Created.next(), Sort::Name);
        assert_eq!(Sort::Name.next(), Sort::Activity);
        assert_eq!(Sort::Activity.natural_dir(), SortDir::Desc);
        assert_eq!(Sort::Name.natural_dir(), SortDir::Asc);
    }

    #[test]
    fn choosing_the_active_field_flips_it_and_another_field_resets_the_direction() {
        assert_eq!(
            next_sort(Sort::Activity, SortDir::Desc, Sort::Activity),
            (Sort::Activity, SortDir::Asc)
        );
        assert_eq!(
            next_sort(Sort::Activity, SortDir::Asc, Sort::Name),
            (Sort::Name, SortDir::Asc),
            "a new field takes its own natural direction"
        );
        assert_eq!(
            next_sort(Sort::Name, SortDir::Asc, Sort::Created),
            (Sort::Created, SortDir::Desc)
        );
    }

    #[test]
    fn activity_desc_is_the_servers_own_order_and_asc_is_its_reverse() {
        let rows = vec![
            session("s-1", "one", "active", "working"),
            session("s-2", "two", "active", "working"),
        ];
        let same = sort_sessions(&rows, Sort::Activity, SortDir::Desc);
        assert_eq!(same.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), ["s-1", "s-2"]);
        let flipped = sort_sessions(&rows, Sort::Activity, SortDir::Asc);
        assert_eq!(flipped.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), ["s-2", "s-1"]);
    }

    #[test]
    fn name_sorting_uses_the_name_then_the_directory_then_the_id() {
        let mut named = session("s-1", "zeta", "active", "working");
        named.name = Some("alpha".to_owned());
        let dir_only = session("s-2", "beta", "active", "working");
        let rows = vec![dir_only, named];
        let sorted = sort_sessions(&rows, Sort::Name, SortDir::Asc);
        assert_eq!(
            sorted.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            ["s-1", "s-2"],
            "the explicit name wins over the directory basename"
        );
    }

    #[test]
    fn created_sorting_puts_the_newest_first_by_default() {
        let mut old = session("s-old", "old", "active", "working");
        old.registered_at = chrono::DateTime::from_timestamp(1_000, 0);
        let mut new = session("s-new", "new", "active", "working");
        new.registered_at = chrono::DateTime::from_timestamp(2_000, 0);
        let sorted = sort_sessions(&[old, new], Sort::Created, SortDir::Desc);
        assert_eq!(sorted.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), ["s-new", "s-old"]);
    }

    #[test]
    fn the_group_cycle_wraps_through_every_dimension() {
        let mut seen = vec![GroupBy::Status];
        let mut at = GroupBy::Status;
        for _ in 0..4 {
            at = at.next();
            seen.push(at);
        }
        assert_eq!(
            seen,
            [GroupBy::Status, GroupBy::Label, GroupBy::WorkingDir, GroupBy::Machine, GroupBy::Room]
        );
        assert_eq!(at.next(), GroupBy::Status);
    }

    #[test]
    fn a_dimension_key_falls_back_to_a_named_empty_bucket() {
        let s = session("s-a", "alpha", "active", "working");
        assert_eq!(group_key_of(&s, GroupBy::Status), None, "status is the row model's own");
        assert_eq!(group_key_of(&s, GroupBy::Label).as_deref(), Some("no label"));
        assert_eq!(group_key_of(&s, GroupBy::WorkingDir).as_deref(), Some("/home/dev/alpha"));
        assert_eq!(group_key_of(&s, GroupBy::Machine).as_deref(), Some("orion"));
        assert_eq!(group_key_of(&s, GroupBy::Room).as_deref(), Some("no room"));
    }

    #[test]
    fn grouping_by_a_dimension_keeps_first_seen_order() {
        let a = session("s-a", "alpha", "active", "working");
        let mut b = session("s-b", "beta", "active", "working");
        b.machine_id = "cyberia".to_owned();
        b.machine_name = Some("cyberia".to_owned());
        let c = session("s-c", "gamma", "active", "working");
        let rows = [&a, &b, &c];
        let groups = dimension_groups(&rows, GroupBy::Machine);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "orion");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "cyberia");
    }

    #[test]
    fn the_color_dimension_picks_the_key_whose_hue_tints_the_row() {
        let mut s = session("s-a", "alpha", "active", "working");
        assert_eq!(ColorBy::None.key_of(&s), None);
        assert_eq!(ColorBy::Machine.key_of(&s).as_deref(), Some("orion"));
        assert_eq!(ColorBy::WorkingDir.key_of(&s).as_deref(), Some("/home/dev/alpha"));
        assert_eq!(ColorBy::Label.key_of(&s), None, "no label, no tint");
        s.room_id = Some("r-1".to_owned());
        assert_eq!(ColorBy::Room.key_of(&s).as_deref(), Some("r-1"));
    }

    #[test]
    fn the_shape_round_trips_through_the_webui_settings_blob() {
        let shape = ListShape {
            sections: Sections::parse("live,archived"),
            sort: Sort::Name,
            sort_dir: SortDir::Desc,
            group_by: GroupBy::Machine,
            color_by: ColorBy::Label,
        };
        let blob = serde_json::json!({ "sessionList": shape.settings_patch() });
        assert_eq!(ListShape::from_settings(&blob), shape);
    }

    #[test]
    fn a_sort_without_a_direction_takes_the_natural_one() {
        let blob = serde_json::json!({ "sessionList": { "sort": "name" } });
        let shape = ListShape::from_settings(&blob);
        assert_eq!(shape.sort, Sort::Name);
        assert_eq!(shape.sort_dir, SortDir::Asc);
    }

    #[test]
    fn an_empty_or_unreadable_blob_leaves_the_defaults() {
        assert_eq!(ListShape::from_settings(&serde_json::json!({})), ListShape::default());
        let junk = serde_json::json!({
            "sessionList": {"sort": "sideways", "groupBy": "vibes", "colorBy": "mood"}
        });
        assert_eq!(ListShape::from_settings(&junk), ListShape::default());
    }

    #[test]
    fn the_summary_reads_like_the_status_line_the_ticket_asks_for() {
        let mut shape = ListShape::default();
        assert_eq!(shape.summary(), "sort: activity ↓  group: status");
        shape.group_by = GroupBy::Machine;
        shape.sort_dir = SortDir::Asc;
        assert_eq!(shape.summary(), "sort: activity ↑  group: machine");
    }

    #[test]
    fn the_visible_rows_are_filtered_then_sorted() {
        let mut pinned = session("s-p", "pin", "active", "working");
        pinned.pinned = true;
        pinned.name = Some("zeta".to_owned());
        let mut live = session("s-l", "live", "active", "working");
        live.name = Some("alpha".to_owned());
        let rows = vec![pinned, live, archived("s-old")];

        // Through the real transition, so the direction is the one `o` would set.
        let base = ListShape::default();
        let (sort, sort_dir) = next_sort(base.sort, base.sort_dir, Sort::Name);
        let shape = ListShape { sort, sort_dir, ..base };
        let visible = visible_refs(&rows, &shape);
        assert_eq!(
            visible.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            ["s-l", "s-p"],
            "the archived row is out and the rest are A→Z"
        );
    }
}
