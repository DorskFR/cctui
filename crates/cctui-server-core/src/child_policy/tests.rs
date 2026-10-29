use super::*;

fn cap(adapters: &[&str], max_budget: Option<f64>, max_children: Option<u32>) -> SpawnCapability {
    SpawnCapability {
        adapters: adapters.iter().map(|s| (*s).to_owned()).collect(),
        max_budget_usd: max_budget,
        max_children,
        ..SpawnCapability::default()
    }
}

fn usage(live_children: u32) -> Usage {
    Usage { live_children, ..Usage::default() }
}

fn req(adapter: &str, budget: Option<f64>) -> SpawnChildRequest {
    SpawnChildRequest {
        adapter: adapter.to_owned(),
        prompt: "review this".into(),
        budget_usd: budget,
        ..SpawnChildRequest::default()
    }
}

#[test]
fn no_capability_denies() {
    assert_eq!(authorize(None, &req("opencode", None), &usage(0)), Err(Denied::NoCapability));
}

#[test]
fn empty_adapter_list_denies_like_no_capability() {
    let cap = cap(&[], Some(1.0), None);
    assert_eq!(authorize(Some(&cap), &req("opencode", None), &usage(0)), Err(Denied::NoCapability));
}

#[test]
fn unlisted_adapter_denies_and_names_the_allowed_set() {
    let cap = cap(&["opencode"], Some(1.0), None);
    let err = authorize(Some(&cap), &req("claude-code", None), &usage(0)).unwrap_err();
    assert_eq!(
        err,
        Denied::Adapter { requested: "claude-code".into(), allowed: vec!["opencode".to_owned()] }
    );
    assert!(err.to_string().contains("opencode"));
}

#[test]
fn listed_adapter_allows_and_inherits_the_ceiling_budget() {
    let cap = cap(&["opencode", "codex"], Some(2.5), None);
    let ok = authorize(Some(&cap), &req("opencode", None), &usage(0)).unwrap();
    assert_eq!(
        ok,
        Authorized {
            adapter: "opencode".into(),
            budget_usd: Some(2.5),
            permission_mode: PermissionMode::Ask,
        }
    );
}

#[test]
fn budget_over_the_ceiling_denies() {
    let cap = cap(&["opencode"], Some(2.0), None);
    assert_eq!(
        authorize(Some(&cap), &req("opencode", Some(5.0)), &usage(0)),
        Err(Denied::Budget { requested: 5.0, max: 2.0 })
    );
}

#[test]
fn without_a_ceiling_a_named_budget_is_kept_and_none_stays_unlimited() {
    let cap = cap(&["opencode"], None, None);
    assert_eq!(
        authorize(Some(&cap), &req("opencode", Some(500.0)), &usage(0)).unwrap().budget_usd,
        Some(500.0)
    );
    assert_eq!(authorize(Some(&cap), &req("opencode", None), &usage(0)).unwrap().budget_usd, None);
}

#[test]
fn budget_at_the_ceiling_is_allowed() {
    let cap = cap(&["opencode"], Some(2.0), None);
    assert_eq!(
        authorize(Some(&cap), &req("opencode", Some(2.0)), &usage(0)).unwrap().budget_usd,
        Some(2.0)
    );
}

#[test]
fn nonpositive_or_nonfinite_budget_is_a_bad_request() {
    let cap = cap(&["opencode"], Some(2.0), None);
    assert!(matches!(
        authorize(Some(&cap), &req("opencode", Some(0.0)), &usage(0)),
        Err(Denied::BadRequest(_))
    ));
    assert!(matches!(
        authorize(Some(&cap), &req("opencode", Some(f64::NAN)), &usage(0)),
        Err(Denied::BadRequest(_))
    ));
}

#[test]
fn child_cap_denies_once_reached() {
    let cap = cap(&["opencode"], Some(1.0), Some(2));
    assert!(authorize(Some(&cap), &req("opencode", None), &usage(1)).is_ok());
    assert_eq!(
        authorize(Some(&cap), &req("opencode", None), &usage(2)),
        Err(Denied::TooManyChildren { max: 2 })
    );
}

