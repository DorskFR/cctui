use super::*;
use cctui_server_sessions::routes::daemon::test_support::{
    drop_machines, seed_machine, seed_owned_session, seed_session,
};

#[test]
fn ungranted_sessions_spawn_up_to_their_live_mode() {
    use cctui_proto::adapter::PermissionMode;
    use cctui_server_dispatch::child_policy::{Usage, authorize};
    let cap = cctui_proto::api::SpawnCapability::machine_default();
    let req = |m| cctui_proto::api::SpawnChildRequest {
        adapter: "codex".into(),
        prompt: "go".into(),
        permission_mode: Some(m),
        ..Default::default()
    };
    let usage = |p| Usage { parent_mode: Some(p), ..Usage::default() };

    let a = authorize(Some(&cap), &req(PermissionMode::Yolo), &usage(PermissionMode::Yolo))
        .expect("yolo parent spawns yolo");
    assert_eq!(a.permission_mode, PermissionMode::Yolo);

    assert!(
        authorize(Some(&cap), &req(PermissionMode::Yolo), &usage(PermissionMode::Ask)).is_err()
    );
    let a = authorize(Some(&cap), &req(PermissionMode::Ask), &usage(PermissionMode::Ask))
        .expect("ask parent spawns ask");
    assert_eq!(a.permission_mode, PermissionMode::Ask);
}

