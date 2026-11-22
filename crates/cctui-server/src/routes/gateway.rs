/// Resolve the database a DB-gated test should run against.
///
/// Locally a missing URL skips the test. Under CI it panics instead: a silent
/// skip there means the gate never ran and the suite passes green having
/// exercised none of the SQL.
#[cfg(test)]
pub fn test_db_url(test_name: &str) -> Option<String> {
    let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty());

    if url.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "{test_name}: DATABASE_URL/TEST_DATABASE_URL must point at a migrated database in CI"
        );
        eprintln!("skipping {test_name}: no DATABASE_URL/TEST_DATABASE_URL");
    }
    url
}
