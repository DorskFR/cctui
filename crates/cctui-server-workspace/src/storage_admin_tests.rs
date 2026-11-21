use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::{StorageDropRule, StorageLifecycleRules, StorageTrimRule};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::routes::storage_admin::{overview, preview_lifecycle, update_lifecycle};
use crate::state::AppState;

fn ctx(scopes: &[Scope]) -> AuthContext {
    AuthContext {
        user_id: Uuid::new_v4(),
        key_id: Uuid::nil(),
        machine_id: None,
        scopes: scopes.iter().copied().collect(),
    }
}

fn state() -> AppState {
    AppState::for_test(sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap())
}

fn rules() -> StorageLifecycleRules {
    StorageLifecycleRules {
        trim: Some(StorageTrimRule { enabled: true, after_days: 7, above_bytes: 10_240 }),
        drop: Some(StorageDropRule { enabled: true, after_days: 90 }),
    }
}

#[tokio::test]
async fn non_admins_get_403_before_any_query() {
    let non_admin: [&[Scope]; 3] =
        [&[], &[Scope::Read], &[Scope::Read, Scope::Dispatch, Scope::Enroll]];
    for scopes in non_admin {
        let denied = overview(State(state()), Extension(ctx(scopes))).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        let denied = update_lifecycle(State(state()), Extension(ctx(scopes)), Json(rules())).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        let denied = preview_lifecycle(State(state()), Extension(ctx(scopes)), Json(rules())).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn an_admin_gets_400_for_a_rule_out_of_range() {
    let admin = || Extension(ctx(&[Scope::Admin]));
    let mut bad = rules();
    bad.trim = Some(StorageTrimRule { enabled: true, after_days: 7, above_bytes: 10 });
    let denied = update_lifecycle(State(state()), admin(), Json(bad)).await;
    assert_eq!(denied.unwrap_err().status(), StatusCode::BAD_REQUEST);
    let mut bad = rules();
    bad.drop = Some(StorageDropRule { enabled: true, after_days: 0 });
    let denied = preview_lifecycle(State(state()), admin(), Json(bad)).await;
    assert_eq!(denied.unwrap_err().status(), StatusCode::BAD_REQUEST);
}
