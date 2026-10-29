pub(crate) use cctui_server_core::live_sessions_predicate;

#[cfg(test)]
mod tests {
    use super::live_sessions_predicate;

    #[test]
    fn predicate_matches_the_partial_index_in_migration_117() {
        let migration = include_str!("../../../migrations/117_sessions_live.up.sql");
        assert!(
            migration.contains(&format!("WHERE {}", live_sessions_predicate!())),
            "migration 117's index predicate drifted from live_sessions_predicate!"
        );
    }

    #[test]
    fn predicate_excludes_exactly_the_archived_status() {
        use crate::store::sessions::SessionRowStatus;
        assert_eq!(
            live_sessions_predicate!(),
            format!("status <> '{}'", SessionRowStatus::Archived.as_str())
        );
    }

    #[test]
    fn qualified_form_prefixes_the_alias() {
        assert_eq!(live_sessions_predicate!("s"), "s.status <> 'archived'");
    }
}
