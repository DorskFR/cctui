//! Database-gated tests require an already migrated database in CI.

pub fn db_url(test_name: &str) -> Option<String> {
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
