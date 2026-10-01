//! Session list grouping, subagent nesting, folding and viewport maths.
//!
//! Grouping mirrors the webui `routes/sessions/sessions.logic.ts`.

use std::collections::{HashMap, HashSet};

use cctui_proto::api::SessionListItem;
use cctui_proto::classifier::Bucket;
use cctui_proto::models::Liveness;

use crate::config::uistate::UiState;

/// Machine kinds the server manages itself; their sessions are unattended.
pub const SYSTEM_MACHINE_KINDS: [&str; 2] = ["dispatch", "ephemeral"];

/// A session list group, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Pinned,
    Bucket(Bucket),
    Dispatched,
}

impl Group {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pinned => "Pinned",
            Self::Bucket(b) => b.label(),
            Self::Dispatched => "Dispatched",
        }
    }

    /// Stable key for the persisted fold state; the label is free to change.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Bucket(Bucket::Working) => "working",
            Self::Bucket(Bucket::Blocked) => "blocked",
            Self::Bucket(Bucket::Review) => "review",
            Self::Bucket(Bucket::Done) => "done",
            Self::Dispatched => "dispatched",
        }
    }

    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Pinned => 0,
            Self::Bucket(Bucket::Blocked) => 1,
            Self::Bucket(Bucket::Review) => 2,
            Self::Bucket(Bucket::Working) => 3,
            Self::Bucket(Bucket::Done) => 4,
            Self::Dispatched => 5,
        }
    }
}

/// `true` when the session runs on a server-managed machine.
#[must_use]
pub fn is_dispatched(s: &SessionListItem) -> bool {
    s.machine_kind.as_deref().is_some_and(|k| SYSTEM_MACHINE_KINDS.contains(&k))
}

/// Group owning a session: pinned wins, then blocked, then dispatch.
#[must_use]
pub fn group_of(s: &SessionListItem) -> Group {
    if s.pinned {
        return Group::Pinned;
    }
    if s.bucket == Bucket::Blocked {
        return Group::Bucket(Bucket::Blocked);
    }
    if is_dispatched(s) {
        return Group::Dispatched;
    }
    Group::Bucket(s.bucket)
}

/// Uptime derived from `registered_at`; 0 when unset.
#[must_use]
/// Only an ordering key: every row is measured against the same instant, so
/// the order it produces does not depend on which instant that is. Anything
/// *rendered* must use [`uptime_secs_at`] instead, or the row text drifts with
/// the wall clock and takes every snapshot of it along.
pub fn uptime_secs(s: &SessionListItem) -> i64 {
    uptime_secs_at(s, chrono::Utc::now().timestamp_millis())
}

#[must_use]
pub fn uptime_secs_at(s: &SessionListItem, now_ms: i64) -> i64 {
    s.registered_at.map_or(0, |r| (now_ms - r.timestamp_millis()) / 1_000)
}

fn meta_str<'a>(s: &'a SessionListItem, key: &str) -> Option<&'a str> {
    s.metadata.get(key).and_then(serde_json::Value::as_str).filter(|v| !v.is_empty())
}

/// A fork keeps its own top-level row: it is a sibling of its parent, not a
/// child of it.
fn is_fork(s: &SessionListItem) -> bool {
    meta_str(s, "relation") == Some("fork")
}

/// Agents still worth counting as running.
#[must_use]
pub fn running_count(agents: &[&SessionListItem]) -> usize {
    agents
        .iter()
        .filter(|a| {
            a.status != cctui_proto::models::SessionStatus::Archived
                && a.liveness != Liveness::Dead
                && !a.hibernated
        })
        .count()
}

/// A parent's children, split into the plain group and one group per workflow run.
pub struct SubGroup<'a> {
    /// `plain` or `wf:<run id>`, unique within a parent.
    pub key: String,
    pub label: String,
    pub agents: Vec<&'a SessionListItem>,
    pub running: usize,
}

/// Stable id for a group's fold state.
#[must_use]
pub fn group_id(parent_id: &str, key: &str) -> String {
    format!("{parent_id}/{key}")
}