/// A session launched without a capability gets the default grant, except
/// one whose owner has a dispatch policy: it may spawn no children.
#[tokio::test]
async fn the_default_grant_spawns_no_children_for_a_policy_bound_owner() {
    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_policy_default") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let state = AppState::for_test(pool.clone());
    let default = crate::routes::server_settings::spawn_default_capability(&state).await;
    for bound in [false, true] {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("policy-default-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        if bound {
            let rules = cctui_proto::api::admin::DispatchPolicyRules::default();
            cctui_server_dispatch::store::dispatch_policies::upsert(&pool, uid, &rules)
                .await
                .unwrap();
        }
        let sid = format!("policy-default-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO auth_keys (user_id, key_hash, kind, session_id, expires_at) \
             VALUES ($1, $2, 'ephemeral', $3, now() + interval '1 hour')",
        )
        .bind(uid)
        .bind(format!("eph-{sid}"))
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();

        let cap = spawn_capability_for(&state, &sid).await.expect("granted");
        let max_children = if bound { Some(0) } else { default.max_children };
        assert_eq!(
            cap,
            cctui_proto::api::SpawnCapability { max_children, ..default.clone() },
            "policy-bound: {bound}"
        );

        sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM auth_keys WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}

#[tokio::test]
async fn gateway_env_is_refused_for_sessions_the_machine_does_not_own() {
    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_owner") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (ua, ma) = seed_machine(&pool, "owner").await;
    let (ub, mb) = seed_machine(&pool, "intruder").await;
    let ma2 = {
        let mid = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(ua)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed second machine");
        mid
    };
    let owned = seed_owned_session(&pool, ua, ma).await;
    let (ownerless_user, ownerless) = seed_session(&pool, "claude-code", "anthropic").await;
    sqlx::query("UPDATE sessions SET user_id = NULL WHERE id = $1")
        .bind(&ownerless)
        .execute(&pool)
        .await
        .expect("clear owner");

    let allowed = async |user, machine, sid: &str| {
        super::gateway_env_allowed(&pool, user, machine, sid).await.expect("lookup")
    };
    assert!(allowed(ua, ma, &owned).await);
    assert!(allowed(ua, ma, "not-registered-yet").await);
    assert!(!allowed(ub, mb, &owned).await, "another user's session is refused");
    assert!(!allowed(ua, ma2, &owned).await, "another machine's session is refused");
    assert!(!allowed(ub, mb, &ownerless).await, "a NULL-owner session is refused");
    assert!(!allowed(ua, ma, &ownerless).await);

    // An unregistered id whose token was already minted for someone else is
    // that user's session: a machine key that merely knows the id is not
    // entitled to its credential.
    let claimed = format!("ses-claimed-{}", Uuid::new_v4());
    let acct: Uuid =
        sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
            .bind(ua)
            .bind(format!("acct-{claimed}"))
            .fetch_one(&pool)
            .await
            .expect("seed account");
    let prov: Uuid = sqlx::query_scalar(
        "INSERT INTO account_providers (user_id, account_id, provider) \
         VALUES ($1, $2, 'anthropic') RETURNING id",
    )
    .bind(ua)
    .bind(acct)
    .fetch_one(&pool)
    .await
    .expect("seed provider");
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id, user_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(format!("kh-{claimed}"))
    .bind(&claimed)
    .bind(prov)
    .bind(ua)
    .execute(&pool)
    .await
    .expect("seed token");
    assert!(allowed(ua, ma, &claimed).await, "the token's own user is let through");
    assert!(
        !allowed(ub, mb, &claimed).await,
        "an unregistered id already claimed by another user's token is refused"
    );
    sqlx::query("DELETE FROM accounts WHERE id = $1").bind(acct).execute(&pool).await.ok();

    // Archiving is an explicit park: un-archiving is a user action, so an
    // archived session must not re-mint behind the user's back.
    sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
        .bind(&owned)
        .execute(&pool)
        .await
        .expect("archive");
    assert!(!allowed(ua, ma, &owned).await, "an archived session mints nothing");
    sqlx::query("UPDATE sessions SET status = 'ended' WHERE id = $1")
        .bind(&owned)
        .execute(&pool)
        .await
        .expect("end");
    assert!(allowed(ua, ma, &owned).await, "an ended session is still relaunchable");

    sqlx::query("DELETE FROM machines WHERE id = $1").bind(ma2).execute(&pool).await.ok();
    drop_machines(&pool, &[owned, ownerless], &[(ua, ma), (ub, mb)]).await;
    sqlx::query("DELETE FROM users WHERE id = $1").bind(ownerless_user).execute(&pool).await.ok();

    pool.close().await;
    assert!(
        super::gateway_env_allowed(&pool, ua, ma, "any").await.is_err(),
        "a lookup failure surfaces as an error (500), never as allowed",
    );
}

#[tokio::test]
async fn a_launch_key_context_is_readable_only_by_its_owner() {
    let Some(url) = cctui_server_kernel::test_support::db_url("launch_context_owner") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (ua, ma) = seed_machine(&pool, "ctx-owner").await;
    let (ub, mb) = seed_machine(&pool, "ctx-intruder").await;
    let owned = seed_owned_session(&pool, ua, ma).await;
    let key = format!("cmd-{}", Uuid::new_v4());
    let scope = cctui_server_sessions::routes::context::StoredScope {
        user_id: ua,
        coords: cctui_server_sessions::routes::context::SpawnScope::default(),
        dispatched: false,
    };
    cctui_server_sessions::routes::context::remember_intent(&pool, &key, &[], &scope).await;

    let allowed = async |user, machine, sid: &str| {
        super::owns_launch(&AppState::for_test(pool.clone()), user, machine, sid)
            .await
            .expect("lookup")
    };
    assert!(allowed(ua, ma, &owned).await);
    assert!(!allowed(ub, mb, &owned).await);
    assert!(allowed(ua, ma, &key).await, "the fork's own launch key");
    assert!(!allowed(ub, mb, &key).await, "another user's launch key");
    assert!(!allowed(ua, ma, "never-recorded").await, "an unknown key reads nothing");

    sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
        .bind(&key)
        .execute(&pool)
        .await
        .ok();
    drop_machines(&pool, &[owned], &[(ua, ma), (ub, mb)]).await;
}

/// The gateway-env pull serves the same context as the context pull, so it
/// is gated the same way: a launch key recorded for another user yields no
/// rule or memory bodies, even though the key itself has no session row.
#[tokio::test]
async fn gateway_env_context_is_withheld_from_another_users_launch_key() {
    use cctui_server_sessions::routes::context::{ContextItemSpec, clean_spec, insert};

    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_context_owner") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (ua, ma) = seed_machine(&pool, "genv-ctx-owner").await;
    let (ub, mb) = seed_machine(&pool, "genv-ctx-intruder").await;
    let memory = clean_spec(ContextItemSpec {
        kind: "memory".into(),
        name: "deploy".into(),
        title: "Deploy".into(),
        body: "private runbook".into(),
        enabled: true,
        ..ContextItemSpec::default()
    })
    .expect("valid memory");
    let item = insert(&pool, ua, &memory, None, None).await.expect("memory");
    let key = format!("cmd-{}", Uuid::new_v4());
    let scope = cctui_server_sessions::routes::context::StoredScope {
        user_id: ua,
        coords: cctui_server_sessions::routes::context::SpawnScope::default(),
        dispatched: false,
    };
    cctui_server_sessions::routes::context::remember_intent(
        &pool,
        &key,
        std::slice::from_ref(&item),
        &scope,
    )
    .await;

    let state = AppState::for_test(pool.clone());
    let mine = super::launch_context(&state, ua, ma, &key, true).await.expect("lookup");
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].name.as_str(), mine[0].body.as_str()), ("deploy", "private runbook"));
    let theirs = super::launch_context(&state, ub, mb, &key, true).await.expect("lookup");
    assert!(theirs.is_empty(), "another user's machine key reads no context: {theirs:?}");

    sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
        .bind(&key)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM context_items WHERE id = $1").bind(item.id).execute(&pool).await.ok();
    drop_machines(&pool, &[], &[(ua, ma), (ub, mb)]).await;
}

