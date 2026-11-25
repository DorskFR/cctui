use super::{install, load_all, set_enabled, sync, uninstall};
use cctui_server_extras::plugin_archive::test_support::plugin_tgz;
use cctui_server_extras::plugins::PluginRegistry;

/// Whether `registry` holds the plugin `id`, regardless of what other tests
/// have left in the shared `plugins` table.
fn knows(registry: &PluginRegistry, id: &str) -> bool {
    registry.all_admin().iter().any(|p| p.manifest.id == id)
}

#[tokio::test]
async fn install_upgrade_toggle_and_uninstall_round_trip() {
    const ID: &str = "plugin-store-round-trip";
    let Some(url) = crate::routes::gateway::test_db_url("plugin_store_round_trip") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    let registry = PluginRegistry::disabled();
    sqlx::query("DELETE FROM plugins WHERE id = $1").bind(ID).execute(&pool).await.unwrap();

    let v1 = plugin_tgz(ID, None, "1.0.0");
    let plugin = install(&pool, &registry, &v1, None).await.unwrap();
    assert_eq!(plugin.manifest.version, "1.0.0");
    assert!(!plugin.instance_enabled);
    assert!(registry.get(ID).is_none(), "disabled plugins are hidden from users");
    assert_eq!(registry.all_admin().iter().filter(|p| p.manifest.id == ID).count(), 1);

    assert!(set_enabled(&pool, &registry, ID, true).await.unwrap());
    assert!(registry.get(ID).is_some());
    assert!(!set_enabled(&pool, &registry, "nope", true).await.unwrap());

    let upgraded =
        install(&pool, &registry, &plugin_tgz(ID, Some(ID), "1.1.0"), None).await.unwrap();
    assert_eq!(upgraded.manifest.version, "1.1.0");
    assert!(upgraded.instance_enabled, "upgrade keeps the instance toggle");
    assert_eq!(registry.get(ID).unwrap().manifest.version, "1.1.0");

    let fresh = PluginRegistry::disabled();
    fresh.set_installed(load_all(&pool).await.unwrap());
    let loaded = fresh.get(ID).unwrap();
    assert_eq!(loaded.manifest.version, "1.1.0");
    assert_eq!(
        cctui_server_extras::plugins::resolve_static(&loaded, &format!("skills/{ID}/SKILL.md"))
            .unwrap(),
        b"# demo"
    );

    let (hash,): (String,) = sqlx::query_as("SELECT archive_hash FROM plugins WHERE id = $1")
        .bind(ID)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(uninstall(&pool, &registry, ID).await.unwrap());
    assert!(!uninstall(&pool, &registry, ID).await.unwrap());
    assert!(!knows(&registry, ID));
    let (blobs,): (i64,) = sqlx::query_as("SELECT count(*) FROM daemon_blobs WHERE hash = $1")
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(blobs, 0);
}

#[tokio::test]
async fn a_replica_picks_up_changes_made_through_another() {
    const ID: &str = "plugin-store-replicas";
    let Some(url) = crate::routes::gateway::test_db_url("plugin_store_replicas") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    sqlx::query("DELETE FROM plugins WHERE id = $1").bind(ID).execute(&pool).await.unwrap();
    let (a, b) = (PluginRegistry::disabled(), PluginRegistry::disabled());
    sync(&pool, &b).await.unwrap();

    install(&pool, &a, &plugin_tgz(ID, None, "1.0.0"), None).await.unwrap();
    assert!(!knows(&b, ID), "b has not synced since the install");
    sync(&pool, &b).await.unwrap();
    assert!(knows(&b, ID));
    assert!(b.get(ID).is_none(), "still disabled instance-wide");

    set_enabled(&pool, &a, ID, true).await.unwrap();
    sync(&pool, &b).await.unwrap();
    assert!(b.get(ID).is_some());

    install(&pool, &a, &plugin_tgz(ID, Some(ID), "1.1.0"), None).await.unwrap();
    sync(&pool, &b).await.unwrap();
    assert_eq!(b.get(ID).unwrap().manifest.version, "1.1.0");

    uninstall(&pool, &a, ID).await.unwrap();
    sync(&pool, &b).await.unwrap();
    assert!(!knows(&b, ID));
}
