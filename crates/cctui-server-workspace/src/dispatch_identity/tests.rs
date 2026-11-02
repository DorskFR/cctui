use cctui_server_core::test_support::with_isolated_database;

use super::*;

async fn provisioned(pool: &PgPool) {
    cctui_server_core::schema::bootstrap(pool).await.unwrap();
    let mut conn = cctui_server_core::schema::migration_connection(pool).await.unwrap();
    crate::schema::provision(&mut conn).await.unwrap();
}

fn request(dispatch_id: Uuid) -> RegisterRequest {
    RegisterRequest {
        dispatch_id,
        session_id: "sess-1".to_owned(),
        namespace: "cctui".to_owned(),
        job_name: "cctui-worker-0123456789ab".to_owned(),
        machine_key: "cctui_m_real".to_owned(),
        env: BTreeMap::from([("FEATURE_FLAG".to_owned(), "on".to_owned())]),
    }
}

async fn register_row(pool: &PgPool, req: &RegisterRequest) -> bool {
    let r = Registration {
        req,
        dispatcher_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        encrypted_machine_key: "sealed-key".to_owned(),
        encrypted_env: "sealed-env".to_owned(),
    };
    insert(pool, &r).await.unwrap()
}

fn pod(name: &str) -> jwt::PodIdentity {
    jwt::PodIdentity {
        issuer: "https://oidc.example.test".to_owned(),
        namespace: "cctui".to_owned(),
        pod_name: name.to_owned(),
        pod_uid: "pod-uid".to_owned(),
        service_account: Some("worker".to_owned()),
    }
}