/// The sequential chain `gateway_env_for` replaced, kept as the oracle.
async fn sequential_gateway_env(
    state: &AppState,
    user_id: Uuid,
    machine_id: Uuid,
    session_id: &str,
) -> cctui_proto::api::GatewayEnvResponse {
    use cctui_server_accounts::routes::gateway::{
        mint_session_env_for_account, resolve_session_accounts, resolve_session_settings,
    };
    assert!(gateway_env_allowed(&state.pool, user_id, machine_id, session_id).await.unwrap());
    let user_settings = user_settings_of(state, user_id).await;
    let whip_phrases = user_settings
        .as_ref()
        .and_then(cctui_server_sessions::routes::settings::whip_stop_phrases_of);
    cctui_server_extras::plugin_store::sync_or_warn(&state.pool, &state.plugins, state).await;
    let plugins =
        cctui_server_extras::plugins::session_plugins(&state.plugins, user_settings.as_ref());
    let accounts = resolve_session_accounts(state, session_id).await;
    let mut env = std::collections::BTreeMap::new();
    let mut settings = None;
    if !accounts.is_empty() {
        for account in &accounts {
            if let Some(e) =
                mint_session_env_for_account(state, *account, session_id).await.unwrap()
            {
                env.extend(e);
            }
        }
        settings = resolve_session_settings(state, session_id).await;
        let adapter = cctui_server_sessions::store::sessions::adapter_id(&state.pool, session_id)
            .await
            .unwrap()
            .unwrap_or_default();
        if cctui_server_kernel::execution_settings::adapter_takes_service_tier(&adapter) {
            settings = cctui_server_kernel::execution_settings::overlay_service_tier(settings);
        }
        sqlx::query(
            "UPDATE sessions SET status = 'active', ended_at = NULL, end_reason = NULL, \
             end_detail = NULL WHERE id = $1 AND status = 'ended'",
        )
        .bind(session_id)
        .execute(&state.pool)
        .await
        .unwrap();
    }
    cctui_proto::api::GatewayEnvResponse {
        account_bound: !accounts.is_empty(),
        env,
        settings,
        whip_phrases,
        spawn_capability: spawn_capability_for(state, session_id).await,
        plugins,
        context: launch_context(state, user_id, machine_id, session_id, true).await.unwrap(),
        hook_policy: Some(
            cctui_server_accounts::hook_policy::for_session(&state.pool, session_id).await.unwrap(),
        ),
    }
}

async fn end_session(pool: &sqlx::PgPool, sid: &str) {
    sqlx::query("UPDATE sessions SET status = 'ended', ended_at = now() WHERE id = $1")
        .bind(sid)
        .execute(pool)
        .await
        .unwrap();
}

async fn session_status(pool: &sqlx::PgPool, sid: &str) -> String {
    sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Binds `sid` to one account carrying `providers`, each with its own
/// `settings_json`, minting the first token of every family.
async fn bind_families(
    state: &AppState,
    uid: Uuid,
    sid: &str,
    providers: &[(&str, serde_json::Value)],
) -> Uuid {
    let account: Uuid =
        sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
            .bind(uid)
            .bind(format!("genv-same-{}", Uuid::new_v4()))
            .fetch_one(&state.pool)
            .await
            .unwrap();
    for (provider, settings) in providers {
        let row: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider, settings_json) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(uid)
        .bind(account)
        .bind(provider)
        .bind(settings)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        cctui_server_accounts::routes::gateway::mint_session_env_for_account(state, row, sid)
            .await
            .unwrap()
            .expect("provider row exists");
    }
    account
}

