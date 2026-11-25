use axum::extract::{Path, State};
use axum::{Extension, Json};
use cctui_proto::api::SpawnRequest;

use super::spawn_share_tests::{SPAWNER, caller, seed_machine, seed_user};
use super::{DRAFT_SEALED_ENV, DRAFT_SEALED_PROMPT, DRAFT_UNSEALABLE, unseal_draft_prompt};
use cctui_server_sessions::store::sessions::SessionRowStatus;

const TAGGED: &str = "log in with <redacted>hunter2-xyz</redacted> please";
const MASKED: &str = "log in with [REDACTED:user] please";

fn request(machine: &str, prompt: &str) -> SpawnRequest {
    serde_json::from_value(serde_json::json!({
        "machine_id": machine, "working_dir": "/w", "prompt": prompt,
    }))
    .unwrap()
}

/// Saving and launching a sealed draft never goes through the scheduling
/// module: the launch gets past unsealing with `scheduling` off.
#[tokio::test]
async fn a_sealed_draft_launches_with_scheduling_off() {
    use cctui_proto::features::{Feature, FeatureSet};
    let name = "a_sealed_draft_launches_with_scheduling_off";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await.to_string();
    let mut state = crate::state::AppState::for_test(pool.clone());
    state.config.features = FeatureSet::all().without(Feature::Scheduling);
    let ctx = caller(owner, SPAWNER);

    let mut req = request(&machine, TAGGED);
    req.save_draft = true;
    let (_, Json(saved)) = super::save_draft(&state, &ctx, &req, &[]).await.unwrap();
    let id = saved.command_id.to_string();
    let meta = stored_metadata(&pool, &id).await;
    assert!(meta.get(DRAFT_SEALED_PROMPT).is_some(), "{meta}");

    let launched =
        super::launch_stored_draft(&state, &ctx, &id, std::collections::BTreeMap::new()).await;
    if let Err(err) = launched {
        assert_ne!(err.message(), DRAFT_UNSEALABLE, "the sealed prompt must open");
    }
}

async fn stored_metadata(pool: &sqlx::PgPool, id: &str) -> serde_json::Value {
    sqlx::query_scalar("SELECT metadata FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_saved_and_edited_draft_row_holds_no_plaintext() {
    let name = "a_saved_and_edited_draft_row_holds_no_plaintext";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await.to_string();
    let state = crate::state::AppState::for_test(pool.clone());
    let ctx = caller(owner, SPAWNER);

    let mut req = request(&machine, TAGGED);
    req.save_draft = true;
    let (_, Json(saved)) = super::save_draft(&state, &ctx, &req, &[]).await.unwrap();
    let id = saved.command_id.to_string();
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(s)::text FROM sessions s WHERE id = $1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!row.contains("hunter2-xyz"), "plaintext stored: {row}");
    assert!(row.contains(MASKED), "{row}");

    let edit = |prompt: &str| {
        cctui_server_sessions::routes::sessions::update_draft(
            State(state.clone()),
            Extension(ctx.clone()),
            Path(id.clone()),
            Json(request(&machine, prompt)),
        )
    };
    assert_eq!(edit(MASKED).await.unwrap().0.status, "draft");
    let meta = stored_metadata(&pool, &id).await;
    assert!(!meta.to_string().contains("hunter2-xyz"), "{meta}");
    let mut back: SpawnRequest = serde_json::from_value(meta["draft"].clone()).unwrap();
    unseal_draft_prompt(&mut back, &meta, cctui_server_kernel::crypto::vault_key).unwrap();
    assert_eq!(back.prompt.as_deref(), Some(TAGGED), "an unchanged edit keeps the real value");

    let Json(edited) = edit("now <REDACTED>other-value</redacted>").await.unwrap();
    assert_eq!(edited.command_id.to_string(), id);
    let meta = stored_metadata(&pool, &id).await;
    assert!(!meta.to_string().contains("other-value"), "{meta}");
    assert_eq!(meta["draft"]["prompt"], "now [REDACTED:user]");

    assert_eq!(edit("rewritten").await.unwrap().0.status, "draft");
    let meta = stored_metadata(&pool, &id).await;
    assert!(meta.get(DRAFT_SEALED_PROMPT).is_none(), "a rewritten prompt drops the old seal");
}

fn with_env(machine: &str, keys: &[&str], env: &[(&str, &str)]) -> SpawnRequest {
    let mut req = request(machine, "plain");
    req.env_keys = keys.iter().map(|k| (*k).to_owned()).collect();
    req.env = env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    req
}

fn env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

#[tokio::test]
async fn a_draft_env_is_sealed_write_only_and_edited_per_key() {
    let name = "a_draft_env_is_sealed_write_only_and_edited_per_key";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await.to_string();
    let state = crate::state::AppState::for_test(pool.clone());
    let ctx = caller(owner, SPAWNER);

    let mut req = with_env(&machine, &[], &[("A", "alpha-secret"), ("B", "beta-secret")]);
    req.save_draft = true;
    let (_, Json(saved)) = super::save_draft(&state, &ctx, &req, &[]).await.unwrap();
    let id = saved.command_id.to_string();
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(s)::text FROM sessions s WHERE id = $1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!row.contains("alpha-secret") && !row.contains("beta-secret"), "{row}");
    let meta = stored_metadata(&pool, &id).await;
    assert_eq!(meta["draft"]["env_keys"], serde_json::json!(["A", "B"]));
    assert!(meta["draft"].get("env").is_none(), "{meta}");
    assert!(meta.get(DRAFT_SEALED_ENV).is_some(), "{meta}");
    let stored = |state: crate::state::AppState, id: String| async move {
        super::stored_spawn(&state, &id, SessionRowStatus::Draft).await.unwrap().0.env
    };
    assert_eq!(
        stored(state.clone(), id.clone()).await,
        env_of(&[("A", "alpha-secret"), ("B", "beta-secret")])
    );

    let edit = |req: SpawnRequest| {
        cctui_server_sessions::routes::sessions::update_draft(
            State(state.clone()),
            Extension(ctx.clone()),
            Path(id.clone()),
            Json(req),
        )
    };
    let Json(reply) =
        edit(with_env(&machine, &["A", "B"], &[("B", "beta-new"), ("C", "gamma-new")]))
            .await
            .unwrap();
    assert!(!serde_json::to_string(&reply).unwrap().contains("-new"));
    assert_eq!(
        stored(state.clone(), id.clone()).await,
        env_of(&[("A", "alpha-secret"), ("B", "beta-new"), ("C", "gamma-new")]),
        "A kept, B replaced, C added"
    );
    let meta = stored_metadata(&pool, &id).await;
    assert!(!meta.to_string().contains("alpha-secret"), "{meta}");
    assert_eq!(meta["draft"]["env_keys"], serde_json::json!(["A", "B", "C"]));

    let _ = edit(with_env(&machine, &["A"], &[])).await.unwrap();
    assert_eq!(stored(state.clone(), id.clone()).await, env_of(&[("A", "alpha-secret")]));

    let _ = edit(with_env(&machine, &[], &[])).await.unwrap();
    let meta = stored_metadata(&pool, &id).await;
    assert!(meta.get(DRAFT_SEALED_ENV).is_none(), "no names left, no seal: {meta}");
    assert!(stored(state, id).await.is_empty());
}
