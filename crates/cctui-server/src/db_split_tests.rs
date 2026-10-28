use super::*;
use cctui_server_core::schema::{self, Origin};
use cctui_server_core::test_support::with_isolated_database;
use serde_json::Value;
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
        migrate(&pool).await.unwrap();
        assert_eq!(execution_data(&pool).await,before);
        let after: Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(after,history);
        let data: (i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM session_attachments), (SELECT count(*) FROM session_images), (SELECT count(*) FROM stream_events)")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(data,(1,1,1));
        let media: (String,String,Vec<u8>)=sqlx::query_as("SELECT a.name,e.payload->>'text',i.bytes FROM session_attachments a JOIN stream_events e USING (session_id) JOIN session_images i USING (session_id)")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(media,("fixture.txt".into(),"fixture transcript".into(),vec![1,2,3]));
        let modes: Vec<String>=sqlx::query_scalar("SELECT installation FROM _cctui_schema_migrations ORDER BY package")
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
        migrate(&pool).await.unwrap();
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
            migrate(&reference).await.unwrap();
            assert_eq!(schema_shape(&reference).await, shape);
        })
        .await;
        migrate(&pool).await.unwrap();
        schema::bootstrap(&pool).await.unwrap();
    })
    .await;
}