/// Plain children first, then each workflow run in the order it was seen.
#[must_use]
pub fn sub_groups<'a>(kids: &[&'a SessionListItem]) -> Vec<SubGroup<'a>> {
    let mut plain: Vec<&'a SessionListItem> = Vec::new();
    let mut order: Vec<&'a str> = Vec::new();
    let mut by_run: HashMap<&'a str, (Option<&'a str>, Vec<&'a SessionListItem>)> = HashMap::new();
    for &k in kids {
        let Some(run) = meta_str(k, "workflow_run_id") else {
            plain.push(k);
            continue;
        };
        let entry = by_run.entry(run).or_insert_with(|| {
            order.push(run);
            (meta_str(k, "workflow_name"), Vec::new())
        });
        entry.1.push(k);
    }

    let mut out: Vec<SubGroup<'a>> = Vec::with_capacity(order.len() + 1);
    if !plain.is_empty() {
        out.push(SubGroup {
            key: "plain".to_owned(),
            label: "subagents".to_owned(),
            running: running_count(&plain),
            agents: plain,
        });
    }
    for run in order {
        let (name, agents) = by_run.remove(run).expect("the run was just inserted");
        out.push(SubGroup {
            key: format!("wf:{run}"),
            label: name.map_or_else(|| "workflow".to_owned(), |n| format!("wf: {n}")),
            running: running_count(&agents),
            agents,
        });
    }
    out
}

/// One rendered line of the list.
#[derive(Debug, Clone)]
pub enum Row<'a> {
    Header {
        group: Group,
        /// Top-level sessions the group holds, folded or not.
        total: usize,
        open: bool,
    },
    /// Header of a group-by dimension other than status, whose buckets are
    /// data rather than a fixed enum.
    DimHeader {
        key: String,
        total: usize,
        open: bool,
    },
    Session {
        session: &'a SessionListItem,
        /// Index into [`sessions_of`], i.e. what `selected_index` addresses.
        index: usize,
        depth: usize,
    },
    SubHeader {
        id: String,
        label: String,
        total: usize,
        running: usize,
        open: bool,
        depth: usize,
    },
}

struct Walk<'a, 'u> {
    kids: HashMap<&'a str, Vec<&'a SessionListItem>>,
    ui: &'u UiState,
    out: Vec<Row<'a>>,
    seen: HashSet<&'a str>,
    index: usize,
}

impl<'a> Walk<'a, '_> {
    fn descend(&mut self, parent: &'a SessionListItem, depth: usize) {
        let Some(kids) = self.kids.get(parent.id.as_str()).cloned() else { return };
        for group in sub_groups(&kids) {
            let id = group_id(&parent.id, &group.key);
            let open = self.ui.group_open(&id, group.agents.len());
            self.out.push(Row::SubHeader {
                id,
                label: group.label,
                total: group.agents.len(),
                running: group.running,
                open,
                depth,
            });
            if !open {
                continue;
            }
            for agent in group.agents {
                if !self.seen.insert(agent.id.as_str()) {
                    continue;
                }
                self.out.push(Row::Session { session: agent, index: self.index, depth });
                self.index += 1;
                self.descend(agent, depth + 1);
            }
        }
    }
}

/// `rows_by` over an unsorted, unfiltered list under the status grouping.
/// Test-only: the app always goes through [`rows_by`] with its chosen shape.
#[cfg(test)]
#[must_use]
pub fn rows<'a>(sessions: &'a [SessionListItem], ui: &UiState) -> Vec<Row<'a>> {
    let mut refs: Vec<&'a SessionListItem> = sessions.iter().collect();
    // No caller-chosen sort here: youngest first, as the list has always shown.
    refs.sort_by_key(|s| uptime_secs(s));
    rows_by(&refs, sessions, ui, super::list_view::GroupBy::Status)
}

