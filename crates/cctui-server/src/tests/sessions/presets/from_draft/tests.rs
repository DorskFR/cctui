use axum::extract::{Path, State};
use axum::{Extension, Json};
use cctui_proto::api::presets::RepeatDraftRequest;

use crate::state::AppState;
use cctui_server_kernel::auth::{AuthContext, Scope};

async fn seed_draft(state: &AppState, machine: uuid::Uuid) -> (String, Option<String>) {
    let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
        "machine_id": machine.to_string(), "working_dir": "/w/widgets",
        "prompt": "digest with <redacted>hunter2-xyz</redacted>", "name": "nightly digest",
        "adapter_id": "claude-code", "model": "opus", "permission_mode": "auto",
        "effort": "high", "account": "personal", "auto_archive": true,
        "context": {"items": ["notes"], "auto": false},
        "env": {"FEED_TOKEN": "feed-secret-value"}, "save_draft": true,
    }))
    .unwrap();
    let brief = cctui_server_sessions::uploads::RawUpload {
        name: "brief.md".into(),
        bytes: b"# brief".to_vec().into(),
        content_type: Some("text/markdown".into()),
    };
    let draft = cctui_server_sessions::routes::spawn::store_spawn_row(
        state,
        &req,
        machine,
        &[brief],
        cctui_server_sessions::store::sessions::SessionRowStatus::Draft,
    )
    .await
    .unwrap()
    .to_string();
    let draft_env: Option<String> =
        sqlx::query_scalar("SELECT metadata->>'draft_sealed_env' FROM sessions WHERE id = $1")
            .bind(&draft)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    (draft, draft_env)
}

#[tokio::test]
async fn a_repeat_schedule_carries_the_drafts_whole_payload_and_sealed_env() {
    let name = "a_repeat_schedule_carries_the_drafts_whole_payload_and_sealed_env";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = cctui_server_sessions::presets::test_support::seed_owner(&pool, "repeat").await;
    sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read'), ($1, 'dispatch')")
        .bind(owner.user)
        .execute(&pool)
        .await
        .unwrap();
    let account: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (user_id, name) VALUES ($1, 'personal') RETURNING id",
    )
    .bind(owner.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let state = AppState::for_test(pool.clone());
    let ctx = AuthContext {
        user_id: owner.user,
        key_id: uuid::Uuid::new_v4(),
        machine_id: None,
        scopes: [Scope::Read, Scope::Dispatch].into_iter().collect(),
    };
    let (draft, draft_env) = seed_draft(&state, owner.machine).await;

    let body = RepeatDraftRequest {
        cron: "0 7 * * *".into(),
        timezone: Some("UTC".into()),
        ..RepeatDraftRequest::default()
    };
    let (_, Json(preset)) = cctui_server_sessions::routes::presets::repeat_draft(
        State(state),
        Extension(ctx),
        Path(draft.clone()),
        Json(body),
    )
    .await
    .unwrap();
    assert_eq!(preset.name, "nightly digest");
    assert_eq!(preset.cron.as_deref(), Some("0 7 * * *"));
    assert_eq!(preset.working_dir.as_deref(), Some("/w/widgets"));
    assert_eq!(preset.model.as_deref(), Some("opus"));
    assert_eq!(preset.account_id, Some(account));
    assert_eq!(preset.template.effort.as_deref(), Some("high"));
    assert!(preset.template.auto_archive);
    assert_eq!(preset.template.context.as_ref().map(|c| c.items.len()), Some(1));
    assert_eq!(preset.template.attachment_names, ["brief.md"]);
    assert_eq!(preset.env_keys, ["FEED_TOKEN"]);
    let shown = serde_json::to_string(&preset).unwrap();
    assert!(!shown.contains("hunter2-xyz") && !shown.contains("feed-secret-value"), "{shown}");

    let (sealed_env, sealed_prompt): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT sealed_env, sealed_prompt FROM presets WHERE id = $1")
            .bind(preset.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sealed_env, draft_env, "the sealed env moved as stored");
    let env = cctui_server_sessions::sealed_env::unseal(
        sealed_env.as_deref(),
        cctui_server_kernel::crypto::vault_key,
    );
    assert_eq!(env.unwrap().get("FEED_TOKEN").map(String::as_str), Some("feed-secret-value"));
    let loaded =
        cctui_server_sessions::inbox::preset::load(&pool, preset.id).await.unwrap().unwrap();
    assert_eq!(loaded.sealed_prompt, sealed_prompt);
    let open = cctui_server_sessions::inbox::preset::opened(
        loaded,
        cctui_server_kernel::crypto::vault_key,
    )
    .unwrap();
    assert_eq!(open.prompt, "digest with <redacted>hunter2-xyz</redacted>");
    let key = cctui_server_sessions::presets::store::files_key(preset.id);
    let (files, _) = cctui_server_sessions::routes::spawn::load_draft_uploads::<
        crate::state::AppState,
    >(&pool, &key, &[])
    .await
    .unwrap();
    assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["brief.md"]);
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id = $1")
        .bind(&draft)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(gone, 0, "the schedule replaces the draft");
}
