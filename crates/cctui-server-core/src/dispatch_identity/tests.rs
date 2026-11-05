use crate::test_support::with_isolated_database;

use super::*;

async fn provisioned(pool: &PgPool) {
    crate::schema::bootstrap(pool).await.unwrap();
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

async fn register_by(pool: &PgPool, req: &RegisterRequest, dispatcher_id: Uuid, user_id: Uuid) {
    let r = Registration {
        req,
        dispatcher_id,
        user_id,
        encrypted_machine_key: "sealed-key".to_owned(),
        encrypted_env: "sealed-env".to_owned(),
    };
    assert!(insert(pool, &r).await.unwrap());
}

async fn age_by_days(pool: &PgPool, days: i32) {
    for sql in [
        "UPDATE dispatch_identities SET created_at = created_at - make_interval(days => $1), \
         expires_at = expires_at - make_interval(days => $1)",
        "UPDATE auth_keys SET created_at = created_at - make_interval(days => $1), \
         expires_at = expires_at - make_interval(days => $1)",
    ] {
        sqlx::query(sql).bind(days).execute(pool).await.unwrap();
    }
}

#[tokio::test]
async fn a_held_queued_registration_outlives_the_ttl_while_an_unheld_one_still_expires() {
    with_isolated_database("dispatch_identity_hold", |pool| async move {
        provisioned(&pool).await;
        let (dispatcher, user) = (Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'dispatch-owner', 'h')")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        let mut queued = request(Uuid::new_v4());
        queued.machine_key = "cctui_m_queued".to_owned();
        let idle = request(Uuid::new_v4());
        register_by(&pool, &queued, dispatcher, user).await;
        register_by(&pool, &idle, dispatcher, user).await;
        sqlx::query(
            "INSERT INTO auth_keys (user_id, key_hash, kind, session_id, expires_at) \
             VALUES ($1, $2, 'ephemeral', 'sess-1', now() + interval '31 minutes')",
        )
        .bind(user)
        .bind(crate::auth::sha256_hex(&queued.machine_key))
        .execute(&pool)
        .await
        .unwrap();

        assert!(hold_queued(&pool, queued.dispatch_id, dispatcher).await.unwrap());
        assert!(
            !hold_queued(&pool, queued.dispatch_id, Uuid::new_v4()).await.unwrap(),
            "only the registering dispatcher can hold it"
        );
        age_by_days(&pool, 6).await;
        assert!(hold_queued(&pool, queued.dispatch_id, dispatcher).await.unwrap());
        age_by_days(&pool, 2).await;

        let pod = pod("cctui-worker-0123456789ab-x7k2p");
        assert!(binding(&pool, idle.dispatch_id).await.unwrap().unwrap().expired);
        assert_eq!(claim(&pool, idle.dispatch_id, &pod).await.unwrap(), None);
        assert!(
            !hold_queued(&pool, idle.dispatch_id, dispatcher).await.unwrap(),
            "an expired registration cannot be revived"
        );

        assert!(!binding(&pool, queued.dispatch_id).await.unwrap().unwrap().expired);
        let key_live: bool = sqlx::query_scalar(
            "SELECT expires_at > now() + interval '4 days' FROM auth_keys WHERE key_hash = $1",
        )
        .bind(crate::auth::sha256_hex(&queued.machine_key))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(key_live, "the sealed worker key is kept alive with its registration");
        let sealed = claim(&pool, queued.dispatch_id, &pod).await.unwrap();
        assert_eq!(sealed.as_deref(), Some("sealed-key"));
        rearm_held_key(&pool, queued.dispatch_id).await.unwrap();
        let ttl: f64 = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM (expires_at - now()))::float8 FROM auth_keys \
             WHERE key_hash = $1",
        )
        .bind(crate::auth::sha256_hex(&queued.machine_key))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!((1800.0..=1870.0).contains(&ttl), "the key gets its own lifetime back: {ttl}");
        assert!(
            !hold_queued(&pool, queued.dispatch_id, dispatcher).await.unwrap(),
            "a redeemed registration is no longer held"
        );
    })
    .await;
}
