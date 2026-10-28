//! The extracted HTTP dispatcher retains its external request/reply contract.
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use cctui_server_core::dispatchers::http::HttpDispatcher;
use cctui_server_core::dispatchers::{DispatchError, DispatchSpec, Dispatcher};
use serde_json::{Value, json};

type RecordedRequests = tokio::sync::mpsc::UnboundedReceiver<(HeaderMap, Value)>;

async fn fixture(
    status: StatusCode,
    response: Value,
) -> (HttpDispatcher, RecordedRequests, tokio::task::JoinHandle<()>) {
    cctui_server_core::client::install_crypto_provider();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().route(
        "/dispatch",
        axum::routing::post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let response = response.clone();
            tx.send((headers, body)).unwrap();
            async move { (status, axum::Json(response)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/dispatch", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client =
        reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap();
    (HttpDispatcher::new("remote", url, Some("fixture-token".into()), client), rx, server)
}

#[tokio::test]
async fn http_dispatch_preserves_payload_auth_and_queue_status() {
    let (dispatcher, mut rx, server) = fixture(StatusCode::OK, json!({
        "handle": "job/worker", "namespace": "automation", "status": "deduplicated", "queue_position": 3
    })).await;
    let payload = json!({"prompt_file": "review", "env": {"TEST_SECRET": "fixture"}});
    let reply = dispatcher
        .dispatch(&DispatchSpec {
            session_id: "run-http",
            timeout_minutes: Some(15),
            reply_url: Some("https://example.invalid/reply"),
            dedup_key: Some("worker-dedup"),
            payload: &payload,
        })
        .await
        .unwrap();
    let (headers, body) = rx.recv().await.unwrap();
    assert_eq!(headers["authorization"], "Bearer fixture-token");
    assert_eq!(
        body,
        json!({"session_id": "run-http", "timeout_minutes": 15,
        "reply_url": "https://example.invalid/reply", "payload": payload})
    );
    assert_eq!(reply.handle, "job/worker");
    assert_eq!(reply.namespace.as_deref(), Some("automation"));
    assert_eq!(reply.status.as_deref(), Some("deduplicated"));
    assert_eq!(reply.queue_position, Some(3));
    server.abort();
}

#[tokio::test]
async fn older_http_dispatchers_can_omit_status_and_optional_fields() {
    let (dispatcher, mut rx, server) =
        fixture(StatusCode::OK, json!({"handle": "legacy/worker"})).await;
    let reply = dispatcher
        .dispatch(&DispatchSpec {
            session_id: "run-legacy",
            timeout_minutes: None,
            reply_url: None,
            dedup_key: None,
            payload: &json!({}),
        })
        .await
        .unwrap();
    assert_eq!(rx.recv().await.unwrap().1, json!({"session_id": "run-legacy", "payload": {}}));
    assert_eq!(reply.status, None);
    assert_eq!(reply.queue_position, None);
    assert_eq!(reply.namespace, None);
    server.abort();
}

#[tokio::test]
async fn http_dispatch_distinguishes_invalid_input_from_backend_failure() {
    for status in [StatusCode::BAD_REQUEST, StatusCode::SERVICE_UNAVAILABLE] {
        let (dispatcher, _rx, server) = fixture(status, json!({"error": "refused"})).await;
        let error = dispatcher
            .dispatch(&DispatchSpec {
                session_id: "run-refused",
                timeout_minutes: None,
                reply_url: None,
                dedup_key: None,
                payload: &json!({}),
            })
            .await
            .unwrap_err();
        assert_eq!(
            matches!(error, DispatchError::InvalidIntent(_)),
            status == StatusCode::BAD_REQUEST
        );
        assert_eq!(matches!(error, DispatchError::Backend(_)), status != StatusCode::BAD_REQUEST);
        server.abort();
    }
}