#[test]
fn machine_default_authorizes_every_default_on_adapter() {
    let cap = SpawnCapability::machine_default();
    for adapter in cctui_proto::adapter::default_enabled_adapters() {
        let ok = authorize(Some(&cap), &req(adapter, None), &usage(0)).unwrap();
        assert_eq!(
            ok,
            Authorized {
                adapter: adapter.to_owned(),
                budget_usd: None,
                permission_mode: PermissionMode::Ask,
            }
        );
    }
    assert_eq!(
        authorize(Some(&cap), &req("claude-code", Some(1.5)), &usage(0)).unwrap().budget_usd,
        Some(1.5)
    );
    assert_eq!(
        authorize(Some(&cap), &req("claude-code", Some(1_000.0)), &usage(0)).unwrap().budget_usd,
        Some(1_000.0),
        "the default grant has no dollar ceiling"
    );
}

/// The ceiling a child is handed is never larger than the budget it was
/// itself granted, so a spawn tree cannot outgrow its root.
#[test]
fn an_inherited_ceiling_only_shrinks_down_the_tree() {
    let root = cap(&["claude-code"], Some(20.0), Some(3));
    let child = root.inherited("root", Some(5.0), None);
    assert_eq!(child.max_budget_usd, Some(5.0));
    assert_eq!(child.adapters, root.adapters);
    assert_eq!(child.max_children, Some(3));

    let grandchild = child.inherited("root", Some(50.0), None);
    assert_eq!(
        grandchild.max_budget_usd,
        Some(5.0),
        "a child cannot hand a descendant more than its own ceiling"
    );

    assert_eq!(root.inherited("root", None, None).max_budget_usd, Some(20.0));
    assert_eq!(
        cap(&["codex"], None, None).inherited("root", Some(2.0), None).max_budget_usd,
        Some(2.0)
    );
    assert_eq!(cap(&["codex"], None, None).inherited("root", None, None).max_budget_usd, None);
}

/// A child spawned under the default grant inherits the budget it was
/// actually given, not the unclamped default.
#[test]
fn a_child_of_the_default_grant_inherits_its_own_budget() {
    let root = SpawnCapability::machine_default();
    let granted = authorize(Some(&root), &req("claude-code", Some(1.5)), &usage(0)).unwrap();
    let child = root.inherited("root", granted.budget_usd, None);
    assert_eq!(child.max_budget_usd, Some(1.5));
    assert!(
        authorize(Some(&child), &req("claude-code", Some(2.0)), &usage(0)).is_err(),
        "the grandchild request must not exceed the child's inherited ceiling"
    );
    assert_eq!(
        authorize(Some(&child), &req("claude-code", None), &usage(0)).unwrap().budget_usd,
        Some(1.5)
    );
}

#[test]
fn empty_prompt_or_adapter_is_a_bad_request() {
    let cap = cap(&["opencode"], Some(1.0), None);
    let mut blank_prompt = req("opencode", None);
    blank_prompt.prompt = "  ".into();
    assert!(matches!(authorize(Some(&cap), &blank_prompt, &usage(0)), Err(Denied::BadRequest(_))));
    assert!(matches!(
        authorize(Some(&cap), &req("  ", None), &usage(0)),
        Err(Denied::BadRequest(_))
    ));
}