/// Every display row, honouring the persisted fold state. A folded section
/// drops its rows but keeps its header and count; a folded subagent group
/// likewise.
///
/// `ordered` is the membership and the order: already narrowed by the sections
/// and sorted. `all` is every session the server sent, and is only ever read
/// for parentage and for the unread rule — judging either against the narrowed
/// list would turn a hidden row's children into top-level ones.
#[must_use]
pub fn rows_by<'a>(
    ordered: &[&'a SessionListItem],
    all: &'a [SessionListItem],
    ui: &UiState,
    by: super::list_view::GroupBy,
) -> Vec<Row<'a>> {
    let ids: HashSet<&str> = all.iter().map(|s| s.id.as_str()).collect();
    let shown = |s: &SessionListItem| !ui.unread_only || super::unread::keeps_row(all, s);
    let mut kids: HashMap<&str, Vec<&SessionListItem>> = HashMap::new();
    for s in ordered {
        if is_fork(s) || !shown(s) {
            continue;
        }
        if let Some(p) = s.parent_id.as_deref().filter(|p| ids.contains(p) && *p != s.id) {
            kids.entry(p).or_default().push(s);
        }
    }

    let mut tops: Vec<&'a SessionListItem> = ordered
        .iter()
        .copied()
        .filter(|s| {
            shown(s)
                && (is_fork(s)
                    || s.parent_id.as_deref().is_none_or(|p| !ids.contains(p) || p == s.id))
        })
        .collect();
    // Stable, and by group rank alone: within a group the caller's order — the
    // sort the operator chose — is the one that survives.
    tops.sort_by_key(|s| group_of(s).rank());
    for group in kids.values_mut() {
        group.sort_by_key(|s| uptime_secs(s));
    }

    let mut totals: HashMap<&'static str, usize> = HashMap::new();
    for top in &tops {
        *totals.entry(group_of(top).key()).or_default() += 1;
    }

    let mut walk =
        Walk { kids, ui, out: Vec::with_capacity(tops.len() + 8), seen: HashSet::new(), index: 0 };
    if by != super::list_view::GroupBy::Status {
        return walk.by_dimension(&tops, by);
    }
    let mut current: Option<Group> = None;
    for top in tops {
        let group = group_of(top);
        let open = ui.section_open(group.key());
        if current != Some(group) {
            current = Some(group);
            walk.out.push(Row::Header {
                group,
                total: totals.get(group.key()).copied().unwrap_or_default(),
                open,
            });
        }
        if !open {
            continue;
        }
        if !walk.seen.insert(top.id.as_str()) {
            continue;
        }
        walk.out.push(Row::Session { session: top, index: walk.index, depth: 0 });
        walk.index += 1;
        walk.descend(top, 1);
    }
    walk.out
}

impl<'a> Walk<'a, '_> {
    /// Buckets the top-level rows by `by`, in first-seen order, under one header
    /// each. Dimension headers fold through the same section state as the status
    /// ones, keyed `dim:<key>` so the two never collide.
    fn by_dimension(
        mut self,
        tops: &[&'a SessionListItem],
        by: super::list_view::GroupBy,
    ) -> Vec<Row<'a>> {
        for (key, members) in super::list_view::dimension_groups(tops, by) {
            let fold_key = format!("dim:{key}");
            let open = self.ui.section_open(&fold_key);
            self.out.push(Row::DimHeader { key, total: members.len(), open });
            if !open {
                continue;
            }
            for top in members {
                if !self.seen.insert(top.id.as_str()) {
                    continue;
                }
                self.out.push(Row::Session { session: top, index: self.index, depth: 0 });
                self.index += 1;
                self.descend(top, 1);
            }
        }
        self.out
    }
}

/// The visible sessions, in the order `selected_index` addresses.
#[must_use]
pub fn sessions_of<'a>(rows: &[Row<'a>]) -> Vec<&'a SessionListItem> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Session { session, .. } => Some(*session),
            _ => None,
        })
        .collect()
}