/// The concurrent, de-duplicated pull answers exactly what the sequential
/// chain answered, for one and for two bound families, and still un-ends
/// an `ended` session.
#[tokio::test]
async fn gateway_env_answers_as_the_sequential_chain_did() {
    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_same_answer") else {
        return;
    };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let state = AppState::for_test(pool.clone());
    let cases: [(&str, &[(&str, serde_json::Value)]); 2] = [
        ("claude-code", &[("anthropic", serde_json::json!({ "theme": "dark" }))]),
        (
            "codex",
            &[
                ("anthropic", serde_json::json!({ "theme": "dark" })),
                ("openai", serde_json::json!({ "model_reasoning_summary": "auto" })),
            ],
        ),
    ];
    for (adapter, providers) in cases {
        let (uid, mid) = seed_machine(&pool, "genv-same").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        sqlx::query("UPDATE sessions SET adapter_id = $2 WHERE id = $1")
            .bind(&sid)
            .bind(adapter)
            .execute(&pool)
            .await
            .unwrap();
        let account = bind_families(&state, uid, &sid, providers).await;

        end_session(&pool, &sid).await;
        let before = sequential_gateway_env(&state, uid, mid, &sid).await;
        assert_eq!(session_status(&pool, &sid).await, "active");
        end_session(&pool, &sid).await;
        let after = super::gateway_env_for(&state, uid, mid, &sid, true).await.expect("pull");
        assert_eq!(session_status(&pool, &sid).await, "active", "{adapter}: un-ended");

        assert!(after.account_bound, "{adapter}");
        assert_eq!(
            after.env.keys().filter(|k| k.ends_with("_BASE_URL")).count(),
            providers.len(),
            "{adapter}: every family minted: {:?}",
            after.env.keys()
        );
        assert!(after.settings.is_some(), "{adapter}: settings served");
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            serde_json::to_value(&before).unwrap(),
            "{adapter}: same answer as the sequential chain"
        );

        sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM accounts WHERE id = $1").bind(account).execute(&pool).await.ok();
        drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }
}

/// An unbound session answers unbound, as before, and stays `ended`.
#[tokio::test]
async fn an_unbound_gateway_env_answers_as_the_sequential_chain_did() {
    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_same_unbound") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "genv-unbound").await;
    let sid = seed_owned_session(&pool, uid, mid).await;
    end_session(&pool, &sid).await;

    let before = sequential_gateway_env(&state, uid, mid, &sid).await;
    let after = super::gateway_env_for(&state, uid, mid, &sid, true).await.expect("pull");
    assert!(!after.account_bound);
    assert!(after.env.is_empty());
    assert_eq!(serde_json::to_value(&after).unwrap(), serde_json::to_value(&before).unwrap());
    assert_eq!(session_status(&pool, &sid).await, "ended", "only a bound relaunch un-ends");

    sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .ok();
    drop_machines(&pool, &[sid], &[(uid, mid)]).await;
}

