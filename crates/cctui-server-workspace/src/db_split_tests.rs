use super::*;
use cctui_server_core::schema::{self, Origin};
use cctui_server_core::test_support::with_isolated_database;
use serde_json::Value;
use sha2::Digest as _;
use uuid::Uuid;

const KEY: &[u8] = &[42; 32];

async fn seed_execution(pool: &PgPool) {
    crate::auth::AuthConfig::new(vec!["fixture-upgrade-admin".into()], pool.clone())
        .seed_admin()
        .await;
    let uid: Uuid =
        sqlx::query_scalar("SELECT id FROM users LIMIT 1").fetch_one(pool).await.unwrap();
    let account = Uuid::new_v4();
    let provider = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_settings (user_id,data) VALUES ($1,'{\"fixture\":{\"preserved\":true}}')",
    )
    .bind(uid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO accounts (id,user_id,name,env_json) VALUES ($1,$2,'fixture-account',$3)",
    )
    .bind(account)
    .bind(uid)
    .bind(cctui_crypto::encrypt("{}", KEY))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO account_providers (id,account_id,user_id,provider,encrypted_access_token,auth_scheme,base_url) VALUES ($1,$2,$3,'openai',$4,'api_key','https://api.example.org/v1')")
        .bind(provider).bind(account).bind(uid)
        .bind(cctui_crypto::encrypt("fixture-provider-secret",KEY))
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO machines (id,user_id,name,key_hash) VALUES ($1,$2,'fixture-worker','fixture-machine-hash')")
        .bind(machine).bind(uid).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO sessions (id,user_id,machine_uuid,machine_id,working_dir,adapter_id,metadata) VALUES ('fixture-run',$1,$2,'fixture-worker','/tmp','codex','{\"fixture\":true}')")
        .bind(uid).bind(machine).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO session_tokens (session_id,user_id,account_id,token_hash,encrypted_token) VALUES ('fixture-run',$1,$2,'fixture-session-hash',$3)")
        .bind(uid).bind(provider).bind(cctui_crypto::encrypt("fixture-worker-token",KEY))
        .execute(pool).await.unwrap();
}

async fn execution_data(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object( \
        'users',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM users t), \
        'keys',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_keys t), \
        'grants',(SELECT jsonb_agg(to_jsonb(t) ORDER BY key_id,scope) FROM key_acls t), \
        'settings',(SELECT jsonb_agg(to_jsonb(t) ORDER BY user_id) FROM user_settings t), \
        'accounts',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM accounts t), \
        'providers',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM account_providers t), \
        'machines',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM machines t), \
        'sessions',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM sessions t), \
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM session_tokens t))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn assert_credentials(pool: &PgPool) {
    assert!(
        crate::auth::AuthConfig::new(vec![], pool.clone())
            .validate("fixture-upgrade-admin")
            .await
            .is_some()
    );
    let token: String = sqlx::query_scalar("SELECT encrypted_access_token FROM account_providers")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(cctui_crypto::decrypt(&token, KEY).as_deref(), Some("fixture-provider-secret"));
}

/// Forward core migrations may add columns during adoption: existing rows keep
/// every value they had and gain only NULLs.
fn without_added_columns(after: &Value, before: &Value) -> Value {
    let mut out = after.clone();
    for (table, rows) in out.as_object_mut().into_iter().flatten() {
        let Some(old_rows) = before.get(table).and_then(Value::as_array) else { continue };
        for (row, old) in rows.as_array_mut().into_iter().flatten().zip(old_rows) {
            let (Some(row), Some(old)) = (row.as_object_mut(), old.as_object()) else { continue };
            row.retain(|k, v| {
                let kept = old.contains_key(k);
                assert!(kept || v.is_null(), "{table}.{k} gained a value during adoption: {v}");
                kept
            });
        }
    }
    out
}

