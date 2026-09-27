//! In-process tunnel test: real server handler, a fake daemon that proxies
//! HTTP to a local upstream and echoes WebSocket messages.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp, PreviewChunk, PreviewHeader};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{PreviewHost, Registry};
use crate::state::AppState;

const HOST: &str = "cctui-pv-{id}.example.test";

async fn upstream() -> u16 {
    let app = Router::new()
        .route(
            "/echo",
            post(|body: String| async move { ([("x-upstream", "yes")], format!("echo:{body}")) }),
        )
        .route("/", get(|| async { "root" }))
        .route(
            "/cookies",
            get(|headers: axum::http::HeaderMap| async move {
                headers
                    .get_all("cookie")
                    .iter()
                    .map(|v| v.to_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
                    .join(" | ")
            }),
        )
        .fallback(|uri: axum::http::Uri| async move { format!("upstream:{uri}") });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

type Pending = (u16, String, String, Vec<PreviewHeader>, Vec<u8>);

async fn fake_daemon(
    state: AppState,
    machine: Uuid,
    user: Uuid,
    mut rx: mpsc::Receiver<DaemonFrameDown>,
) {
    let http = reqwest::Client::new();
    let mut pending: std::collections::HashMap<String, Pending> = std::collections::HashMap::new();
    let mut echo: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(frame) = rx.recv().await {
        match frame {
            DaemonFrameDown::PreviewRequest {
                stream_id,
                port,
                method,
                path,
                upgrade: true,
                ..
            } => {
                assert_eq!(method, "GET");
                assert_eq!(path, "/hmr?token=1");
                assert!(port >= 1024);
                echo.insert(stream_id.clone());
                super::on_frame(
                    &state,
                    machine,
                    user,
                    DaemonFrameUp::PreviewResponse { stream_id, status: 101, headers: vec![] },
                )
                .await;
            }
            DaemonFrameDown::PreviewRequest {
                stream_id,
                port,
                method,
                path,
                has_body,
                headers,
                ..
            } => {
                assert!(headers.iter().all(|h| h.name != "host" && h.name != "authorization"));
                assert!(headers.iter().all(|h| !h.value.contains("cctui_")));
                if has_body {
                    pending.insert(stream_id, (port, method, path, headers, Vec::new()));
                } else {
                    let req = Relay { port, method, path, headers, body: Vec::new() };
                    relay(&state, &http, machine, user, &stream_id, req).await;
                }
            }
            DaemonFrameDown::PreviewChunk(PreviewChunk { stream_id, data, text, end }) => {
                if echo.contains(&stream_id) {
                    let reply = DaemonFrameUp::PreviewChunk(PreviewChunk {
                        stream_id: stream_id.clone(),
                        data: data.clone(),
                        text,
                        end,
                    });
                    super::on_frame(&state, machine, user, reply).await;
                    if end {
                        echo.remove(&stream_id);
                    }
                    continue;
                }
                let Some(entry) = pending.get_mut(&stream_id) else { continue };
                entry.4.extend(BASE64.decode(data).unwrap());
                if end {
                    let (port, method, path, headers, body) = pending.remove(&stream_id).unwrap();
                    let req = Relay { port, method, path, headers, body };
                    relay(&state, &http, machine, user, &stream_id, req).await;
                }
            }
            _ => {}
        }
    }
}

struct Relay {
    port: u16,
    method: String,
    path: String,
    headers: Vec<PreviewHeader>,
    body: Vec<u8>,
}

async fn relay(
    state: &AppState,
    http: &reqwest::Client,
    machine: Uuid,
    user: Uuid,
    stream_id: &str,
    req: Relay,
) {
    let Relay { port, method, path, headers, body } = req;
    let mut upstream = http.request(
        reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("http://127.0.0.1:{port}{path}"),
    );
    for h in headers.iter().filter(|h| h.name != "content-length") {
        upstream = upstream.header(h.name.as_str(), h.value.as_str());
    }
    let resp = upstream.body(body).send().await.unwrap();
    let headers = resp
        .headers()
        .iter()
        .map(|(n, v)| PreviewHeader { name: n.to_string(), value: v.to_str().unwrap().to_owned() })
        .collect();
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.unwrap();
    super::on_frame(
        state,
        machine,
        user,
        DaemonFrameUp::PreviewResponse { stream_id: stream_id.to_owned(), status, headers },
    )
    .await;
    for part in bytes.chunks(3) {
        let chunk = PreviewChunk {
            stream_id: stream_id.to_owned(),
            data: BASE64.encode(part),
            text: false,
            end: false,
        };
        super::on_frame(state, machine, user, DaemonFrameUp::PreviewChunk(chunk)).await;
    }
    let end = PreviewChunk {
        stream_id: stream_id.to_owned(),
        data: String::new(),
        text: false,
        end: true,
    };
    super::on_frame(state, machine, user, DaemonFrameUp::PreviewChunk(end)).await;
}

struct Harness {
    state: AppState,
    port: u16,
    preview_id: String,
    user: Uuid,
    host: String,
}

/// `None` when no test database is configured (CI always has one).
async fn test_pool(name: &str) -> Option<sqlx::PgPool> {
    let url = crate::routes::gateway::test_db_url(name)?;
    Some(sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await.unwrap())
}

async fn test_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("preview-e2e-{id}"))
        .bind(format!("hash-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}

fn pod_state(pool: &sqlx::PgPool, pod: &str) -> AppState {
    let mut state = AppState::for_test(pool.clone());
    state.preview = Arc::new(Registry::new(
        Some(PreviewHost::parse(HOST).unwrap()),
        "http://localhost:8700",
        b"e2e",
    ));
    state.presence = Arc::new(crate::presence::PodIdentity::for_test(pod, "127.0.0.1"));
    state.internal_secret = Some(Arc::from("cluster-secret"));
    state
}

/// The public router of one replica: the preview host gate plus the internal
/// peer endpoint, exactly as `main` mounts them.
async fn serve_pod(state: &AppState) -> u16 {
    let app = Router::new()
        .route("/api/v1/ping", get(|| async { "api" }))
        .route(
            "/internal/preview/{id}/",
            axum::routing::any(crate::routes::internal::preview_serve_root),
        )
        .route(
            "/internal/preview/{id}/{*path}",
            axum::routing::any(crate::routes::internal::preview_serve),
        )
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(state.clone(), super::handler::host_gate));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

async fn harness() -> Option<Harness> {
    let pool = test_pool("preview_e2e").await?;
    let state = pod_state(&pool, "pod-a");
    let user = test_user(&pool).await;
    let (machine, conn) = (Uuid::new_v4(), Uuid::new_v4());
    let session = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(64);
    state.bus.register_daemon(machine, conn, tx);
    state.bus.bind_session_conn(&session, conn);
    tokio::spawn(fake_daemon(state.clone(), machine, user, rx));
    let upstream_port = upstream().await;
    let preview =
        state.preview.open(&pool, &session, user, machine, upstream_port, None).await.unwrap();

    let port = serve_pod(&state).await;
    let host = state.preview.host().unwrap().host_for(&preview.id);
    Some(Harness { state, port, preview_id: preview.id, user, host })
}

impl Harness {
    fn client() -> reqwest::Client {
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn cookie(&self) -> String {
        let ticket = self.state.preview.tickets().mint_ticket(&self.preview_id, self.user);
        let resp = Self::client()
            .get(self.url(&format!("/__cctui/auth?ticket={ticket}")))
            .header("host", &self.host)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 302);
        assert_eq!(resp.headers().get("location").unwrap(), "/");
        let set = resp.headers().get("set-cookie").unwrap().to_str().unwrap().to_owned();
        assert!(
            set.contains("HttpOnly") && set.contains("SameSite=Lax") && !set.contains("Domain")
        );
        set.split(';').next().unwrap().to_owned()
    }
}

#[tokio::test]
async fn preview_host_is_gated_and_tunnels_http() {
    let Some(h) = harness().await else { return };
    let client = Harness::client();

    let api = client.get(h.url("/api/v1/ping")).send().await.unwrap();
    assert_eq!(api.text().await.unwrap(), "api", "non-preview hosts keep the regular router");

    let unknown_host = format!("cctui-pv-{}.example.test", "zzzzzzzzzzzzzzzzzzzzzzzz");
    let resp = client.get(h.url("/")).header("host", &unknown_host).send().await.unwrap();
    assert_eq!(resp.status(), 404);

    let resp = client.get(h.url("/")).header("host", &h.host).send().await.unwrap();
    assert_eq!(resp.status(), 401, "no cookie");

    let cookie = h.cookie().await;
    let ticket = h.state.preview.tickets().mint_ticket(&h.preview_id, Uuid::new_v4());
    let resp = client
        .get(h.url(&format!("/__cctui/auth?ticket={ticket}")))
        .header("host", &h.host)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "ticket for another user");

    let resp = client
        .post(h.url("/echo"))
        .header("host", &h.host)
        .header("cookie", format!("{cookie}; cctui_auth=secret"))
        .header("authorization", "Bearer never-forwarded")
        .body("hello tunnel")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-upstream").unwrap(), "yes");
    assert_eq!(resp.text().await.unwrap(), "echo:hello tunnel");

    let resp = client
        .get(h.url("/"))
        .header("host", &h.host)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.text().await.unwrap(), "root");

    let other = h.state.preview.tickets().mint_cookie(&super::ticket::Grant {
        preview_id: Uuid::new_v4().simple().to_string(),
        user_id: h.user,
    });
    let resp = client
        .get(h.url("/"))
        .header("host", &h.host)
        .header("cookie", format!("cctui_preview={other}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "cookie for another preview");

    h.state.preview.close(&h.state.pool, &h.preview_id).await;
    let resp = client
        .get(h.url("/"))
        .header("host", &h.host)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "closed previews vanish");
    assert_eq!(h.state.preview.stream_count(), 0);
}

#[tokio::test]
async fn preview_websocket_is_passed_through() {
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let Some(h) = harness().await else { return };
    let cookie = h.cookie().await;
    let mut request =
        format!("ws://127.0.0.1:{}/hmr?token=1", h.port).into_client_request().unwrap();
    request.headers_mut().insert("host", h.host.parse().unwrap());
    request.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, resp) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(resp.status(), 101);
    ws.send(Message::Text("ping".into())).await.unwrap();
    let echoed = ws.next().await.unwrap().unwrap();
    assert_eq!(echoed, Message::Text("ping".into()));
    ws.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::Binary(vec![1, 2, 3].into()));
    ws.close(None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(h.state.preview.stream_count(), 0);

    let mut request =
        format!("ws://127.0.0.1:{}/hmr?token=1", h.port).into_client_request().unwrap();
    request.headers_mut().insert("host", h.host.parse().unwrap());
    let err = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
}

async fn assert_app_cookies_cross_the_hop(
    client: &reqwest::Client,
    url: &str,
    host: &str,
    cookie: &str,
) {
    let resp = client
        .get(url)
        .header("host", host)
        .header("cookie", format!("cctui_auth=secret; app=1; {cookie}; theme=dark"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "app=1; theme=dark",
        "the app's cookies survive the hop; cctui's never cross it"
    );
}

async fn assert_websocket_crosses_the_hop(port: u16, host: &str, cookie: &str) {
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = format!("ws://127.0.0.1:{port}/hmr?token=1").into_client_request().unwrap();
    request.headers_mut().insert("host", host.parse().unwrap());
    request.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, resp) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(resp.status(), 101, "the upgrade is tunnelled across both hops");
    ws.send(Message::Text("ping".into())).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::Text("ping".into()));
    ws.send(Message::Binary(vec![9, 8].into())).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::Binary(vec![9, 8].into()));
    ws.close(None).await.unwrap();
}