#[tokio::test]
async fn gateway_env_serves_the_recorded_hook_policy_to_the_owning_machine_only() {
    use cctui_proto::hook_policy::{HookPolicyStatus, SessionHookPolicy};
    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_hook_policy") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "genv-policy").await;
    let (ub, mb) = seed_machine(&pool, "genv-policy-other").await;
    let sid = seed_owned_session(&pool, uid, mid).await;

    let none = super::gateway_env_for(&state, uid, mid, &sid, true).await.expect("pull");
    assert_eq!(none.hook_policy, Some(SessionHookPolicy::default()));

    let carried = cctui_server_accounts::hook_policy::carry("claude-code", Vec::new()).unwrap();
    assert_eq!(carried.status, Some(HookPolicyStatus::Applied));
    cctui_server_accounts::hook_policy::record_for_session(&pool, &sid, &carried)
        .await
        .expect("record");

    let pulled = super::gateway_env_for(&state, uid, mid, &sid, true).await.expect("pull");
    assert_eq!(pulled.hook_policy, Some(carried));
    let wire = serde_json::to_value(&pulled).unwrap();
    assert_eq!(wire["hook_policy"]["status"], serde_json::json!("applied"));

    let foreign = super::gateway_env_for(&state, ub, mb, &sid, true).await.expect("pull");
    assert_eq!(foreign.hook_policy, None, "another machine never sees the policy");

    sqlx::query("DELETE FROM session_hook_policies WHERE session_id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .ok();
    drop_machines(&pool, &[sid], &[(uid, mid), (ub, mb)]).await;
}

#[tokio::test]
async fn with_context_off_both_launch_pulls_are_empty_and_ownership_still_holds() {
    use cctui_proto::features::{Feature, FeatureSet};
    use cctui_server_sessions::routes::context::ownership::test_support::assert_no_content;
    use cctui_server_sessions::routes::context::ownership::{self};
    use cctui_server_sessions::routes::context::{
        ContextItemSpec, SpawnScope, StoredScope, clean_spec, insert,
    };
    let Some(url) = cctui_server_kernel::test_support::db_url("launch_context_off") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (ua, ma) = seed_machine(&pool, "ctx-off-owner").await;
    let (ub, mb) = seed_machine(&pool, "ctx-off-intruder").await;
    let rule = clean_spec(ContextItemSpec {
        kind: "rule".into(),
        name: "always".into(),
        title: "Always".into(),
        body: "every launch".into(),
        enabled: true,
        ..ContextItemSpec::default()
    })
    .expect("valid rule");
    let rule = insert(&pool, ua, &rule, None, None).await.expect("rule");
    let key = format!("cmd-{}", Uuid::new_v4());
    let coords = SpawnScope { machine_id: Some(ma.to_string()), ..SpawnScope::default() };
    let scope = StoredScope { user_id: ua, coords, dispatched: false };
    ownership::record(&pool, &key, &scope).await;
    let mut state = AppState::for_test(pool.clone());
    state.config.features = FeatureSet::all().without(Feature::Context);

    assert!(
        super::owns_launch(&AppState::for_test(pool.clone()), ua, ma, &key).await.expect("lookup")
    );
    assert!(
        !super::owns_launch(&AppState::for_test(pool.clone()), ub, mb, &key).await.expect("lookup")
    );
    let pulled = super::launch_context(&state, ua, ma, &key, true).await.expect("pull");
    assert!(pulled.is_empty(), "the context pull is empty: {pulled:?}");
    let env = super::gateway_env_for(&state, ua, ma, &key, true).await.expect("pull");
    assert!(env.context.is_empty(), "the gateway-env pull carries no context");
    assert!(env.hook_policy.is_some(), "the owner's pull is served in full");
    assert_no_content(&pool, &key).await;

    state.config.features = FeatureSet::all();
    let on = super::launch_context(&state, ua, ma, &key, true).await.expect("pull");
    assert_eq!(on.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["always"]);

    sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
        .bind(&key)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
        .bind(&key)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM context_items WHERE id = $1").bind(rule.id).execute(&pool).await.ok();
    drop_machines(&pool, &[], &[(ua, ma), (ub, mb)]).await;
}

#[tokio::test]
async fn with_plugins_off_a_launch_carries_no_plugins_and_syncs_none() {
    use cctui_proto::features::{Feature, FeatureSet};
    use cctui_server_extras::plugins::test_support::write_plugin;
    let Some(url) = cctui_server_kernel::test_support::db_url("gateway_env_plugins_off") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let root = tempfile::tempdir().unwrap();
    write_plugin(root.path(), "kit", "");
    let (uid, mid) = seed_machine(&pool, "genv-plugins-off").await;
    let sid = seed_owned_session(&pool, uid, mid).await;
    sqlx::query(
        "INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2) \
         ON CONFLICT (user_id) DO UPDATE SET data = EXCLUDED.data",
    )
    .bind(uid)
    .bind(serde_json::json!({ "plugins": { "enabled": { "kit": true } } }))
    .execute(&pool)
    .await
    .expect("enable the plugin");
    let mut state = AppState::for_test(pool.clone());
    state.plugins = std::sync::Arc::new(cctui_server_extras::plugins::PluginRegistry::from_dir(
        root.path().to_path_buf(),
    ));
    state.config.features = FeatureSet::all().without(Feature::Plugins);

    let off = super::gateway_env_for(&state, uid, mid, &sid, true).await.expect("pull");
    assert!(off.plugins.is_empty(), "no plugin entries: {:?}", off.plugins);
    assert_eq!(state.plugins.loaded_from(), None, "no plugin sync was attempted");

    state.config.features = FeatureSet::all();
    let on = super::gateway_env_for(&state, uid, mid, &sid, true).await.expect("pull");
    assert_eq!(on.plugins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["kit"]);
    assert!(state.plugins.loaded_from().is_some(), "a launch with plugins on syncs");

    sqlx::query("DELETE FROM user_settings WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .ok();
    drop_machines(&pool, &[sid], &[(uid, mid)]).await;
}
