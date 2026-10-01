//! The access panel's row logic, shared with the web UI's `access.logic.ts`.

/// The scope set a key may be granted, in the order both clients list it.
pub const ALL_SCOPES: [&str; 4] = ["read", "dispatch", "enroll", "admin"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeCell {
    pub name: &'static str,
    pub granted: bool,
}

#[must_use]
pub fn scope_cells(granted: &[String]) -> Vec<ScopeCell> {
    ALL_SCOPES
        .iter()
        .map(|name| ScopeCell { name, granted: granted.iter().any(|g| g == name) })
        .collect()
}

/// Which rows to draw, as indices into the server's order: the live ones, then
/// the revoked ones when they are being shown at all.
#[must_use]
pub fn visible_order(revoked: &[bool], show_revoked: bool) -> Vec<usize> {
    let live = revoked.iter().enumerate().filter(|(_, r)| !**r).map(|(i, _)| i);
    if !show_revoked {
        return live.collect();
    }
    let dead = revoked.iter().enumerate().filter(|(_, r)| **r).map(|(i, _)| i);
    live.chain(dead).collect()
}

/// Case-insensitive substring filter over names, as indices. An empty query
/// keeps everything.
#[must_use]
pub fn filter_by_name(names: &[String], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    names
        .iter()
        .enumerate()
        .filter(|(_, name)| q.is_empty() || name.to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// The glyph a key's kind gets. `machine` keys belong to a daemon, everything
/// else to a person.
#[must_use]
pub fn key_icon(kind: &str) -> &'static str {
    if kind == "machine" { "tv" } else { "user" }
}

#[cfg(test)]
mod tests {
    use super::{ALL_SCOPES, filter_by_name, key_icon, scope_cells, visible_order};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn scope_cells_cover_every_scope_in_a_fixed_order() {
        let cells = scope_cells(&names(&["admin", "read"]));
        assert_eq!(cells.iter().map(|c| c.name).collect::<Vec<_>>(), ALL_SCOPES.to_vec());
        assert_eq!(
            cells.iter().filter(|c| c.granted).map(|c| c.name).collect::<Vec<_>>(),
            vec!["read", "admin"]
        );
    }

    #[test]
    fn an_unknown_granted_scope_grants_nothing_it_cannot_show() {
        assert!(scope_cells(&names(&["nonsense"])).iter().all(|c| !c.granted));
    }

    #[test]
    fn revoked_rows_sort_last_and_hide_by_default() {
        let revoked = [false, true, false];
        assert_eq!(visible_order(&revoked, false), vec![0, 2]);
        assert_eq!(visible_order(&revoked, true), vec![0, 2, 1]);
    }

    #[test]
    fn the_name_filter_ignores_case_and_surrounding_space() {
        let rows = names(&["dorsk", "Nanachi", "bot"]);
        assert_eq!(filter_by_name(&rows, "  NA "), vec![1]);
        assert_eq!(filter_by_name(&rows, ""), vec![0, 1, 2]);
        assert_eq!(filter_by_name(&rows, "zzz"), Vec::<usize>::new());
    }

    #[test]
    fn a_machine_key_is_a_screen_and_everything_else_a_person() {
        assert_eq!(key_icon("machine"), "tv");
        assert_eq!(key_icon("user"), "user");
    }
}
