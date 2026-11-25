use axum::http::StatusCode;
use base64::Engine;

/// An enabled adapter outside the harness table is not a family the server
/// can launch, so the default capability must not advertise it to
/// `CctuiAgent`.
#[test]
fn the_default_capability_advertises_only_spawnable_adapters() {
    let ids = ["claude-code", "gemini", "codex", "opencode", "qwen", "goose", "custom-acp"]
        .map(str::to_owned)
        .to_vec();
    assert_eq!(
        super::spawnable_adapters(ids),
        ["claude-code", "gemini", "codex", "opencode", "qwen", "goose"]
    );
    assert!(super::spawnable_adapters(Vec::new()).is_empty());
}

#[tokio::test]
async fn only_the_owner_drains_a_machine_queue() {
    use axum::extract::{Path, State};
    use axum::{Extension, Json};

    let Some(url) =
        cctui_server_kernel::test_support::db_url("only_the_owner_drains_a_machine_queue")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let (owner, other, machine) =
        (uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    for uid in [owner, other] {
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("mq-{uid}"))
            .bind(format!("hmq-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(owner)
        .bind(format!("mkmq-{machine}"))
        .execute(&pool)
        .await
        .unwrap();
    let state = crate::state::AppState::for_test(pool);
    state.registry.write().await.queue_machine_command(
        &machine.to_string(),
        "noop",
        serde_json::Value::Null,
    );
    let caller = |user_id| cctui_server_kernel::auth::AuthContext {
        user_id,
        key_id: uuid::Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(cctui_server_kernel::auth::Scope::Read).collect(),
    };

    let denied = super::get_machine_commands(
        State(state.clone()),
        Extension(caller(other)),
        Path(machine.to_string()),
    )
    .await;
    assert!(denied.is_err(), "another user must not drain the queue");

    let Json(drained) = super::get_machine_commands(
        State(state),
        Extension(caller(owner)),
        Path(machine.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(drained.len(), 1, "the refused poll left the command queued");
}

fn upload(name: &str, bytes: &[u8]) -> cctui_server_sessions::uploads::RawUpload {
    cctui_server_sessions::uploads::RawUpload {
        name: name.to_owned(),
        bytes: axum::body::Bytes::copy_from_slice(bytes),
        content_type: None,
    }
}

async fn seed_draft(pool: &sqlx::PgPool) -> String {
    let uid = uuid::Uuid::new_v4();
    let machine = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'draft-att', $2)")
        .bind(uid)
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine)
        .bind(uid)
        .bind(machine.to_string())
        .bind(format!("kh-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    let sid = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id, status) \
         VALUES ($1, $2, '/w', $3, $4, 'claude-code', 'draft')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(uid)
    .bind(machine)
    .execute(pool)
    .await
    .unwrap();
    sid
}

#[tokio::test]
async fn a_launch_reads_the_drafts_files_back_as_spawn_uploads() {
    let Some(url) = cctui_server_kernel::test_support::db_url("draft_attachments_launch") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let draft = seed_draft(&pool).await;
    let note = format!("draft note {draft}");
    cctui_server_sessions::routes::attachments::record_uploads::<crate::state::AppState>(
        &pool,
        &draft,
        &[
            upload("note.md", note.as_bytes()),
            upload("shot.png", &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1]),
        ],
        &[],
    )
    .await
    .unwrap();

    let names = ["note.md".to_owned(), "shot.png".to_owned()];
    let (uploads, raw) =
        super::load_draft_uploads::<crate::state::AppState>(&pool, &draft, &names).await.unwrap();
    assert_eq!(uploads.len(), 2);
    assert_eq!(raw.len(), 2);
    assert_eq!(uploads[0].name, "note.md");
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(&uploads[0].content_b64).unwrap(),
        note.as_bytes(),
        "the daemon decodes the same bytes the form attached"
    );
    assert_eq!(raw[1].name, "shot.png");
    assert_eq!(raw[1].content_type.as_deref(), Some("image/png"));

    let Err(cctui_server_kernel::error::AppError::Status(code, msg)) =
        super::load_draft_uploads::<crate::state::AppState>(
            &pool,
            &draft,
            &["gone.txt".to_owned()],
        )
        .await
    else {
        panic!("a referenced file the store lacks must fail the launch");
    };
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(msg.contains("gone.txt"), "{msg}");

    let rows = super::replace_draft_attachments::<crate::state::AppState>(
        &pool,
        &draft,
        &[upload("only.txt", b"x")],
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let listed =
        cctui_server_sessions::routes::attachments::list_session_attachments(&pool, &draft)
            .await
            .unwrap();
    assert_eq!(listed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["only.txt"]);
    let (uploads, _) =
        super::load_draft_uploads::<crate::state::AppState>(&pool, &draft, &[]).await.unwrap();
    assert_eq!(uploads.len(), 1, "a draft with no listed names still ships what it stored");

    super::replace_draft_attachments::<crate::state::AppState>(&pool, &draft, &[]).await.unwrap();
    assert!(
        cctui_server_sessions::routes::attachments::list_session_attachments(&pool, &draft)
            .await
            .unwrap()
            .is_empty()
    );
    let (uploads, raw) =
        super::load_draft_uploads::<crate::state::AppState>(&pool, &draft, &[]).await.unwrap();
    assert!(uploads.is_empty() && raw.is_empty());

    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&draft).execute(&pool).await.unwrap();
}

#[test]
fn spawn_refuses_an_adapter_with_no_family() {
    assert_eq!(super::spawn_adapter(None).unwrap(), "claude-code");
    assert_eq!(super::spawn_adapter(Some(" ")).unwrap(), "claude-code");
    for adapter in ["claude-code", "codex", "opencode", "gemini", "qwen", "goose"] {
        assert_eq!(super::spawn_adapter(Some(adapter)).unwrap(), adapter);
    }
    assert!(super::spawn_adapter(Some("kilo")).is_err());
}

use super::{AccountDecision, decide_account, resolve_default_account};
use uuid::Uuid;

#[test]
fn named_account_always_wins() {
    assert_eq!(
        decide_account(Some(" acme "), false, false, None),
        AccountDecision::Named("acme".to_owned())
    );
    assert_eq!(
        decide_account(Some("acme"), true, false, None),
        AccountDecision::Named("acme".to_owned())
    );
    // A stale `auto_account` can no more override an explicit pick than a
    // stale `no_account` can.
    assert_eq!(
        decide_account(Some("acme"), false, true, None),
        AccountDecision::Named("acme".to_owned())
    );
}

#[test]
fn no_account_bypasses_default_resolution() {
    assert_eq!(decide_account(None, true, false, None), AccountDecision::Unbound);
    assert_eq!(decide_account(Some("   "), true, false, None), AccountDecision::Unbound);
    // An explicit unbound spawn outranks auto-selection: asking for no
    // account can never be answered with one.
    assert_eq!(decide_account(None, true, true, None), AccountDecision::Unbound);
}

#[test]
fn unset_account_resolves_default() {
    assert_eq!(decide_account(None, false, false, None), AccountDecision::ResolveDefault);
    assert_eq!(decide_account(Some(""), false, false, None), AccountDecision::ResolveDefault);
}

#[test]
fn auto_account_delegates_the_choice() {
    assert_eq!(decide_account(None, false, true, None), AccountDecision::Auto);
    assert_eq!(decide_account(Some("  "), false, true, None), AccountDecision::Auto);
}

#[test]
fn a_pool_bounds_the_choice_and_beats_auto() {
    assert_eq!(
        decide_account(None, false, false, Some(" perso ")),
        AccountDecision::Pool("perso".to_owned())
    );
    // `auto_account` widens to every reachable account; a named pool is
    // the narrower instruction, so it must win rather than be widened.
    assert_eq!(
        decide_account(None, false, true, Some("perso")),
        AccountDecision::Pool("perso".to_owned())
    );
    // The two refusals still outrank it: an explicit account, and an
    // explicit unbound spawn.
    assert_eq!(
        decide_account(Some("acme"), false, false, Some("perso")),
        AccountDecision::Named("acme".to_owned())
    );
    assert_eq!(decide_account(None, true, false, Some("perso")), AccountDecision::Unbound);
    // A blank pool name is not a pool.
    assert_eq!(decide_account(None, false, true, Some("  ")), AccountDecision::Auto);
    assert_eq!(decide_account(None, false, false, Some("")), AccountDecision::ResolveDefault);
}

/// The reauth gate lives in the election only. An account named outright
/// never reaches it, whatever else the request asks for, so naming an
/// account that must be reconnected still binds it, exactly as before.
#[test]
fn a_named_account_is_never_put_to_an_election() {
    for (auto, pool) in [(false, None), (true, None), (true, Some("perso"))] {
        assert_eq!(
            decide_account(Some("refused"), false, auto, pool),
            AccountDecision::Named("refused".to_owned())
        );
    }
    assert_eq!(
        cctui_server_accounts::account_resolve::choose_target(
            "refused",
            true,
            Some(cctui_server_accounts::store::account_pools::AccountPool {
                id: Uuid::nil(),
                user_id: Uuid::nil(),
                name: "refused".to_owned(),
                strategy: cctui_server_accounts::store::account_pools::STRATEGY_HEADROOM.to_owned(),
                failover: false,
                created_at: chrono::Utc::now(),
            })
        ),
        cctui_server_accounts::account_resolve::Target::Account("refused".to_owned())
    );
}

#[test]
fn default_account_zero_one_many() {
    let uid = Uuid::nil();
    assert_eq!(resolve_default_account(&[], uid, "codex").unwrap(), None);
    assert_eq!(
        resolve_default_account(&["solo".to_owned()], uid, "codex").unwrap(),
        Some("solo".to_owned())
    );
    let err = resolve_default_account(&["a".to_owned(), "b".to_owned()], uid, "codex").unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(err.1.0.error.contains("a, b"));
}

/// An account whose owner cleared `pool_eligible` is out of every election
/// (`auto_account`, its owned-only variant for `CctuiAgent` children, and
/// pools, which gateway failover elects through), owned or shared, while
/// the eligible ones next to it stay candidates.
#[tokio::test]
async fn an_ineligible_account_is_never_elected() {
    use cctui_server_kernel::accounts_types::Family;

    let Some(url) =
        cctui_server_kernel::test_support::db_url("an_ineligible_account_is_never_elected")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");

    let me = elig_user(&pool).await;
    let lender = elig_user(&pool).await;
    let spark = elig_account(&pool, me, "Spark local (OpenCode)", "fireworks", false).await;
    let cloud = elig_account(&pool, me, "Fireworks cloud", "fireworks", true).await;
    let lent_spark = elig_account(&pool, lender, "Lent spark", "fireworks", false).await;
    let lent_cloud = elig_account(&pool, lender, "Lent cloud", "fireworks", true).await;
    for lent in [lent_spark, lent_cloud] {
        sqlx::query(
            "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
             VALUES ('account', $1, $2)",
        )
        .bind(lent)
        .bind(me)
        .execute(&pool)
        .await
        .unwrap();
    }

    let names = |rows: Vec<super::AutoAccountRow>| {
        let mut n: Vec<String> = rows.into_iter().map(|r| r.1).collect();
        n.sort();
        n
    };
    let auto = super::auto_account_candidates(&pool, me, Family::Fireworks, false).await.unwrap();
    assert_eq!(names(auto), ["Fireworks cloud", "Lent cloud"]);
    let owned = super::auto_account_candidates(&pool, me, Family::Fireworks, true).await.unwrap();
    assert_eq!(names(owned), ["Fireworks cloud"]);

    // Enrolled in the owner's own pool, the ineligible account is kept as a
    // member but never handed to an election.
    let pool_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO account_pools (user_id, name) VALUES ($1, 'Mine') RETURNING id",
    )
    .bind(me)
    .fetch_one(&pool)
    .await
    .unwrap();
    for (pos, acct) in [spark, cloud, lent_spark, lent_cloud].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO account_pool_members (pool_id, account_id, position) \
             VALUES ($1, $2, $3)",
        )
        .bind(pool_id)
        .bind(acct)
        .bind(i32::try_from(pos).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    }
    let usable = cctui_server_accounts::store::account_pools::usable_members(
        &pool,
        pool_id,
        me,
        "fireworks",
    )
    .await
    .unwrap();
    let usable: Vec<&str> = usable.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(usable, ["Fireworks cloud", "Lent cloud"]);
    let stored =
        cctui_server_accounts::store::account_pools::members(&pool, pool_id).await.unwrap();
    assert_eq!(stored.len(), 4, "membership rows are kept, only elections skip them");

    for uid in [me, lender] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}

async fn elig_user(pool: &sqlx::PgPool) -> uuid::Uuid {
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("elig-{uid}"))
        .bind(format!("helig-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    uid
}
async fn elig_account(
    pool: &sqlx::PgPool,
    owner: uuid::Uuid,
    name: &str,
    provider: &str,
    eligible: bool,
) -> uuid::Uuid {
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (user_id, name, pool_eligible) VALUES ($1, $2, $3) \
         RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(eligible)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_providers (user_id, account_id, provider) \
         VALUES ($1, $2, $3)",
    )
    .bind(owner)
    .bind(id)
    .bind(provider)
    .execute(pool)
    .await
    .unwrap();
    id
}