#[test]
fn a_parent_in_ask_mode_cannot_mint_a_yolo_child() {
    let cap = cap(&["claude-code"], Some(1.0), None);
    let mut r = req("claude-code", None);
    r.permission_mode = Some(PermissionMode::Yolo);
    let ask = Usage { parent_mode: Some(PermissionMode::Ask), ..Usage::default() };
    assert_eq!(
        authorize(Some(&cap), &r, &ask),
        Err(Denied::PermissionMode { requested: PermissionMode::Yolo, max: PermissionMode::Ask })
    );
    r.permission_mode = Some(PermissionMode::Whip);
    assert!(matches!(authorize(Some(&cap), &r, &ask), Err(Denied::PermissionMode { .. })));

    let ceiling = SpawnCapability { max_permission_mode: Some(PermissionMode::Auto), ..cap };
    r.permission_mode = Some(PermissionMode::Yolo);
    let yolo = Usage { parent_mode: Some(PermissionMode::Yolo), ..Usage::default() };
    assert!(
        matches!(authorize(Some(&ceiling), &r, &yolo), Err(Denied::PermissionMode { .. })),
        "the capability ceiling holds even when the parent was toggled to yolo"
    );
    r.permission_mode = Some(PermissionMode::Ask);
    assert_eq!(authorize(Some(&ceiling), &r, &yolo).unwrap().permission_mode, PermissionMode::Ask);
}

#[test]
fn a_child_naming_no_mode_inherits_the_parents() {
    let cap = cap(&["claude-code"], Some(1.0), None);
    for mode in [PermissionMode::Ask, PermissionMode::Auto, PermissionMode::Yolo] {
        let u = Usage { parent_mode: Some(mode), ..Usage::default() };
        assert_eq!(
            authorize(Some(&cap), &req("claude-code", None), &u).unwrap().permission_mode,
            mode
        );
    }
    let stored = SpawnCapability { max_permission_mode: Some(PermissionMode::Auto), ..cap };
    assert_eq!(
        authorize(Some(&stored), &req("claude-code", None), &usage(0)).unwrap().permission_mode,
        PermissionMode::Auto,
        "with no live mode the stored ceiling is the parent's posture"
    );
    let child = stored.inherited("root", Some(1.0), Some(PermissionMode::Ask));
    assert_eq!(child.max_permission_mode, Some(PermissionMode::Ask));
}

#[test]
fn session_labels_parse_to_postures() {
    assert_eq!(PermissionMode::from_session_label("default"), Some(PermissionMode::Ask));
    assert_eq!(PermissionMode::from_session_label("plan"), Some(PermissionMode::Ask));
    assert_eq!(PermissionMode::from_session_label("acceptEdits"), Some(PermissionMode::Auto));
    assert_eq!(PermissionMode::from_session_label("bypassPermissions"), Some(PermissionMode::Yolo));
    assert_eq!(PermissionMode::from_session_label("yolo"), Some(PermissionMode::Yolo));
    assert_eq!(PermissionMode::from_session_label("nonsense"), None);
}

#[test]
fn spawning_past_max_depth_is_denied() {
    let root = SpawnCapability { max_depth: Some(2), ..cap(&["codex"], Some(1.0), None) };
    assert!(authorize(Some(&root), &req("codex", None), &usage(0)).is_ok());
    let child = root.inherited("root", Some(1.0), None);
    assert_eq!(child.max_depth, Some(1));
    assert!(authorize(Some(&child), &req("codex", None), &usage(0)).is_ok());
    let grandchild = child.inherited("child", Some(1.0), None);
    assert_eq!(grandchild.max_depth, Some(0));
    assert_eq!(authorize(Some(&grandchild), &req("codex", None), &usage(0)), Err(Denied::Depth));
    assert_eq!(grandchild.tree_root.as_deref(), Some("root"), "the root id carries down");
}

#[test]
fn machine_default_caps_depth_only() {
    let root = SpawnCapability::machine_default();
    assert_eq!(root.max_depth, Some(cctui_proto::api::DEFAULT_MAX_DEPTH));
    assert_eq!(root.max_children, None);
    assert_eq!(root.max_budget_usd, None);
    assert_eq!(root.max_tree_budget_usd, None);
    const { assert!(cctui_proto::api::DEFAULT_MAX_DEPTH >= 2) };
}

