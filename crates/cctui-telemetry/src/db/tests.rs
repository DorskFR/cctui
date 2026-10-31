use opentelemetry::Value;
use opentelemetry::trace::{SpanKind, Status, TracerProvider as _};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tracing::Instrument;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

use super::{classify, traced};

const SENTINEL: &str = "sentinel-bind-5f1c9a";

fn test_db_url(test_name: &str) -> Option<String> {
    let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty());
    if url.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "{test_name}: DATABASE_URL/TEST_DATABASE_URL must point at a database in CI"
        );
        eprintln!("skipping {test_name}: no DATABASE_URL/TEST_DATABASE_URL");
    }
    url
}

#[test]
fn classify_select_from() {
    assert_eq!(
        classify("SELECT machine_uuid FROM sessions WHERE id = $1"),
        ("SELECT", Some("sessions"))
    );
    assert_eq!(
        classify(
            "SELECT DISTINCT a.name FROM account_providers ap \
             JOIN accounts a ON a.id = ap.account_id WHERE ap.family = $2"
        ),
        ("SELECT", Some("account_providers"))
    );
    assert_eq!(classify(r#"select x from "Quoted" where y = 1"#), ("SELECT", Some("Quoted")));
    assert_eq!(classify("SELECT 'FROM decoy' AS s FROM machines"), ("SELECT", Some("machines")));
}

#[test]
fn classify_select_skips_nested_and_function_froms() {
    assert_eq!(
        classify("SELECT EXTRACT(EPOCH FROM now()) AS t FROM sessions"),
        ("SELECT", Some("sessions"))
    );
    assert_eq!(
        classify("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = $1)"),
        ("SELECT", Some("sessions"))
    );
    assert_eq!(classify("SELECT EXTRACT(EPOCH FROM now())"), ("SELECT", None));
    assert_eq!(classify("SELECT x FROM unnest($1::text[]) AS x"), ("SELECT", None));
}

#[test]
fn classify_insert_into() {
    assert_eq!(
        classify(
            "INSERT INTO machine_launches (command_id,session_id) VALUES ($1,$2) \
             ON CONFLICT (command_id) DO NOTHING"
        ),
        ("INSERT", Some("machine_launches"))
    );
    assert_eq!(
        classify("insert into session_spawner_intents(spawn_key) values ($1)"),
        ("INSERT", Some("session_spawner_intents"))
    );
}

#[test]
fn classify_update() {
    assert_eq!(
        classify("UPDATE machine_launches SET state='delivered' WHERE command_id=$1"),
        ("UPDATE", Some("machine_launches"))
    );
    assert_eq!(classify("UPDATE ONLY sessions SET name = $2"), ("UPDATE", Some("sessions")));
}

#[test]
fn classify_delete_from() {
    assert_eq!(
        classify("DELETE FROM session_spawner_intents WHERE created_at < now() - interval '1 day'"),
        ("DELETE", Some("session_spawner_intents"))
    );
}

#[test]
fn classify_with_cte_names_the_main_statement() {
    assert_eq!(
        classify(
            "WITH claimed AS (DELETE FROM session_spawner_intents WHERE spawn_key = $1 \
                               RETURNING user_id), \
                  named AS (SELECT user_id FROM claimed LIMIT 1) \
             UPDATE sessions SET spawned_by = (SELECT user_id FROM named) WHERE id = $1"
        ),
        ("UPDATE", Some("sessions"))
    );
    assert_eq!(
        classify("WITH RECURSIVE recent AS (SELECT id FROM sessions) SELECT count(*) FROM recent"),
        ("SELECT", Some("recent"))
    );
}

#[test]
fn classify_skips_leading_comments_and_whitespace() {
    assert_eq!(
        classify("  \n\t-- lookup FROM decoy\n/* UPDATE decoy */ select status from sessions"),
        ("SELECT", Some("sessions"))
    );
}

#[test]
fn classify_unknown_statements() {
    assert_eq!(classify(""), ("UNKNOWN", None));
    assert_eq!(classify("-- only a comment"), ("UNKNOWN", None));
    assert_eq!(classify("BEGIN"), ("BEGIN", None));
    assert_eq!(classify("WITH x AS (SELECT 1)"), ("WITH", None));
}

#[tokio::test]
async fn traced_passes_the_query_result_through() {
    let ok: Result<u8, ()> = traced("SELECT 1", |_| async { Ok(1) }).await;
    assert_eq!(ok, Ok(1));
    let err: Result<u8, &str> = traced("SELECT 1", |_| async { Err("boom") }).await;
    assert_eq!(err, Err("boom"));
}

fn attr(span: &SpanData, key: &str) -> Option<Value> {
    span.attributes.iter().find(|kv| kv.key.as_str() == key).map(|kv| kv.value.clone())
}

fn leaks(span: &SpanData) -> bool {
    let event_values = span.events.events.iter().flat_map(|e| e.attributes.iter());
    span.attributes.iter().chain(event_values).any(|kv| kv.value.as_str().contains(SENTINEL))
        || matches!(&span.status, Status::Error { description } if description.contains(SENTINEL))
}

#[tokio::test]
async fn traced_query_exports_a_db_span_without_bind_values() {
    let name = "traced_query_exports_a_db_span_without_bind_values";
    let Some(url) = test_db_url(name) else { return };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(exporter.clone()).build();
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("test"))
            .with_filter(crate::widen(tracing_subscriber::EnvFilter::new("error"))),
    );
    let _default = tracing::subscriber::set_default(subscriber);

    async {
        let found: Option<String> =
            traced("SELECT relname::text FROM pg_class WHERE relname = $1", |sql| {
                sqlx::query_scalar(sql).bind(SENTINEL).fetch_optional(&pool)
            })
            .await
            .expect("lookup runs");
        assert_eq!(found, None);
        let failed = traced("SELECT $1::int FROM pg_class LIMIT 1", |sql| {
            sqlx::query_scalar::<_, i32>(sql).bind(SENTINEL).fetch_one(&pool)
        })
        .await;
        assert!(failed.is_err(), "the sentinel is not an int");
    }
    .instrument(tracing::info_span!("handler"))
    .await;
    pool.close().await;

    let spans = exporter.get_finished_spans().expect("exporter readable");
    let handler = spans.iter().find(|s| s.name == "handler").expect("handler span exported");
    let queries: Vec<&SpanData> = spans.iter().filter(|s| s.name == "db.query").collect();
    assert_eq!(queries.len(), 2, "{spans:#?}");
    for span in &queries {
        assert_eq!(span.span_kind, SpanKind::Client);
        assert_eq!(span.parent_span_id, handler.span_context.span_id());
        assert_eq!(attr(span, "db.system"), Some(Value::from("postgresql")));
        assert_eq!(attr(span, "db.operation"), Some(Value::from("SELECT")));
        assert_eq!(attr(span, "db.sql.table"), Some(Value::from("pg_class")));
        assert!(attr(span, "db.statement").is_some_and(|v| v.as_str().contains("$1")));
        assert!(!leaks(span), "a bind value reached the span: {span:#?}");
    }
    assert_eq!(queries[0].status, Status::Unset);
    assert_eq!(attr(queries[0], "error"), None);
    assert!(matches!(queries[1].status, Status::Error { .. }), "{:?}", queries[1].status);
    assert_eq!(attr(queries[1], "error"), Some(Value::Bool(true)));
}
