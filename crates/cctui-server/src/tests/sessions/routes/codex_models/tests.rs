use super::*;

/// Answers one request per entry of `bodies`, in order, and hands back the
/// raw requests it saw.
async fn mock_http(bodies: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut seen = Vec::new();
        for body in bodies {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        }
        seen
    });
    (format!("http://{addr}/"), handle)
}

async fn one_shot(body: &'static str) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    mock_http(vec![body.to_owned()]).await
}

#[tokio::test]
async fn the_latest_version_is_read_from_the_npm_dist_tag() {
    let (url, served) = one_shot(r#"{"name":"@openai/codex","version":"0.156.1"}"#).await;
    let client = crate::build_http_client();
    assert_eq!(fetch_latest_codex_version(&client, &url).await.as_deref(), Some("0.156.1"));
    assert!(served.await.unwrap()[0].starts_with("GET /"));
}

#[tokio::test]
async fn an_unreachable_npm_leaves_the_caller_with_no_version() {
    let client = crate::build_http_client();
    let dead = "http://127.0.0.1:1/";
    assert_eq!(fetch_latest_codex_version(&client, dead).await, None);
    assert_eq!(resolve_client_version(None, None), BUNDLED_CODEX_CLIENT_VERSION);
}

#[tokio::test]
async fn the_catalog_read_carries_the_resolved_client_version() {
    let (url, served) = one_shot(r#"{"models":[{"id":"gpt-6-astra"}]}"#).await;
    let client = crate::build_http_client();
    let resp = catalog_request(&client, &url, "tok", "acct-1", "0.156.1")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(parse_remote_catalog(&resp).models[0].id, "gpt-6-astra");
    let req = served.await.unwrap().remove(0);
    assert!(req.contains("client_version=0.156.1"), "{req}");
    assert!(req.contains("authorization: Bearer tok") || req.contains("Authorization: Bearer tok"));
    assert!(req.contains("chatgpt-account-id: acct-1"));
}

#[tokio::test]
async fn a_moved_version_is_reported_once_and_is_cached_after() {
    if codex_version_pin().is_some() {
        return;
    }
    let (url, served) = mock_http(vec![
        r#"{"version":"0.157.0"}"#.to_owned(),
        r#"{"version":"0.157.0"}"#.to_owned(),
    ])
    .await;
    let client = crate::build_http_client();
    let cache = std::sync::Mutex::new(None);

    let (version, moved) = resolve_latest_into(&client, &url, &cache).await;
    assert_eq!(version, "0.157.0");
    assert!(moved, "moving off the bundled {BUNDLED_CODEX_CLIENT_VERSION} is a refetch");
    assert_eq!(cache.lock().unwrap().as_ref().unwrap().version, "0.157.0");

    let (version, moved) = resolve_latest_into(&client, &url, &cache).await;
    assert_eq!(version, "0.157.0");
    assert!(!moved, "an unchanged version must not refetch the catalogs");
    assert_eq!(served.await.unwrap().len(), 2);
}

#[tokio::test]
async fn an_unreachable_npm_keeps_the_last_good_version_and_never_moves() {
    if codex_version_pin().is_some() {
        return;
    }
    let (url, served) = one_shot(r#"{"version":"0.157.0"}"#).await;
    let client = crate::build_http_client();
    let cache = std::sync::Mutex::new(None);
    assert!(resolve_latest_into(&client, &url, &cache).await.1);
    served.await.unwrap();

    let (version, moved) = resolve_latest_into(&client, "http://127.0.0.1:1/", &cache).await;
    assert_eq!(version, "0.157.0");
    assert!(!moved);
    let stamped = cache.lock().unwrap().as_ref().unwrap().fetched_at;
    assert!(Utc::now() - stamped < LATEST_VERSION_TTL);
}