/// Without the cluster secret the internal leg is not usable at all, and the
/// forwarded identity must be the preview's owner.
async fn assert_internal_leg_is_guarded(client: &reqwest::Client, port: u16, preview_id: &str) {
    let naked = client
        .get(format!("http://127.0.0.1:{port}/internal/preview/{preview_id}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(naked.status(), 401, "the internal endpoint needs the cluster secret");

    let wrong_user = client
        .get(format!("http://127.0.0.1:{port}/internal/preview/{preview_id}/hello"))
        .bearer_auth("cluster-secret")
        .header(super::forward::USER_HEADER, Uuid::new_v4().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_user.status(), 403, "the forwarded identity must be the preview's owner");
}

/// Two replicas sharing one database: the daemon's WS is on pod A, the browser
/// hits pod B. Pod B must resolve the preview, gate it on the cookie, and
/// reverse-proxy both HTTP and WebSocket to pod A.
#[tokio::test]
async fn a_browser_on_the_wrong_pod_is_forwarded_to_the_pod_holding_the_daemon() {
    let Some(pool) = test_pool("preview_two_pods").await else { return };
    let user = test_user(&pool).await;
    let session = Uuid::new_v4();
    let (machine, conn) = (Uuid::new_v4(), Uuid::new_v4());

    // Pod A terminates the daemon WS and registers the preview.
    let pod_a = pod_state(&pool, "pod-a");
    let (tx, rx) = mpsc::channel(64);
    pod_a.bus.register_daemon(machine, conn, tx);
    pod_a.bus.bind_session_conn(&session.to_string(), conn);
    tokio::spawn(fake_daemon(pod_a.clone(), machine, user, rx));
    let upstream_port = upstream().await;
    let preview = pod_a
        .preview
        .open(&pool, &session.to_string(), user, machine, upstream_port, None)
        .await
        .unwrap();
    let port_a = serve_pod(&pod_a).await;

    // Pod B holds no link. Its peer lookups must land on pod A's port.
    let mut pod_b = pod_state(&pool, "pod-b");
    pod_b.config.port = port_a;
    let port_b = serve_pod(&pod_b).await;

    sqlx::query(
        "INSERT INTO ws_presence (kind, entity_id, pod, pod_ip) VALUES ('session', $1, $2, $3) \
         ON CONFLICT (kind, entity_id) DO UPDATE SET pod = EXCLUDED.pod, pod_ip = EXCLUDED.pod_ip",
    )
    .bind(session)
    .bind("pod-a")
    .bind("127.0.0.1")
    .execute(&pool)
    .await
    .unwrap();

    let host = pod_a.preview.host().unwrap().host_for(&preview.id);
    let client = Harness::client();
    let url_b = |path: &str| format!("http://127.0.0.1:{port_b}{path}");

    assert!(pod_b.preview.local(&preview.id).is_none(), "pod B must not think it holds the link");

    let unauthed = client.get(url_b("/")).header("host", &host).send().await.unwrap();
    assert_eq!(unauthed.status(), 401, "pod B gates on the cookie before forwarding");

    // The ticket is minted on pod B and redeemed on pod B: both sides of the
    // single-use check go through the shared table.
    let ticket = pod_b.preview.tickets().mint_ticket(&preview.id, user);
    let redeem = client
        .get(url_b(&format!("/__cctui/auth?ticket={ticket}")))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(redeem.status(), 302);
    let cookie = redeem
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_owned();

    let replay = client
        .get(url_b(&format!("/__cctui/auth?ticket={ticket}")))
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 403, "a ticket cannot be replayed on any pod");

    for (path, body) in [("/hello?x=1", "upstream:/hello?x=1"), ("/", "root")] {
        let resp =
            client.get(url_b(path)).header("host", &host).header("cookie", &cookie).send().await;
        let resp = resp.unwrap();
        assert_eq!(resp.status(), 200, "{path} is forwarded");
        assert_eq!(resp.text().await.unwrap(), body);
    }

    assert_app_cookies_cross_the_hop(&client, &url_b("/cookies"), &host, &cookie).await;
    assert_websocket_crosses_the_hop(port_b, &host, &cookie).await;
    assert_internal_leg_is_guarded(&client, port_a, &preview.id).await;

    sqlx::query("DELETE FROM ws_presence WHERE entity_id = $1")
        .bind(session)
        .execute(&pool)
        .await
        .unwrap();
    super::store::delete_session(&pool, &session.to_string()).await;
}

/// A rolling restart replaces both pods. The daemon comes back on a fresh pod
/// and re-announces its preview by id; the row is re-bound there and the
/// browser is served whichever new pod it lands on.
#[tokio::test]
async fn a_rolling_restart_keeps_the_preview_once_the_daemon_re_announces_it() {
    let Some(pool) = test_pool("preview_rollout").await else { return };
    let user = test_user(&pool).await;
    let session = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id) \
         VALUES ($1, $2, '/w', $3, 'claude-code')",
    )
    .bind(session.to_string())
    .bind(machine.to_string())
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    let upstream_port = upstream().await;
    let pod_a = pod_state(&pool, "pod-a");
    let (tx_a, _rx_a) = mpsc::channel(64);
    pod_a.bus.register_daemon(machine, Uuid::new_v4(), tx_a);
    crate::presence::register(&pod_a, crate::presence::Kind::Session, session).await;
    let preview = pod_a
        .preview
        .open(&pool, &session.to_string(), user, machine, upstream_port, None)
        .await
        .unwrap();

    // Pod A goes down: its WS close path detaches the machine's previews.
    pod_a.preview.detach_machine(&pool, machine).await;
    crate::presence::unregister(&pod_a, crate::presence::Kind::Session, session).await;
    drop(pod_a);

    // Fresh pods C and D share the database. The daemon reconnects to C.
    let pod_c = pod_state(&pool, "pod-c");
    let port_c = serve_pod(&pod_c).await;
    let mut pod_d = pod_state(&pool, "pod-d");
    pod_d.config.port = port_c;
    let port_d = serve_pod(&pod_d).await;

    let conn_c = Uuid::new_v4();
    let (tx_c, rx_c) = mpsc::channel(64);
    pod_c.bus.register_daemon(machine, conn_c, tx_c);
    crate::presence::register(&pod_c, crate::presence::Kind::Daemon, machine).await;
    tokio::spawn(fake_daemon(pod_c.clone(), machine, user, rx_c));
    super::on_frame(
        &pod_c,
        machine,
        user,
        DaemonFrameUp::PreviewOpen {
            request_id: Uuid::new_v4(),
            session_id: session.to_string(),
            port: upstream_port,
            preview_id: Some(preview.id.clone()),
        },
    )
    .await;
    pod_c.bus.bind_session_conn(&session.to_string(), conn_c);
    crate::presence::register(&pod_c, crate::presence::Kind::Session, session).await;

    let rebound = pod_c.preview.local(&preview.id).expect("pod C holds the re-bound preview");
    assert_eq!(rebound.id, preview.id, "the browser's URL survives the rollout");
    let detached: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT detached_at FROM previews WHERE id = $1")
            .bind(&preview.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(detached.is_none(), "the re-bind clears the detach marker");

    let host = pod_c.preview.host().unwrap().host_for(&preview.id);
    let client = Harness::client();
    let cookie = format!(
        "cctui_preview={}",
        pod_d
            .preview
            .tickets()
            .mint_cookie(&super::ticket::Grant { preview_id: preview.id.clone(), user_id: user })
    );
    for port in [port_d, port_c] {
        let resp = client
            .get(format!("http://127.0.0.1:{port}/hello"))
            .header("host", &host)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "served through pod on :{port}");
        assert_eq!(resp.text().await.unwrap(), "upstream:/hello");
    }

    sqlx::query("DELETE FROM ws_presence WHERE entity_id = $1 OR entity_id = $2")
        .bind(session)
        .bind(machine)
        .execute(&pool)
        .await
        .unwrap();
    super::store::delete_session(&pool, &session.to_string()).await;
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(session.to_string())
        .execute(&pool)
        .await
        .unwrap();
}
