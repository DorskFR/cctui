use axum::http::{HeaderMap, StatusCode};

#[tokio::test]
async fn static_route_serves_installed_plugins_only_while_enabled() {
    use axum::extract::{Path, State};
    let state = crate::state::AppState::for_test(
        sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap(),
    );
    let plugin = crate::plugin_archive::load_archive(
        &crate::plugin_archive::test_support::demo_tgz(None, "1.0.0"),
        true,
    )
    .unwrap();
    state.plugins.upsert_installed(plugin);
    let get = |path: &str| {
        super::static_file(
            State(state.clone()),
            Path(("demo".to_owned(), path.to_owned())),
            HeaderMap::new(),
        )
    };
    let resp = get("web/index.js").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/javascript; charset=utf-8");
    assert_eq!(get("skills/demo/SKILL.md").await.status(), StatusCode::OK);
    assert_eq!(get("../plugin.json").await.status(), StatusCode::NOT_FOUND);
    assert_eq!(get("web/nope.js").await.status(), StatusCode::NOT_FOUND);
    state.plugins.set_installed_enabled("demo", false);
    assert_eq!(get("web/index.js").await.status(), StatusCode::NOT_FOUND);
}