#[tokio::test]
async fn the_workspace_package_installs_the_table_once() {
    with_isolated_database("dispatch_identity_migration", |pool| async move {
        provisioned(&pool).await;
        let present: bool =
            sqlx::query_scalar("SELECT to_regclass('public.dispatch_identities') IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(present);
        let mut conn = cctui_server_core::schema::migration_connection(&pool).await.unwrap();
        crate::schema::provision(&mut conn).await.unwrap();
        drop(conn);
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _cctui_schema_migrations WHERE package = 'workspace' \
             ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(versions, [1, 2]);

        sqlx::raw_sql("DROP TABLE public.dispatch_identities").execute(&pool).await.unwrap();
        sqlx::raw_sql(crate::schema::MIGRATIONS[1].sql).execute(&pool).await.unwrap();
        assert!(register_row(&pool, &request(Uuid::new_v4())).await, "re-applied up works");
    })
    .await;
}

#[tokio::test]
async fn a_dispatch_id_registers_once() {
    with_isolated_database("dispatch_identity_register_once", |pool| async move {
        provisioned(&pool).await;
        let req = request(Uuid::new_v4());
        assert!(register_row(&pool, &req).await);
        assert!(!register_row(&pool, &req).await, "a second registration is refused");
    })
    .await;
}

#[tokio::test]
async fn a_redeem_is_single_use() {
    with_isolated_database("dispatch_identity_single_use", |pool| async move {
        provisioned(&pool).await;
        let req = request(Uuid::new_v4());
        register_row(&pool, &req).await;
        let pod = pod("cctui-worker-0123456789ab-x7k2p");

        let first = claim(&pool, req.dispatch_id, &pod).await.unwrap();
        assert_eq!(first.as_deref(), Some("sealed-key"));
        let replay = claim(&pool, req.dispatch_id, &pod).await.unwrap();
        assert_eq!(replay, None, "the same token cannot redeem twice");

        let bound = binding(&pool, req.dispatch_id).await.unwrap().unwrap();
        assert!(bound.redeemed);
        let (key, pod_name): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT encrypted_machine_key, pod_name FROM dispatch_identities \
             WHERE dispatch_id = $1",
        )
        .bind(req.dispatch_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(key, None, "the key is not kept after redeem");
        assert_eq!(pod_name.as_deref(), Some("cctui-worker-0123456789ab-x7k2p"));
    })
    .await;
}

#[tokio::test]
async fn concurrent_redeems_hand_out_the_key_once() {
    with_isolated_database("dispatch_identity_concurrent", |pool| async move {
        provisioned(&pool).await;
        let req = request(Uuid::new_v4());
        register_row(&pool, &req).await;
        let pod = pod("cctui-worker-0123456789ab-x7k2p");
        let (a, b) =
            tokio::join!(claim(&pool, req.dispatch_id, &pod), claim(&pool, req.dispatch_id, &pod));
        let won = [a.unwrap(), b.unwrap()].into_iter().flatten().count();
        assert_eq!(won, 1);
    })
    .await;
}

#[tokio::test]
async fn an_expired_registration_cannot_be_redeemed() {
    with_isolated_database("dispatch_identity_expired", |pool| async move {
        provisioned(&pool).await;
        let req = request(Uuid::new_v4());
        register_row(&pool, &req).await;
        sqlx::query(
            "UPDATE dispatch_identities SET expires_at = now() - interval '1 minute' \
             WHERE dispatch_id = $1",
        )
        .bind(req.dispatch_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(binding(&pool, req.dispatch_id).await.unwrap().unwrap().expired);
        let pod = pod("cctui-worker-0123456789ab-x7k2p");
        assert_eq!(claim(&pool, req.dispatch_id, &pod).await.unwrap(), None);

        assert!(register_row(&pool, &request(Uuid::new_v4())).await);
        assert!(binding(&pool, req.dispatch_id).await.unwrap().is_none(), "pruned on register");
    })
    .await;
}

#[tokio::test]
async fn the_env_is_read_once_by_the_redeemed_key_only() {
    with_isolated_database("dispatch_identity_env", |pool| async move {
        provisioned(&pool).await;
        let req = request(Uuid::new_v4());
        register_row(&pool, &req).await;
        assert_eq!(
            take_env(&pool, req.dispatch_id, &req.machine_key).await.unwrap(),
            None,
            "nothing before redeem"
        );
        claim(&pool, req.dispatch_id, &pod("cctui-worker-0123456789ab-x7k2p")).await.unwrap();
        assert_eq!(take_env(&pool, req.dispatch_id, "another-key").await.unwrap(), None);
        assert_eq!(
            take_env(&pool, req.dispatch_id, &req.machine_key).await.unwrap().as_deref(),
            Some("sealed-env")
        );
        assert_eq!(take_env(&pool, req.dispatch_id, &req.machine_key).await.unwrap(), None);
    })
    .await;
}

#[test]
fn registrations_name_real_kubernetes_objects_and_env_vars() {
    assert!(check_register(&request(Uuid::nil())).is_ok());
    let mut bad = request(Uuid::nil());
    bad.namespace = "Not_A_Namespace".to_owned();
    assert!(check_register(&bad).unwrap_err().contains("namespace"));
    let mut bad = request(Uuid::nil());
    bad.job_name = "-x".to_owned();
    assert!(check_register(&bad).unwrap_err().contains("job_name"));
    let mut bad = request(Uuid::nil());
    bad.machine_key = String::new();
    assert!(check_register(&bad).is_err());
    let mut bad = request(Uuid::nil());
    bad.env.insert("BAD-NAME".to_owned(), "x".to_owned());
    assert!(check_register(&bad).unwrap_err().contains("BAD-NAME"));
}

#[test]
fn a_rejection_names_its_reason_and_status() {
    let id = Uuid::nil();
    let (status, body) = rejected(id, &jwt::Rejection::UntrustedIssuer("https://x".into()));
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.error.contains("CCTUI_DISPATCH_TRUSTED_ISSUERS"), "{}", body.error);
    let (status, _) = rejected(id, &jwt::Rejection::WrongPod { job: "j".into(), pod: "p".into() });
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) =
        rejected(id, &jwt::Rejection::Keys { issuer: "i".into(), error: "down".into() });
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}
