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