async fn schema_shape(pool: &PgPool) -> Value {
    sqlx::query_scalar("WITH tables AS ( \
        SELECT c.oid,c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
        WHERE n.nspname='public' AND c.relkind IN ('r','p') \
          AND c.relname NOT IN ('_sqlx_migrations','_cctui_schema_lineage','_cctui_schema_migrations') \
    ) SELECT jsonb_build_object( \
        'columns',(SELECT jsonb_agg(to_jsonb(r) ORDER BY relname,attname) FROM ( \
            SELECT t.relname,a.attname,format_type(a.atttypid,a.atttypmod) AS type, \
                   a.attnotnull,pg_get_expr(d.adbin,d.adrelid) AS expression \
            FROM tables t JOIN pg_attribute a ON a.attrelid=t.oid \
            LEFT JOIN pg_attrdef d ON d.adrelid=t.oid AND d.adnum=a.attnum \
            WHERE a.attnum>0 AND NOT a.attisdropped) r), \
        'constraints',(SELECT jsonb_agg(to_jsonb(r) ORDER BY relname,conname) FROM ( \
            SELECT t.relname,c.conname,pg_get_constraintdef(c.oid) AS definition \
            FROM tables t JOIN pg_constraint c ON c.conrelid=t.oid) r), \
        'indexes',(SELECT jsonb_agg(to_jsonb(r) ORDER BY relname,definition) FROM ( \
            SELECT t.relname,pg_get_indexdef(i.indexrelid) AS definition \
            FROM tables t JOIN pg_index i ON i.indrelid=t.oid) r), \
        'triggers',(SELECT jsonb_agg(to_jsonb(r) ORDER BY relname,tgname) FROM ( \
            SELECT t.relname,g.tgname,pg_get_triggerdef(g.oid) AS definition \
            FROM tables t JOIN pg_trigger g ON g.tgrelid=t.oid WHERE NOT g.tgisinternal) r))")
        .fetch_one(pool).await.unwrap()
}

#[test]
fn the_legacy_stream_stays_at_the_package_baseline() {
    assert_eq!(MIGRATOR.iter().map(|m| m.version).max(), Some(schema::LEGACY_BASELINE));
}

#[tokio::test]
async fn legacy_full_adoption_preserves_data_checksums_and_the_older_migrator() {
    with_isolated_database("legacy_full_upgrade",|pool| async move {
        MIGRATOR.run(&pool).await.unwrap();
        seed_execution(&pool).await;
        sqlx::raw_sql("INSERT INTO session_attachments (session_id,name,hash,size) VALUES ('fixture-run','fixture.txt','fixture-hash',7); \
            INSERT INTO session_images (session_id,sha256,media_type,byte_len,bytes) VALUES ('fixture-run','fixture-image','image/png',3,'\\x010203'); \
            INSERT INTO stream_events (session_id,event_type,payload) VALUES ('fixture-run','message','{\"text\":\"fixture transcript\"}');")
            .execute(&pool).await.unwrap();
        let before=execution_data(&pool).await;
        let history: Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool).await.unwrap();
        migrate(&pool, true).await.unwrap();
        assert_eq!(without_added_columns(&execution_data(&pool).await,&before),before);
        let after: Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(after,history);
        let data: (i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM session_attachments), (SELECT count(*) FROM session_images), (SELECT count(*) FROM stream_events)")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(data,(1,1,1));
        let media: (String,String,Vec<u8>)=sqlx::query_as("SELECT a.name,e.payload->>'text',i.bytes FROM session_attachments a JOIN stream_events e USING (session_id) JOIN session_images i USING (session_id)")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(media,("fixture.txt".into(),"fixture transcript".into(),vec![1,2,3]));
        let modes: Vec<String>=sqlx::query_scalar("SELECT installation FROM _cctui_schema_migrations WHERE version=1 ORDER BY package")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(modes,["adopted","adopted"]);
        assert_credentials(&pool).await;
        MIGRATOR.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO _sqlx_migrations (version,description,success,checksum,execution_time) VALUES (192,'unfinished fixture',false,'\\x00',0)")
            .execute(&pool).await.unwrap();
        assert!(schema::bootstrap(&pool).await.is_err(),"core refuses an unfinished legacy migration");
    }).await;
}