/// Under the default grant no generation meets a dollar or child-count
/// limit: a busy parent and a heavily spent tree still spawn uncapped
/// children, and the reservation draws nothing from the tree.
#[test]
fn the_default_tree_is_unlimited_at_every_generation() {
    let busy = Usage { live_children: 10_000, tree_granted_usd: 1e9, parent_mode: None };
    let mut node = SpawnCapability::machine_default();
    let mut id = "root".to_owned();
    for generation in 1..=cctui_proto::api::DEFAULT_MAX_DEPTH {
        let a = authorize(Some(&node), &req("claude-code", None), &busy).unwrap();
        assert_eq!(a.budget_usd, None, "generation {generation} is uncapped");
        assert!(tree_grant_usd(Some(&node), &a).abs() < f64::EPSILON);
        node = node.inherited(&id, a.budget_usd, Some(a.permission_mode));
        assert_eq!(
            (node.max_budget_usd, node.max_children, node.max_tree_budget_usd),
            (None, None, None),
            "generation {generation} inherits no ceiling"
        );
        id = format!("gen-{generation}");
    }
}

#[test]
fn only_a_capped_tree_draws_from_its_budget() {
    let a = Authorized {
        adapter: "codex".into(),
        budget_usd: Some(2.0),
        permission_mode: PermissionMode::Ask,
    };
    let capped = SpawnCapability { max_tree_budget_usd: Some(10.0), ..cap(&["codex"], None, None) };
    assert!((tree_grant_usd(Some(&capped), &a) - 2.0).abs() < f64::EPSILON);
    assert!(tree_grant_usd(Some(&cap(&["codex"], None, None)), &a).abs() < f64::EPSILON);
    assert!(tree_grant_usd(None, &a).abs() < f64::EPSILON);
}

#[test]
fn with_no_mode_information_the_ceiling_is_ask() {
    let root = SpawnCapability::machine_default();
    let mut r = req("claude-code", None);
    assert_eq!(authorize(Some(&root), &r, &usage(0)).unwrap().permission_mode, PermissionMode::Ask);
    r.permission_mode = Some(PermissionMode::Yolo);
    assert!(matches!(
        authorize(Some(&root), &r, &usage(0)),
        Err(Denied::PermissionMode { max: PermissionMode::Ask, .. })
    ));
}

/// A session launched in yolo is stamped with a yolo ceiling at spawn time,
/// and its tree keeps spawning yolo descendants even when no session row
/// ever reports a live mode.
#[test]
fn a_yolo_launched_tree_spawns_yolo_children_and_grandchildren() {
    let root = SpawnCapability {
        max_permission_mode: Some(PermissionMode::Yolo),
        ..SpawnCapability::machine_default()
    };
    let unknown = usage(0);
    let mut explicit = req("claude-code", None);
    explicit.permission_mode = Some(PermissionMode::Yolo);
    let implicit = req("codex", None);

    let mut node = root;
    let mut id = "root".to_owned();
    for generation in 1..=cctui_proto::api::DEFAULT_MAX_DEPTH {
        let a = authorize(Some(&node), &explicit, &unknown).unwrap();
        assert_eq!(a.permission_mode, PermissionMode::Yolo, "generation {generation}");
        let b = authorize(Some(&node), &implicit, &unknown).unwrap();
        assert_eq!(b.permission_mode, PermissionMode::Yolo, "an omitted mode inherits yolo");
        node = node.inherited(&id, a.budget_usd, Some(a.permission_mode));
        assert_eq!(node.max_permission_mode, Some(PermissionMode::Yolo));
        id = format!("gen-{generation}");
    }
    assert_eq!(authorize(Some(&node), &explicit, &unknown), Err(Denied::Depth));

    let reported_yolo = Usage { parent_mode: Some(PermissionMode::Yolo), ..Usage::default() };
    let child = SpawnCapability::machine_default().inherited("r", None, Some(PermissionMode::Yolo));
    assert_eq!(
        authorize(Some(&child), &explicit, &reported_yolo).unwrap().permission_mode,
        PermissionMode::Yolo
    );
    let toggled_to_ask = Usage { parent_mode: Some(PermissionMode::Ask), ..Usage::default() };
    assert!(
        authorize(Some(&child), &explicit, &toggled_to_ask).is_err(),
        "a yolo tree whose parent was switched to ask stops minting yolo children"
    );
}