/// Every foldable group on screen plus every section, for a fold-everything key.
#[must_use]
pub fn fold_targets(rows: &[Row<'_>]) -> (Vec<(String, usize)>, Vec<String>) {
    let mut groups = Vec::new();
    let mut sections = Vec::new();
    for row in rows {
        match row {
            Row::SubHeader { id, total, .. } => groups.push((id.clone(), *total)),
            Row::Header { group, .. } => sections.push(group.key().to_owned()),
            Row::DimHeader { key, .. } => sections.push(format!("dim:{key}")),
            Row::Session { .. } => {}
        }
    }
    (groups, sections)
}

/// The groups `z` toggles for the session at `session_id`: its own subagent
/// groups when it has any, else the one group it sits in.
#[must_use]
pub fn fold_scope(rows: &[Row<'_>], session_id: &str) -> Vec<(String, usize)> {
    let prefix = format!("{session_id}/");
    let own: Vec<(String, usize)> = rows
        .iter()
        .filter_map(|r| match r {
            Row::SubHeader { id, total, .. } if id.starts_with(&prefix) => {
                Some((id.clone(), *total))
            }
            _ => None,
        })
        .collect();
    if !own.is_empty() {
        return own;
    }
    // A leaf: fold the group it belongs to, which is the nearest sub-header above.
    let mut nearest: Option<(String, usize)> = None;
    for row in rows {
        match row {
            Row::SubHeader { id, total, .. } => nearest = Some((id.clone(), *total)),
            Row::Session { session, .. } if session.id == session_id => {
                return nearest.into_iter().collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

/// Row index of the session at `selected` in [`sessions_of`] order.
#[must_use]
pub fn selected_row(rows: &[Row<'_>], selected: usize) -> usize {
    rows.iter()
        .position(|r| matches!(r, Row::Session { index, .. } if *index == selected))
        .unwrap_or(0)
}

/// First visible row so that `selected` stays inside a `height`-row viewport.
#[must_use]
pub const fn viewport_offset(total: usize, selected: usize, height: usize) -> usize {
    if height == 0 || total <= height {
        return 0;
    }
    let max_offset = total - height;
    let wanted = if selected >= height { selected + 1 - height } else { 0 };
    if wanted > max_offset { max_offset } else { wanted }
}

#[cfg(test)]
mod tests {
    use cctui_proto::classifier::Bucket;

    use super::{
        Group, Row, UiState, fold_scope, fold_targets, group_of, is_dispatched, rows,
        running_count, selected_row, sessions_of, sub_groups, uptime_secs_at, viewport_offset,
    };
    use crate::testsupport::{dispatched_session, pinned_session, session, subagent};

    /// The rendered uptime is a function of the app clock and nothing else: a
    /// row that reads the wall clock rots every snapshot it appears in.
    #[test]
    fn the_rendered_uptime_only_moves_with_the_app_clock() {
        let mut s = session("s-a", "alpha", "active", "working");
        s.registered_at = chrono::DateTime::from_timestamp_millis(1_000_000);
        assert_eq!(uptime_secs_at(&s, 1_000_000), 0);
        assert_eq!(uptime_secs_at(&s, 1_000_000 + 3_600_000), 3_600);
        assert_eq!(uptime_secs_at(&s, 1_000_000), 0, "the same clock gives the same answer");

        s.registered_at = None;
        assert_eq!(uptime_secs_at(&s, i64::MAX), 0, "an unregistered row has no uptime");
    }

    fn flat<'a>(
        sessions: &'a [cctui_proto::api::SessionListItem],
        ui: &UiState,
    ) -> Vec<&'a cctui_proto::api::SessionListItem> {
        sessions_of(&rows(sessions, ui))
    }

    fn ids(sessions: &[&cctui_proto::api::SessionListItem]) -> Vec<String> {
        sessions.iter().map(|s| s.id.clone()).collect()
    }

    fn headers(rows: &[Row<'_>]) -> Vec<Group> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Header { group, .. } => Some(*group),
                _ => None,
            })
            .collect()
    }

    fn sub_headers(rows: &[Row<'_>]) -> Vec<(String, usize, usize, bool)> {
        rows.iter()
            .filter_map(|r| match r {
                Row::SubHeader { id, total, running, open, .. } => {
                    Some((id.clone(), *total, *running, *open))
                }
                _ => None,
            })
            .collect()
    }

    fn workflow_child(
        id: &str,
        parent: &str,
        run: &str,
        name: Option<&str>,
    ) -> cctui_proto::api::SessionListItem {
        let mut s = subagent(id, parent, "lane");
        s.metadata = serde_json::json!({
            "project_name": "lane",
            "workflow_run_id": run,
            "workflow_name": name,
        });
        s
    }

    #[test]
    fn pinned_wins_over_every_bucket() {
        assert_eq!(group_of(&pinned_session("s", "p")), Group::Pinned);
    }

    #[test]
    fn dispatched_detected_from_machine_kind() {
        assert!(is_dispatched(&dispatched_session("s", "p", "working")));
        assert!(!is_dispatched(&session("s", "p", "active", "working")));
    }

    #[test]
    fn blocked_beats_dispatched_so_attention_is_never_buried() {
        assert_eq!(
            group_of(&dispatched_session("s", "p", "blocked")),
            Group::Bucket(Bucket::Blocked)
        );
        assert_eq!(group_of(&dispatched_session("s", "p", "working")), Group::Dispatched);
    }

    #[test]
    fn groups_are_ordered_pinned_first_dispatched_last() {
        let sessions = vec![
            session("s-done", "notes", "inactive", "done"),
            dispatched_session("s-disp", "worker", "working"),
            session("s-working", "cctui", "active", "working"),
            pinned_session("s-pin", "pinned"),
            session("s-blocked", "infra", "active", "blocked"),
            session("s-review", "review", "active", "review"),
        ];
        assert_eq!(
            ids(&flat(&sessions, &UiState::default())),
            ["s-pin", "s-blocked", "s-review", "s-working", "s-done", "s-disp"]
        );
    }

    #[test]
    fn subagents_follow_their_parent() {
        let sessions = vec![
            session("s-working", "cctui", "active", "working"),
            session("s-blocked", "infra", "active", "blocked"),
            subagent("s-child", "s-working", "sub"),
        ];
        assert_eq!(
            ids(&flat(&sessions, &UiState::default())),
            ["s-blocked", "s-working", "s-child"]
        );
    }

    #[test]
    fn a_fork_keeps_its_own_top_level_row() {
        let mut fork = subagent("s-fork", "s-working", "fork");
        fork.metadata = serde_json::json!({"project_name": "fork", "relation": "fork"});
        let sessions = vec![session("s-working", "cctui", "active", "working"), fork];
        let list = rows(&sessions, &UiState::default());
        assert!(sub_headers(&list).is_empty(), "a fork opens no subagent group");
        assert_eq!(ids(&sessions_of(&list)).len(), 2);
    }

    #[test]
    fn nothing_is_truncated() {
        let sessions: Vec<_> =
            (0..40).map(|i| session(&format!("s-{i}"), "p", "active", "working")).collect();
        assert_eq!(flat(&sessions, &UiState::default()).len(), 40);
    }

    #[test]
    fn headers_open_each_group_once_and_carry_its_count() {
        let sessions = vec![
            session("s-1", "a", "active", "working"),
            session("s-2", "b", "active", "working"),
            pinned_session("s-pin", "p"),
        ];
        let list = rows(&sessions, &UiState::default());
        assert_eq!(headers(&list), [Group::Pinned, Group::Bucket(Bucket::Working)]);
        match &list[2] {
            Row::Header { group, total, open } => {
                assert_eq!(*group, Group::Bucket(Bucket::Working));
                assert_eq!(*total, 2);
                assert!(open);
            }
            other => panic!("expected the Working header, got {other:?}"),
        }
        assert_eq!(list.len(), 5);
    }

    #[test]
    fn a_small_subagent_group_opens_inline_under_one_sub_header() {
        let sessions = vec![
            session("s-p", "cctui", "active", "working"),
            subagent("s-c1", "s-p", "one"),
            subagent("s-c2", "s-p", "two"),
        ];
        let list = rows(&sessions, &UiState::default());
        assert_eq!(sub_headers(&list), [("s-p/plain".to_owned(), 2, 2, true)]);
        assert_eq!(ids(&sessions_of(&list)), ["s-p", "s-c1", "s-c2"]);
    }

    #[test]
    fn a_big_subagent_group_starts_folded_and_hides_its_rows() {
        let mut sessions = vec![session("s-p", "cctui", "active", "working")];
        for i in 0..12 {
            sessions.push(subagent(&format!("s-c{i:02}"), "s-p", "lane"));
        }
        sessions[4].hibernated = true;
        let ui = UiState::default();
        let list = rows(&sessions, &ui);
        assert_eq!(sub_headers(&list), [("s-p/plain".to_owned(), 12, 11, false)]);
        assert_eq!(ids(&sessions_of(&list)), ["s-p"], "a folded group buries nothing else");

        let mut ui = ui;
        assert!(ui.toggle_group("s-p/plain", 12));
        assert_eq!(sessions_of(&rows(&sessions, &ui)).len(), 13);
    }

    #[test]
    fn workflow_children_get_their_own_group_per_run() {
        let sessions = vec![
            session("s-p", "cctui", "active", "working"),
            subagent("s-plain", "s-p", "plain"),
            workflow_child("s-a", "s-p", "run-1", Some("release-wave")),
            workflow_child("s-b", "s-p", "run-1", Some("release-wave")),
            workflow_child("s-z", "s-p", "run-2", None),
        ];
        let list = rows(&sessions, &UiState::default());
        let heads = sub_headers(&list);
        assert_eq!(heads.len(), 3);
        assert_eq!(heads[0].0, "s-p/plain");
        assert_eq!(heads[1].0, "s-p/wf:run-1");
        assert_eq!(heads[2].0, "s-p/wf:run-2");
        assert_eq!(heads[1].1, 2);

        let labels: Vec<String> = list
            .iter()
            .filter_map(|r| match r {
                Row::SubHeader { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["subagents", "wf: release-wave", "workflow"]);
    }

    #[test]
    fn the_running_count_skips_archived_dead_and_hibernated_agents() {
        let alive = session("a", "p", "active", "working");
        let mut dead = session("b", "p", "inactive", "working");
        dead.liveness = cctui_proto::models::Liveness::Dead;
        let mut sleeping = session("c", "p", "active", "working");
        sleeping.hibernated = true;
        let archived = session("d", "p", "archived", "working");
        assert_eq!(running_count(&[&alive, &dead, &sleeping, &archived]), 1);
    }

    #[test]
    fn a_folded_section_keeps_its_header_and_drops_its_rows() {
        let sessions = vec![
            session("s-1", "a", "active", "working"),
            session("s-2", "b", "active", "working"),
            pinned_session("s-pin", "p"),
        ];
        let mut ui = UiState::default();
        assert!(!ui.toggle_section("working"));
        let list = rows(&sessions, &ui);
        assert_eq!(headers(&list), [Group::Pinned, Group::Bucket(Bucket::Working)]);
        assert_eq!(ids(&sessions_of(&list)), ["s-pin"]);
        match list.last() {
            Some(Row::Header { total, open, .. }) => {
                assert_eq!(*total, 2, "the count survives the fold");
                assert!(!open);
            }
            other => panic!("expected a folded header, got {other:?}"),
        }
    }

    #[test]
    fn grandchildren_nest_one_level_deeper() {
        let sessions = vec![
            session("s-p", "cctui", "active", "working"),
            subagent("s-c", "s-p", "child"),
            subagent("s-g", "s-c", "grandchild"),
        ];
        let list = rows(&sessions, &UiState::default());
        let depths: Vec<usize> = list
            .iter()
            .filter_map(|r| match r {
                Row::Session { depth, .. } => Some(*depth),
                _ => None,
            })
            .collect();
        assert_eq!(depths, [0, 1, 2]);
    }

    #[test]
    fn a_self_parented_row_does_not_loop() {
        let mut s = session("s-loop", "p", "active", "working");
        s.parent_id = Some("s-loop".to_owned());
        let list = rows(std::slice::from_ref(&s), &UiState::default());
        assert_eq!(ids(&sessions_of(&list)), ["s-loop"]);
    }

    #[test]
    fn fold_targets_lists_every_group_and_section_on_screen() {
        let sessions = vec![
            pinned_session("s-pin", "p"),
            session("s-p", "cctui", "active", "working"),
            subagent("s-c", "s-p", "child"),
        ];
        let (groups, sections) = fold_targets(&rows(&sessions, &UiState::default()));
        assert_eq!(groups, [("s-p/plain".to_owned(), 1)]);
        assert_eq!(sections, ["pinned", "working"]);
    }

    #[test]
    fn fold_scope_is_a_parents_own_groups_and_a_childs_containing_group() {
        let sessions =
            vec![session("s-p", "cctui", "active", "working"), subagent("s-c", "s-p", "child")];
        let list = rows(&sessions, &UiState::default());
        assert_eq!(fold_scope(&list, "s-p"), [("s-p/plain".to_owned(), 1)]);
        assert_eq!(fold_scope(&list, "s-c"), [("s-p/plain".to_owned(), 1)]);
        assert!(fold_scope(&list, "nobody").is_empty());
    }

    #[test]
    fn selected_row_accounts_for_headers() {
        let sessions = vec![
            pinned_session("s-pin", "p"),
            session("s-1", "a", "active", "working"),
            session("s-2", "b", "active", "working"),
        ];
        let list = rows(&sessions, &UiState::default());
        assert_eq!(selected_row(&list, 0), 1);
        assert_eq!(selected_row(&list, 1), 3);
        assert_eq!(selected_row(&list, 2), 4);
    }

    #[test]
    fn viewport_keeps_the_selection_visible() {
        assert_eq!(viewport_offset(3, 2, 10), 0);
        assert_eq!(viewport_offset(30, 0, 10), 0);
        assert_eq!(viewport_offset(30, 9, 10), 0);
        assert_eq!(viewport_offset(30, 10, 10), 1);
        assert_eq!(viewport_offset(30, 29, 10), 20);
    }

    #[test]
    fn viewport_never_scrolls_past_the_last_row() {
        assert_eq!(viewport_offset(12, 11, 10), 2);
        assert_eq!(viewport_offset(12, 11, 0), 0);
    }

    #[test]
    fn a_group_with_no_plain_children_has_no_plain_header() {
        let kids = [workflow_child("s-a", "s-p", "run-1", None)];
        let refs: Vec<&cctui_proto::api::SessionListItem> = kids.iter().collect();
        let groups = sub_groups(&refs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "wf:run-1");
    }
}