#[tokio::test]
async fn core_expansion_preserves_execution_data_and_matches_the_full_schema() {
    with_isolated_database("core_to_full", |pool| async move {
        schema::bootstrap(&pool).await.unwrap();
        seed_execution(&pool).await;
        let before = execution_data(&pool).await;
        migrate(&pool, true).await.unwrap();
        assert_eq!(execution_data(&pool).await, before);
        assert_credentials(&pool).await;
        let absent: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(absent, "expansion must not fabricate historical migration records");
        let mut conn = schema::migration_connection(&pool).await.unwrap();
        assert_eq!(schema::origin(&mut conn).await.unwrap(), Some(Origin::Core));
        drop(conn);
        let hosts: Value = sqlx::query_scalar(
            "SELECT value FROM instance_settings WHERE key='upstream_allowed_hosts'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(hosts, serde_json::json!(["api.example.org:443"]));
        let shape = schema_shape(&pool).await;
        with_isolated_database("full_schema_reference", |reference| async move {
            migrate(&reference, true).await.unwrap();
            assert_eq!(schema_shape(&reference).await, shape);
        })
        .await;
        migrate(&pool, true).await.unwrap();
        schema::bootstrap(&pool).await.unwrap();
    })
    .await;
}

async fn search_objects(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object( \
        'extensions',(SELECT COALESCE(jsonb_agg(extname ORDER BY extname),'[]') FROM pg_extension \
            WHERE extname IN ('pg_trgm','btree_gin')), \
        'indexes',(SELECT COALESCE(jsonb_agg(c.relname ORDER BY c.relname),'[]') \
            FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid \
            WHERE pg_get_indexdef(i.indexrelid) LIKE '%trgm_ops%'), \
        'stream',(SELECT COALESCE(jsonb_agg(version ORDER BY version),'[]') \
            FROM _cctui_schema_migrations WHERE package='workspace_search'))")
        .fetch_one(pool).await.unwrap()
}

/// Relations with their storage, so a rewrite or an index rebuild shows.
async fn catalog(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object( \
        'extensions',(SELECT jsonb_agg(jsonb_build_array(e.extname,e.extversion,n.nspname) ORDER BY e.extname) \
            FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace), \
        'relations',(SELECT jsonb_agg(jsonb_build_array(c.relname,c.relkind,c.relfilenode::text,c.reloptions) \
            ORDER BY c.relname) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
            WHERE n.nspname='public'), \
        'functions',(SELECT jsonb_agg(jsonb_build_array(p.proname,md5(p.prosrc)) ORDER BY p.proname,md5(p.prosrc)) \
            FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public'))")
        .fetch_one(pool).await.unwrap()
}

async fn ledger(pool: &PgPool) -> Vec<(String, i64, Vec<u8>, String)> {
    sqlx::query_as("SELECT package,version,checksum,installation FROM _cctui_schema_migrations \
        ORDER BY package,version")
        .fetch_all(pool).await.unwrap()
}

/// A session and one transcript row as ingest stores it with search off.
async fn ingest_transcript(pool: &PgPool, text: &str) -> (String, i64) {
    let (uid, machine) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO users (id,name,key_hash) VALUES ($1,$2,$3)")
        .bind(uid).bind(format!("searchless-{uid}")).bind(format!("h-{uid}"))
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO machines (id,user_id,name,key_hash) VALUES ($1,$2,'m',$3)")
        .bind(machine).bind(uid).bind(format!("mk-{machine}"))
        .execute(pool).await.unwrap();
    let sid = format!("searchless-{uid}");
    sqlx::query("INSERT INTO sessions (id,machine_id,machine_uuid,user_id,working_dir,status,adapter_id) \
        VALUES ($1,$2,$2,$3,'/w','active','claude-code')")
        .bind(&sid).bind(machine).bind(uid)
        .execute(pool).await.unwrap();
    let id: i64 = sqlx::query_scalar("INSERT INTO stream_events (session_id,event_type,payload,search_text) \
        VALUES ($1,'message',jsonb_build_object('role','assistant','text',$2::text),'') RETURNING id")
        .bind(&sid).bind(text)
        .fetch_one(pool).await.unwrap();
    (sid, id)
}

const NO_SEARCH: &str = r#"{"extensions":[],"indexes":[],"stream":[]}"#;

#[tokio::test]
async fn a_fresh_searchless_database_needs_no_search_extension_and_keeps_transcripts() {
    with_isolated_database("searchless_fresh", |pool| async move {
        migrate(&pool, false).await.unwrap();
        let none: Value = serde_json::from_str(NO_SEARCH).unwrap();
        assert_eq!(search_objects(&pool).await, none);
        let mut conn = schema::migration_connection(&pool).await.unwrap();
        assert_eq!(schema::origin(&mut conn).await.unwrap(), Some(Origin::Core));
        drop(conn);
        let baseline: Vec<u8> = sqlx::query_scalar(
            "SELECT checksum FROM _cctui_schema_migrations WHERE package='workspace' AND version=1")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(baseline, sha2::Sha256::digest(crate::schema::searchless_bootstrap().as_bytes()).to_vec());
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _cctui_schema_migrations WHERE package='workspace' ORDER BY version")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(versions, (1..=6).collect::<Vec<i64>>());

        let (sid, id) = ingest_transcript(&pool, "a searchless transcript").await;
        let read: (i64, String) = sqlx::query_as(
            "SELECT id, payload->>'text' FROM stream_events WHERE session_id=$1")
            .bind(&sid).fetch_one(&pool).await.unwrap();
        assert_eq!(read, (id, "a searchless transcript".to_owned()));

        let before = catalog(&pool).await;
        migrate(&pool, false).await.unwrap();
        assert_eq!(catalog(&pool).await, before, "a restart with search off changes nothing");
        assert_eq!(search_objects(&pool).await, none);
    })
    .await;
}

#[tokio::test]
async fn switching_search_on_over_a_searchless_database_finds_its_history() {
    with_isolated_database("searchless_switched_on", |pool| async move {
        migrate(&pool, false).await.unwrap();
        let (sid, id) = ingest_transcript(&pool, "a phrase from searchless history").await;

        migrate(&pool, true).await.unwrap();
        let on: Value = serde_json::json!({
            "extensions": ["btree_gin", "pg_trgm"],
            "indexes": ["idx_search_chunks_session_body_trgm", "idx_stream_events_session_search_trgm"],
            "stream": [1],
        });
        assert_eq!(search_objects(&pool).await, on);

        let mut walk = crate::search_index::Backfill::without_lag(crate::search_index::BACKFILL, 50);
        let mut steps = 0;
        while walk.step(&pool).await.unwrap() > 0 {
            steps += 1;
            assert!(steps < 100, "backfill never caught up");
        }
        let found: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT event_id FROM search_chunks \
             WHERE session_id=$1 AND body ILIKE '%phrase from searchless history%'")
            .bind(&sid).fetch_all(&pool).await.unwrap();
        assert_eq!(found, [id]);

        let before = catalog(&pool).await;
        migrate(&pool, true).await.unwrap();
        assert_eq!(catalog(&pool).await, before, "the search step applies once");
        let shape = schema_shape(&pool).await;
        with_isolated_database("searchless_full_reference", |reference| async move {
            migrate(&reference, true).await.unwrap();
            assert_eq!(schema_shape(&reference).await, shape);
        })
        .await;
    })
    .await;
}

/// The database a release without the search step leaves behind.
async fn current_without_search_step(pool: &PgPool, legacy: bool) {
    if legacy {
        MIGRATOR.run(pool).await.unwrap();
    }
    schema::bootstrap(pool).await.unwrap();
    let workspace = crate::schema::MIGRATIONS;
    let mut conn = schema::migration_connection(pool).await.unwrap();
    schema::install_package(&mut conn, "workspace", &workspace[..1], legacy).await.unwrap();
    schema::adopt(&mut conn, "workspace", workspace[1]).await.unwrap();
    schema::install_package(&mut conn, "workspace", workspace, legacy).await.unwrap();
}

#[tokio::test]
async fn the_search_step_changes_nothing_in_a_current_database() {
    for legacy in [true, false] {
        let name = if legacy { "search_step_legacy_full" } else { "search_step_core" };
        with_isolated_database(name, |pool| async move {
            current_without_search_step(&pool, legacy).await;
            ingest_transcript(&pool, "kept").await;
            let (before, shape, history) =
                (catalog(&pool).await, schema_shape(&pool).await, ledger(&pool).await);

            migrate(&pool, true).await.unwrap();
            assert_eq!(catalog(&pool).await, before, "legacy={legacy}");
            assert_eq!(schema_shape(&pool).await, shape, "legacy={legacy}");
            let mut after = ledger(&pool).await;
            let step: Vec<i64> =
                after.iter().filter(|r| r.0 == "workspace_search").map(|r| r.1).collect();
            assert_eq!(step, [1], "legacy={legacy}");
            after.retain(|r| r.0 != "workspace_search");
            assert_eq!(after, history, "legacy={legacy}");

            migrate(&pool, false).await.unwrap();
            assert_eq!(catalog(&pool).await, before, "search off drops nothing, legacy={legacy}");
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM stream_events")
                .fetch_one(&pool).await.unwrap();
            assert_eq!(rows, 1);
        })
        .await;
    }
}
