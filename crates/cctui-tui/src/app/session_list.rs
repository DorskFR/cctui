//! Session list grouping, flattening and viewport maths.
//!
//! Grouping mirrors the webui `routes/sessions/sessions.logic.ts`.

use cctui_proto::api::SessionListItem;
use cctui_proto::classifier::Bucket;

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
pub fn uptime_secs(s: &SessionListItem) -> i64 {
    s.registered_at.map_or(0, |r| (chrono::Utc::now() - r).num_seconds())
}

/// Sessions ordered for display: grouped, oldest first within a group, with
/// Task-tool subagents immediately after their parent.
#[must_use]
pub fn flatten(sessions: &[SessionListItem]) -> Vec<&SessionListItem> {
    use std::collections::{HashMap, HashSet};
    let ids: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let mut kids: HashMap<&str, Vec<&SessionListItem>> = HashMap::new();
    for s in sessions {
        if let Some(p) = s.parent_id.as_deref().filter(|p| ids.contains(p)) {
            kids.entry(p).or_default().push(s);
        }
    }
    let mut tops: Vec<&SessionListItem> = sessions
        .iter()
        .filter(|s| s.parent_id.as_deref().is_none_or(|p| !ids.contains(p)))
        .collect();
    tops.sort_by_key(|s| (group_of(s).rank(), uptime_secs(s)));
    let mut out: Vec<&SessionListItem> = Vec::new();
    for t in tops {
        out.push(t);
        if let Some(cs) = kids.get_mut(t.id.as_str()) {
            cs.sort_by_key(|s| uptime_secs(s));
            out.extend(cs.iter().copied());
        }
    }
    out
}

/// One rendered line of the list.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Header(Group),
    Session {
        session: &'a SessionListItem,
        /// Index into [`flatten`], i.e. what `selected_index` addresses.
        index: usize,
    },
}

/// Display rows for a flattened list: a header opens each group.
#[must_use]
pub fn rows<'a>(flat: &[&'a SessionListItem]) -> Vec<Row<'a>> {
    let mut out: Vec<Row<'a>> = Vec::with_capacity(flat.len() + 6);
    let mut current: Option<Group> = None;
    for (index, session) in flat.iter().enumerate() {
        if session.parent_id.is_none() {
            let group = group_of(session);
            if current != Some(group) {
                current = Some(group);
                out.push(Row::Header(group));
            }
        }
        out.push(Row::Session { session, index });
    }
    out
}

/// Row index of the session at `selected` in [`flatten`] order.
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
        Group, Row, flatten, group_of, is_dispatched, rows, selected_row, viewport_offset,
    };
    use crate::testsupport::{dispatched_session, pinned_session, session, subagent};

    fn ids<'a>(flat: &[&'a cctui_proto::api::SessionListItem]) -> Vec<&'a str> {
        flat.iter().map(|s| s.id.as_str()).collect()
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
            ids(&flatten(&sessions)),
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
        assert_eq!(ids(&flatten(&sessions)), ["s-blocked", "s-working", "s-child"]);
    }

    #[test]
    fn nothing_is_truncated() {
        let sessions: Vec<_> =
            (0..40).map(|i| session(&format!("s-{i}"), "p", "active", "working")).collect();
        assert_eq!(flatten(&sessions).len(), 40);
    }

    #[test]
    fn headers_open_each_group_once() {
        let sessions = vec![
            session("s-1", "a", "active", "working"),
            session("s-2", "b", "active", "working"),
            pinned_session("s-pin", "p"),
        ];
        let flat = flatten(&sessions);
        let list = rows(&flat);
        let headers: Vec<Group> = list
            .iter()
            .filter_map(|r| match r {
                Row::Header(g) => Some(*g),
                Row::Session { .. } => None,
            })
            .collect();
        assert_eq!(headers, [Group::Pinned, Group::Bucket(Bucket::Working)]);
        assert_eq!(list.len(), 5);
    }

    #[test]
    fn subagents_do_not_open_a_group() {
        let sessions = vec![
            session("s-working", "cctui", "active", "working"),
            subagent("s-child", "s-working", "sub"),
        ];
        let flat = flatten(&sessions);
        assert_eq!(rows(&flat).len(), 3);
    }

    #[test]
    fn selected_row_accounts_for_headers() {
        let sessions = vec![
            pinned_session("s-pin", "p"),
            session("s-1", "a", "active", "working"),
            session("s-2", "b", "active", "working"),
        ];
        let flat = flatten(&sessions);
        let list = rows(&flat);
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
}