#[test]
fn granted_budgets_under_one_root_never_exceed_the_tree_ceiling() {
    let root =
        SpawnCapability { max_tree_budget_usd: Some(10.0), ..cap(&["codex"], Some(4.0), None) };
    let mut granted = 0.0;
    let mut denied = false;
    for i in 0..10 {
        let parent =
            if i % 2 == 0 { root.clone() } else { root.inherited("root", Some(4.0), None) };
        let u = Usage { tree_granted_usd: granted, ..Usage::default() };
        match authorize(Some(&parent), &req("codex", None), &u) {
            Ok(a) => granted += a.budget_usd.unwrap(),
            Err(Denied::TreeBudget { .. }) => denied = true,
            Err(e) => panic!("unexpected denial {e}"),
        }
        assert!(granted <= 10.0, "granted {granted} past the tree ceiling");
    }
    assert!(denied);
    assert!((granted - 10.0).abs() < f64::EPSILON, "the last grant is clamped to the remainder");

    let u = Usage { tree_granted_usd: 8.0, ..Usage::default() };
    assert_eq!(
        authorize(Some(&root), &req("codex", Some(3.0)), &u),
        Err(Denied::TreeBudget { requested: 3.0, remaining: 2.0 })
    );
}

async fn execution_fixture(pool: &sqlx::PgPool) {
    let mut conn = crate::schema::migration_connection(pool).await.unwrap();
    crate::schema::provision(&mut conn).await.unwrap();
    drop(conn);
    sqlx::query("INSERT INTO sessions (id, parent_id, machine_id, working_dir) VALUES ('parent', NULL, 'fixture', '/work'), ('running', 'parent', 'fixture', '/work')")
        .execute(pool).await.unwrap();
}

#[tokio::test]
async fn execution_children_enforce_slots_without_transcript_storage() {
    crate::test_support::with_isolated_database("child_execution_slots", |pool| async move {
        execution_fixture(&pool).await;
        let grant = cap(&["codex"], None, Some(1));
        for end in [None, Some("completed")] {
            sqlx::query("UPDATE sessions SET end_reason=$1 WHERE id='running'")
                .bind(end).execute(&pool).await.unwrap();
            let result = reserve_child(&ExecutionChildCount, &pool, "parent", Some(&grant), &req("codex", None), usage(0), "replacement").await;
            assert!(matches!(result, Err(AppError::Status(StatusCode::FORBIDDEN, _))), "a running or completed child occupies its slot: {result:?}");
        }
        sqlx::query("UPDATE sessions SET end_reason='crashed' WHERE id='running'").execute(&pool).await.unwrap();
        sqlx::query(r#"INSERT INTO sessions (id, parent_id, machine_id, working_dir, metadata, status) VALUES ('fork', 'parent', 'fixture', '/work', '{"relation":"fork"}', 'active'), ('followup', 'parent', 'fixture', '/work', '{"relation":"followup"}', 'active'), ('archived', 'parent', 'fixture', '/work', '{}', 'archived')"#).execute(&pool).await.unwrap();
        reserve_child(&ExecutionChildCount, &pool, "parent", Some(&grant), &req("codex", None), usage(0), "replacement").await.unwrap();
        assert_eq!(pending_child_count(&pool, "parent").await.unwrap(), 1);
        assert!(crate::store::spawn_capabilities::get(&pool, "replacement").await.unwrap().is_some());
    }).await;
}

#[tokio::test]
async fn concurrent_reservations_cannot_take_the_same_execution_slot() {
    crate::test_support::with_isolated_database("child_execution_concurrency", |pool| async move {
        execution_fixture(&pool).await;
        sqlx::query("DELETE FROM sessions WHERE id='running'").execute(&pool).await.unwrap();
        let grant = cap(&["codex"], None, Some(1));
        let request = req("codex", None);
        let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(
                reserve_child(
                    &ExecutionChildCount,
                    &pool,
                    "parent",
                    Some(&grant),
                    &request,
                    usage(0),
                    "child-a"
                ),
                reserve_child(
                    &ExecutionChildCount,
                    &pool,
                    "parent",
                    Some(&grant),
                    &request,
                    usage(0),
                    "child-b"
                ),
            )
        })
        .await
        .unwrap();
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let denied = if a.is_err() { a } else { b };
        assert!(matches!(denied, Err(AppError::Status(StatusCode::FORBIDDEN, _))));
        assert_eq!(pending_child_count(&pool, "parent").await.unwrap(), 1);
    })
    .await;
}

