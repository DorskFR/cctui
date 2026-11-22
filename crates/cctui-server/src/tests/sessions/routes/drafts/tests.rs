use std::collections::BTreeSet;

use uuid::Uuid;

use super::*;

fn ctx(user_id: Uuid) -> AuthContext {
    AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes: BTreeSet::new() }
}

async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn insert_user(pool: &sqlx::PgPool, tag: &str) -> Uuid {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{tag}-{uid}"))
        .bind(format!("h-{tag}-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    uid
}

#[tokio::test]
async fn a_draft_round_trips_and_an_empty_put_deletes_it() {
    let Some(pool) = test_pool("a_draft_round_trips_and_an_empty_put_deletes_it").await else {
        return;
    };
    let uid = insert_user(&pool, "draft-rt").await;
    let state = AppState::for_test(pool);
    let key = format!("cctui_draft_{uid}");

    let put = put_draft(
        State(state.clone()),
        Extension(ctx(uid)),
        Path(key.clone()),
        Json(PutDraftRequest { text: "half a sentence".to_owned() }),
    )
    .await
    .unwrap();
    assert_eq!(put, StatusCode::NO_CONTENT);

    let got =
        get_draft(State(state.clone()), Extension(ctx(uid)), Path(key.clone())).await.unwrap();
    assert_eq!(got.0.text, "half a sentence");
    assert_eq!(got.0.key, key);

    let listed = list_drafts(State(state.clone()), Extension(ctx(uid))).await.unwrap();
    assert!(listed.0.drafts.iter().any(|d| d.key == key));

    let cleared = put_draft(
        State(state.clone()),
        Extension(ctx(uid)),
        Path(key.clone()),
        Json(PutDraftRequest { text: String::new() }),
    )
    .await
    .unwrap();
    assert_eq!(cleared, StatusCode::NO_CONTENT);
    let gone = get_draft(State(state), Extension(ctx(uid)), Path(key)).await;
    assert_eq!(gone.unwrap_err().status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn drafts_and_prompt_history_store_tagged_spans_masked() {
    let Some(pool) = test_pool("drafts_and_prompt_history_store_tagged_spans_masked").await else {
        return;
    };
    let uid = insert_user(&pool, "draft-redact").await;
    let state = AppState::for_test(pool.clone());
    let draft_key = format!("cctui_draft_{uid}");
    let history_key = cctui_proto::drafts::session_history_key(&format!("s-{uid}"));
    let cases = [
        (draft_key, "pw <REDACTED>hunter2-xyz</redacted> ok", "pw [REDACTED:user] ok"),
        (
            history_key,
            "[\"older\",\"pw <redacted>hunter2-xyz\\nline2</redacted>\"]",
            "[\"older\",\"pw [REDACTED:user]\"]",
        ),
        (
            cctui_proto::drafts::PROMPT_HISTORY_KEY.to_owned(),
            "[\"spawn with <redacted>hunter2-xyz</redacted>\"]",
            "[\"spawn with [REDACTED:user]\"]",
        ),
    ];
    for (key, sent, want) in cases {
        put_draft(
            State(state.clone()),
            Extension(ctx(uid)),
            Path(key.clone()),
            Json(PutDraftRequest { text: sent.to_owned() }),
        )
        .await
        .unwrap();
        let row: String =
            sqlx::query_scalar("SELECT text FROM user_drafts WHERE user_id = $1 AND key = $2")
                .bind(uid)
                .bind(&key)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!row.contains("hunter2-xyz"), "plaintext stored under {key}: {row}");
        assert_eq!(row, want);
    }
    let history = cctui_proto::drafts::session_history_key(&format!("s-{uid}"));
    let got = get_draft(State(state), Extension(ctx(uid)), Path(history)).await.unwrap();
    let parsed: Vec<String> = serde_json::from_str(&got.0.text).unwrap();
    assert_eq!(parsed, ["older", "pw [REDACTED:user]"]);
}

/// Unbounded drafts are a per-user storage leak: keys are client-chosen, so
/// nothing but this cap limits the row count.
#[tokio::test]
async fn drafts_past_the_cap_evict_the_least_recently_updated() {
    let Some(pool) = test_pool("drafts_past_the_cap_evict_the_least_recently_updated").await else {
        return;
    };
    let uid = insert_user(&pool, "draft-cap").await;
    let other = insert_user(&pool, "draft-cap-other").await;
    let state = AppState::for_test(pool.clone());

    // Pre-seed the cap with rows whose age is explicit, so which one is
    // "least recently updated" does not depend on statement timestamps.
    for i in 0..DRAFT_CAP_PER_USER {
        sqlx::query(
            "INSERT INTO user_drafts (user_id, key, text, updated_at) \
             VALUES ($1, $2, $3, now() - make_interval(secs => $4))",
        )
        .bind(uid)
        .bind(format!("k{i:04}"))
        .bind("text")
        .bind(f64::from(u32::try_from(DRAFT_CAP_PER_USER - i).expect("fits")))
        .execute(&pool)
        .await
        .unwrap();
    }
    // One row for the other user, to prove eviction is per user.
    sqlx::query(
        "INSERT INTO user_drafts (user_id, key, text, updated_at) \
         VALUES ($1, $2, $3, now() - make_interval(secs => 9999))",
    )
    .bind(other)
    .bind("theirs")
    .bind("text")
    .execute(&pool)
    .await
    .unwrap();

    let count = |uid: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_drafts WHERE user_id = $1")
                .bind(uid)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count(uid).await, i64::try_from(DRAFT_CAP_PER_USER).unwrap(), "at the cap");

    // The oldest of the seeded rows, which the next put must evict.
    let oldest = "k0000";
    put_draft(
        State(state.clone()),
        Extension(ctx(uid)),
        Path("brand-new".to_owned()),
        Json(PutDraftRequest { text: "newest".to_owned() }),
    )
    .await
    .unwrap();

    assert_eq!(
        count(uid).await,
        i64::try_from(DRAFT_CAP_PER_USER).unwrap(),
        "the cap holds instead of growing by one"
    );
    let kept = get_draft(State(state.clone()), Extension(ctx(uid)), Path("brand-new".to_owned()))
        .await
        .unwrap();
    assert_eq!(kept.0.text, "newest", "a put is never the row evicted");
    let evicted =
        get_draft(State(state.clone()), Extension(ctx(uid)), Path(oldest.to_owned())).await;
    assert_eq!(
        evicted.unwrap_err().status(),
        StatusCode::NOT_FOUND,
        "the least recently updated draft went"
    );
    let survivor =
        get_draft(State(state.clone()), Extension(ctx(uid)), Path("k0001".to_owned())).await;
    assert!(survivor.is_ok(), "the next-oldest is still inside the cap");

    assert_eq!(count(other).await, 1, "another user's older draft is untouched");

    // Updating an existing draft is not a new row, so nothing is evicted.
    put_draft(
        State(state.clone()),
        Extension(ctx(uid)),
        Path("brand-new".to_owned()),
        Json(PutDraftRequest { text: "edited".to_owned() }),
    )
    .await
    .unwrap();
    assert_eq!(count(uid).await, i64::try_from(DRAFT_CAP_PER_USER).unwrap());
    assert!(
        get_draft(State(state), Extension(ctx(uid)), Path("k0001".to_owned())).await.is_ok(),
        "an in-place update evicts nothing"
    );
}

/// The whole point of moving drafts server-side is that they are per user:
/// knowing another user's key must not read, overwrite or delete their text.
#[tokio::test]
async fn a_draft_is_invisible_to_another_user_who_knows_its_key() {
    let Some(pool) = test_pool("a_draft_is_invisible_to_another_user_who_knows_its_key").await
    else {
        return;
    };
    let owner = insert_user(&pool, "draft-owner").await;
    let other = insert_user(&pool, "draft-other").await;
    let state = AppState::for_test(pool);
    let key = format!("cctui_draft_shared_{owner}");

    let saved = put_draft(
        State(state.clone()),
        Extension(ctx(owner)),
        Path(key.clone()),
        Json(PutDraftRequest { text: "owner's secret".to_owned() }),
    )
    .await
    .unwrap();
    assert_eq!(saved, StatusCode::NO_CONTENT);

    let peek = get_draft(State(state.clone()), Extension(ctx(other)), Path(key.clone())).await;
    assert_eq!(peek.unwrap_err().status(), StatusCode::NOT_FOUND);

    let listed = list_drafts(State(state.clone()), Extension(ctx(other))).await.unwrap();
    assert!(listed.0.drafts.iter().all(|d| d.key != key));

    let swept =
        delete_draft(State(state.clone()), Extension(ctx(other)), Path(key.clone())).await.unwrap();
    assert_eq!(swept, StatusCode::NO_CONTENT);
    let trampled = put_draft(
        State(state.clone()),
        Extension(ctx(other)),
        Path(key.clone()),
        Json(PutDraftRequest { text: "trampled".to_owned() }),
    )
    .await
    .unwrap();
    assert_eq!(trampled, StatusCode::NO_CONTENT);

    let still = get_draft(State(state), Extension(ctx(owner)), Path(key)).await.unwrap();
    assert_eq!(still.0.text, "owner's secret", "another user's PUT wrote its own row");
}
