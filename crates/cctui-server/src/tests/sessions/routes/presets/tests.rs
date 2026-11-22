use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::presets::LaunchPresetRequest;
use uuid::Uuid;

use super::{create, delete, get_one, list, update};
use crate::state::AppState;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_sessions::presets::test_support::{Owner, seed_owner};

fn caller(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: scopes.iter().copied().collect(),
    }
}

fn writer(owner: &Owner) -> AuthContext {
    caller(owner.user, &[Scope::Read, Scope::Dispatch])
}

async fn state(name: &str) -> Option<AppState> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(AppState::for_test(pool))
}

fn body(owner: &Owner, name: &str) -> LaunchPresetRequest {
    LaunchPresetRequest {
        name: name.into(),
        prompt: "triage".into(),
        machine_id: Some(owner.machine),
        working_dir: Some("/w".into()),
        ..LaunchPresetRequest::default()
    }
}

#[tokio::test]
async fn preset_routes_are_owner_scoped_and_need_dispatch_to_write() {
    let Some(state) = state("preset_routes_are_owner_scoped").await else { return };
    let (a, b) = (seed_owner(&state.pool, "ha").await, seed_owner(&state.pool, "hb").await);

    let read_only = caller(a.user, &[Scope::Read]);
    let err = create(State(state.clone()), Extension(read_only.clone()), Json(body(&a, "x")))
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    let (code, Json(preset)) =
        create(State(state.clone()), Extension(writer(&a)), Json(body(&a, "ops"))).await.unwrap();
    assert_eq!(code, StatusCode::CREATED);
    let Json(seen) =
        get_one(State(state.clone()), Extension(read_only.clone()), Path(preset.id)).await.unwrap();
    assert_eq!(seen.id, preset.id);

    let err =
        get_one(State(state.clone()), Extension(writer(&b)), Path(preset.id)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    let err =
        update(State(state.clone()), Extension(writer(&b)), Path(preset.id), Json(body(&b, "y")))
            .await
            .unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    let err =
        delete(State(state.clone()), Extension(writer(&b)), Path(preset.id)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    let Json(theirs) = list(State(state.clone()), Extension(writer(&b))).await.unwrap();
    assert!(theirs.iter().all(|p| p.id != preset.id));

    let err =
        delete(State(state.clone()), Extension(read_only), Path(preset.id)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
    let code = delete(State(state), Extension(writer(&a)), Path(preset.id)).await.unwrap();
    assert_eq!(code, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn preset_route_errors_map_to_status_codes() {
    let Some(state) = state("preset_route_errors_map_to_status_codes").await else { return };
    let (a, b) = (seed_owner(&state.pool, "ea").await, seed_owner(&state.pool, "eb").await);
    let post =
        |req: LaunchPresetRequest| create(State(state.clone()), Extension(writer(&a)), Json(req));

    let unguarded = LaunchPresetRequest { external_sources: true, ..body(&a, "external") };
    let err = post(unguarded).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(err.message().contains("guarded"), "{}", err.message());

    let foreign = LaunchPresetRequest { machine_id: Some(b.machine), ..body(&a, "foreign") };
    assert_eq!(post(foreign).await.unwrap_err().status(), StatusCode::BAD_REQUEST);

    let (code, _) = post(body(&a, "twice")).await.unwrap();
    assert_eq!(code, StatusCode::CREATED);
    assert_eq!(post(body(&a, "twice")).await.unwrap_err().status(), StatusCode::CONFLICT);
}

fn env(pairs: &[(&str, &str)]) -> cctui_proto::api::presets::EnvValues {
    cctui_proto::api::presets::EnvValues(
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
    )
}

async fn stored_env(
    state: &AppState,
    id: Uuid,
) -> (Option<String>, cctui_server_sessions::sealed_env::EnvMap) {
    let sealed: Option<String> = sqlx::query_scalar("SELECT sealed_env FROM presets WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let open = cctui_server_sessions::sealed_env::unseal(
        sealed.as_deref(),
        cctui_server_kernel::crypto::vault_key,
    )
    .unwrap();
    (sealed, open)
}

#[tokio::test]
async fn preset_env_is_write_only_and_edited_per_key() {
    let Some(state) = state("preset_env_is_write_only_and_edited_per_key").await else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let a = seed_owner(&state.pool, "henv").await;
    let put = |id: Uuid, req: LaunchPresetRequest| {
        update(State(state.clone()), Extension(writer(&a)), Path(id), Json(req))
    };
    let first = LaunchPresetRequest {
        env: Some(env(&[
            ("KEEP", "keep-secret"),
            ("SWAP", "swap-secret"),
            ("GONE", "gone-secret"),
        ])),
        ..body(&a, "digest")
    };
    let (_, Json(created)) =
        create(State(state.clone()), Extension(writer(&a)), Json(first)).await.unwrap();
    assert_eq!(created.env_keys, ["GONE", "KEEP", "SWAP"]);
    let (sealed, open) = stored_env(&state, created.id).await;
    assert!(!sealed.as_deref().unwrap_or_default().contains("secret"), "sealed, not plaintext");
    assert_eq!(open.len(), 3);

    let Json(seen) =
        get_one(State(state.clone()), Extension(writer(&a)), Path(created.id)).await.unwrap();
    let Json(all) = list(State(state.clone()), Extension(writer(&a))).await.unwrap();
    for json in [serde_json::to_string(&seen).unwrap(), serde_json::to_string(&all).unwrap()] {
        assert!(!json.contains("secret"), "a read leaked a value: {json}");
        assert!(!json.contains("v1:"), "a read leaked the sealed blob: {json}");
    }
    assert_eq!(seen.env_keys, created.env_keys);

    let untouched = LaunchPresetRequest { enabled: Some(false), ..body(&a, "digest") };
    let Json(paused) = put(created.id, untouched).await.unwrap();
    assert_eq!(paused.env_keys, created.env_keys, "no env fields: env unchanged");
    assert_eq!(stored_env(&state, created.id).await.0, sealed, "the seal is not even rewritten");

    let round_trip = LaunchPresetRequest { env_keys: Some(seen.env_keys), ..body(&a, "digest") };
    let _ = put(created.id, round_trip).await.unwrap();
    assert_eq!(stored_env(&state, created.id).await.1, open, "GET names back: every value kept");

    let edit = LaunchPresetRequest {
        env_keys: Some(vec!["KEEP".into(), "SWAP".into()]),
        env: Some(env(&[("SWAP", "swap-new"), ("ADDED", "added-new")])),
        ..body(&a, "digest")
    };
    let Json(edited) = put(created.id, edit).await.unwrap();
    assert_eq!(edited.env_keys, ["ADDED", "KEEP", "SWAP"]);
    assert!(!serde_json::to_string(&edited).unwrap().contains("-new"));
    let (_, open) = stored_env(&state, created.id).await;
    assert_eq!(open.get("KEEP").map(String::as_str), Some("keep-secret"), "kept");
    assert_eq!(open.get("SWAP").map(String::as_str), Some("swap-new"), "replaced");
    assert_eq!(open.get("ADDED").map(String::as_str), Some("added-new"), "added");
    assert!(!open.contains_key("GONE"), "deleted");

    let cleared = LaunchPresetRequest { env_keys: Some(Vec::new()), ..body(&a, "digest") };
    let Json(cleared) = put(created.id, cleared).await.unwrap();
    assert!(cleared.env_keys.is_empty());
    assert_eq!(stored_env(&state, created.id).await.0, None);

    let bad = LaunchPresetRequest { env: Some(env(&[("lower", "x")])), ..body(&a, "bad") };
    let err = create(State(state), Extension(writer(&a)), Json(bad)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
}

async fn seals(pool: &sqlx::PgPool, id: Uuid) -> (Option<String>, Option<String>) {
    sqlx::query_as("SELECT sealed_env, sealed_prompt FROM presets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_get_put_round_trip_keeps_the_template_prompt_seal_env_and_files() {
    use cctui_proto::adapter::PermissionMode;
    use cctui_proto::api::presets::SpawnTemplate;
    let Some(state) = state("a_get_put_round_trip_keeps_the_template").await else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let a = seed_owner(&state.pool, "hrt").await;
    let first = LaunchPresetRequest {
        prompt: "digest with <redacted>hunter2-xyz</redacted>".into(),
        permission_mode: Some(PermissionMode::Auto),
        env: Some(env(&[("FEED_TOKEN", "feed-secret")])),
        template: Some(SpawnTemplate {
            effort: Some("high".into()),
            label_ids: vec!["l1".into()],
            auto_archive: true,
            ..SpawnTemplate::default()
        }),
        ..body(&a, "round-trip")
    };
    let (_, Json(created)) =
        create(State(state.clone()), Extension(writer(&a)), Json(first)).await.unwrap();
    assert_eq!(created.prompt, "digest with [REDACTED:user]", "a read shows the masked prompt");
    let brief = cctui_server_sessions::uploads::RawUpload {
        name: "brief.md".into(),
        bytes: b"# brief".to_vec().into(),
        content_type: Some("text/markdown".into()),
    };
    cctui_server_sessions::presets::store::set_files::<crate::state::AppState>(
        &state.pool,
        Some(a.user),
        created.id,
        &[brief],
    )
    .await
    .unwrap();
    let before_seals = seals(&state.pool, created.id).await;
    assert!(before_seals.0.is_some() && before_seals.1.is_some());

    let Json(seen) =
        get_one(State(state.clone()), Extension(writer(&a)), Path(created.id)).await.unwrap();
    let mut sent = serde_json::to_value(&seen).unwrap();
    for key in ["id", "user_id", "next_fire_at", "last_fire_at", "created_at", "updated_at"] {
        sent.as_object_mut().unwrap().remove(key);
    }
    sent["external_sources"] = serde_json::Value::Bool(true);
    let round: LaunchPresetRequest =
        serde_json::from_value(sent).expect("every GET field is accepted by PUT");
    let Json(back) =
        update(State(state.clone()), Extension(writer(&a)), Path(created.id), Json(round))
            .await
            .unwrap();

    assert!(back.external_sources);
    assert_eq!(back.template, seen.template, "template kept, stored file names included");
    assert_eq!(back.template.attachment_names, ["brief.md"]);
    assert_eq!(back.template.effort.as_deref(), Some("high"));
    assert_eq!(back.prompt, seen.prompt);
    assert_eq!(back.env_keys, ["FEED_TOKEN"]);
    assert_eq!(seals(&state.pool, created.id).await, before_seals, "seals untouched");
    let key = cctui_server_sessions::presets::store::files_key(created.id);
    let (files, _) = cctui_server_sessions::routes::spawn::load_draft_uploads::<
        crate::state::AppState,
    >(&state.pool, &key, &[])
    .await
    .unwrap();
    assert_eq!(files.len(), 1);

    let legacy = LaunchPresetRequest {
        permission_mode: Some(PermissionMode::Auto),
        prompt: seen.prompt,
        ..body(&a, "round-trip")
    };
    let Json(untouched) =
        update(State(state.clone()), Extension(writer(&a)), Path(created.id), Json(legacy))
            .await
            .unwrap();
    assert_eq!(untouched.template, seen.template, "a client that sends no template keeps it");

    assert!(delete(State(state.clone()), Extension(writer(&a)), Path(created.id)).await.is_ok());
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_attachments WHERE session_id = $1")
            .bind(&key)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(left, 0, "deleting the preset drops its files");
}