#[tokio::test]
async fn execution_tree_budgets_and_release_survive_without_workspace_tables() {
    crate::test_support::with_isolated_database("child_execution_tree_budget", |pool| async move {
        execution_fixture(&pool).await;
        let grant =
            SpawnCapability { max_tree_budget_usd: Some(5.0), ..cap(&["codex"], Some(3.0), None) };
        let (first, inherited) = reserve_child(
            &ExecutionChildCount,
            &pool,
            "parent",
            Some(&grant),
            &req("codex", None),
            usage(0),
            "child",
        )
        .await
        .unwrap();
        assert_eq!(first.budget_usd, Some(3.0));
        let (second, _) = reserve_child(
            &ExecutionChildCount,
            &pool,
            "child",
            Some(&inherited),
            &req("codex", None),
            usage(0),
            "grandchild",
        )
        .await
        .unwrap();
        assert_eq!(second.budget_usd, Some(2.0));
        let denied = reserve_child(
            &ExecutionChildCount,
            &pool,
            "parent",
            Some(&grant),
            &req("codex", None),
            usage(0),
            "over-budget",
        )
        .await;
        assert!(matches!(denied, Err(AppError::Status(StatusCode::FORBIDDEN, _))));
        release_child(&pool, "grandchild").await;
        assert!(
            crate::store::spawn_capabilities::get(&pool, "grandchild").await.unwrap().is_none()
        );
        let (replacement, _) = reserve_child(
            &ExecutionChildCount,
            &pool,
            "parent",
            Some(&grant),
            &req("codex", None),
            usage(0),
            "replacement",
        )
        .await
        .unwrap();
        assert_eq!(replacement.budget_usd, Some(2.0));
        assert!((tree_granted_usd(&pool, "parent").await.unwrap() - 5.0).abs() < f64::EPSILON);
    })
    .await;
}

struct UnavailableCount;
impl ChildCount for UnavailableCount {
    async fn live_child_count(
        &self,
        _: &mut sqlx::PgConnection,
        _: &str,
    ) -> Result<u32, sqlx::Error> {
        Err(sqlx::Error::Protocol("fixture unavailable".into()))
    }
}
#[tokio::test]
async fn an_unreadable_child_count_cannot_mint_a_capability_or_reservation() {
    crate::test_support::with_isolated_database(
        "child_execution_count_failure",
        |pool| async move {
            execution_fixture(&pool).await;
            let grant = cap(&["codex"], None, None);
            let denied = reserve_child(
                &UnavailableCount,
                &pool,
                "parent",
                Some(&grant),
                &req("codex", None),
                usage(0),
                "denied",
            )
            .await;
            assert!(matches!(denied, Err(AppError::Db(_))));
            assert_eq!(pending_child_count(&pool, "parent").await.unwrap(), 0);
            assert!(
                crate::store::spawn_capabilities::get(&pool, "denied").await.unwrap().is_none()
            );
            assert!(tree_granted_usd(&pool, "parent").await.unwrap().abs() < f64::EPSILON);
        },
    )
    .await;
}
