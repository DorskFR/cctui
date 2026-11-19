use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::presets::LaunchPresetRequest;
use uuid::Uuid;

use super::{create, delete, get_one, list, update};
use crate::auth::{AuthContext, Scope};
use crate::presets::store_tests::{Owner, seed_owner};
use crate::state::AppState;

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
    let url = crate::routes::gateway::test_db_url(name)?;
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

fn env(pairs: &[(&str, &str)]) -> Option<cctui_proto::api::presets::EnvValues> {
    Some(cctui_proto::api::presets::EnvValues(
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
    ))
}

async fn stored_env(state: &AppState, id: Uuid) -> (Option<String>, crate::sealed_env::EnvMap) {
    let sealed: Option<String> = sqlx::query_scalar("SELECT sealed_env FROM presets WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let open = crate::sealed_env::unseal(sealed.as_deref(), crate::crypto::vault_key).unwrap();
    (sealed, open)
}

#[tokio::test]
async fn preset_env_is_write_only_and_edited_per_key() {
    let Some(state) = state("preset_env_is_write_only_and_edited_per_key").await else { return };
    crate::crypto::install_vault_key(vec![7u8; 32]);
    let a = seed_owner(&state.pool, "henv").await;
    let put = |id: Uuid, req: LaunchPresetRequest| {
        update(State(state.clone()), Extension(writer(&a)), Path(id), Json(req))
    };
    let first = LaunchPresetRequest {
        env: env(&[("KEEP", "keep-secret"), ("SWAP", "swap-secret"), ("GONE", "gone-secret")]),
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

    let round_trip =
        LaunchPresetRequest { env_keys: Some(seen.env_keys), ..body(&a, "digest") };
    put(created.id, round_trip).await.unwrap();
    assert_eq!(stored_env(&state, created.id).await.1, open, "GET names back: every value kept");

    let edit = LaunchPresetRequest {
        env_keys: Some(vec!["KEEP".into(), "SWAP".into()]),
        env: env(&[("SWAP", "swap-new"), ("ADDED", "added-new")]),
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

    let bad = LaunchPresetRequest { env: env(&[("lower", "x")]), ..body(&a, "bad") };
    let err = create(State(state), Extension(writer(&a)), Json(bad)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
}
