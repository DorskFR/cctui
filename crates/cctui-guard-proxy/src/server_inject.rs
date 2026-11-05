//! Machine-key injection for the cctui server host. The worker holds only the
//! placeholder; the proxy swaps it for the redeemed key on the way out and
//! masks the key wherever the server echoes it back.

use std::net::SocketAddr;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::dispatch::ServerTarget;
use crate::inject::{
    AuthShape, Dialer, Framing, HttpBuf, Idle, Injector, PerPodCa, PrefixedStream, Redial,
    ReplayTee, Sent, SniResolver, Step, TcpDialer, UpstreamConn, bad_gateway, body_framing,
    header_value, next_request, request_method, request_path, response_framing, response_status,
    rewrite_head, wants_close,
};

/// Must equal `cctui_proto::dispatch_identity::PLACEHOLDER_MACHINE_KEY`, which
/// the dispatcher hands the worker (checked by a test).
pub const PLACEHOLDER_MACHINE_KEY: &str = "cctui-dispatch-placeholder-key";
/// The daemon also presents its key in this request's JSON body, and the
/// server echoes it back as the session token.
const DAEMON_AUTH_PATH: &str = "/api/v1/daemon/auth";
const MAX_SWAP_BODY: usize = 64 * 1024;

enum Exchange<S> {
    Done(Step<S>),
    Upgraded(Box<UpstreamConn<S>>),
}

pub struct ServerInjector {
    target: ServerTarget,
    machine_key: String,
    acceptor: Option<TlsAcceptor>,
    connector: TlsConnector,
}

impl ServerInjector {
    pub fn new(
        target: ServerTarget,
        machine_key: String,
        ca: Option<Arc<PerPodCa>>,
    ) -> anyhow::Result<Self> {
        let acceptor = if target.https {
            let ca = ca.ok_or_else(|| anyhow::anyhow!("an https cctui server needs the pod CA"))?;
            crate::inject::install_crypto();
            let mut config = ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(SniResolver(ca)));
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Some(TlsAcceptor::from(Arc::new(config)))
        } else {
            None
        };
        let connector = Injector::build_connector(Vec::new())?;
        Ok(Self { target, machine_key, acceptor, connector })
    }

    pub fn matches(&self, host: &str, port: u16) -> bool {
        self.target.matches(host, port)
    }

    /// The proxy's own resolution of the configured host: the key only ever
    /// reaches the real server, whatever the worker dialed.
    async fn resolve(&self) -> anyhow::Result<Vec<SocketAddr>> {
        let addrs: Vec<SocketAddr> =
            tokio::net::lookup_host((self.target.host.as_str(), self.target.port))
                .await?
                .filter(SocketAddr::is_ipv4)
                .collect();
        anyhow::ensure!(!addrs.is_empty(), "{} has no IPv4 address", self.target.host);
        Ok(addrs)
    }

    pub async fn handle(&self, conn: TcpStream, prefix: Vec<u8>) -> anyhow::Result<()> {
        let addrs = self.resolve().await?;
        let host = self.target.host.as_str();
        let client = PrefixedStream::new(conn, prefix);
        let Some(acceptor) = &self.acceptor else {
            return Box::pin(self.pump(client, &TcpDialer { host, addrs: &addrs })).await;
        };
        let client = acceptor.accept(client).await?;
        let dialer = Dialer {
            connector: &self.connector,
            server_name: ServerName::try_from(self.target.host.clone())?,
            host,
            addrs: &addrs,
        };
        Box::pin(self.pump(client, &dialer)).await
    }

    fn outbound_head(&self, head: &[u8]) -> Vec<u8> {
        let placeholder = format!("Bearer {PLACEHOLDER_MACHINE_KEY}");
        let route = request_path(head)
            .is_some_and(|p| is_worker_route(request_method(head).as_deref(), &p));
        if route && header_value(head, "authorization") == Some(placeholder.as_bytes()) {
            rewrite_head(head, &AuthShape::Bearer, &self.machine_key, None)
        } else {
            head.to_vec()
        }
    }

    /// An upstream that closes an idle keep-alive is dropped and redialled on
    /// the next request: the worker must never see a bare EOF.
    pub async fn pump<C, D>(&self, mut client: C, dialer: &D) -> anyhow::Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin,
        D: Redial,
    {
        let mut cbuf = HttpBuf::default();
        let mut upstream: Option<UpstreamConn<D::Stream>> = None;
        loop {
            let head = match next_request(&mut cbuf, &mut client, upstream.as_mut()).await? {
                Idle::Request(head) => head,
                Idle::ClientClosed => return Ok(()),
                Idle::UpstreamClosed => {
                    tracing::debug!("cctui server {} closed an idle keep-alive", dialer.host());
                    upstream = None;
                    continue;
                }
            };
            let step =
                Box::pin(self.serve_one(&mut client, upstream.take(), dialer, &head, &mut cbuf))
                    .await?;
            match step {
                Exchange::Done(Step::KeepAlive(conn)) => upstream = Some(*conn),
                Exchange::Done(Step::Close) => return Ok(()),
                Exchange::Upgraded(mut conn) => {
                    client.write_all(&conn.ubuf.take_buffered()).await?;
                    conn.stream.write_all(&cbuf.take_buffered()).await?;
                    client.flush().await?;
                    conn.stream.flush().await?;
                    tokio::io::copy_bidirectional(&mut client, &mut conn.stream).await?;
                    return Ok(());
                }
            }
        }
    }

    async fn serve_one<C, D>(
        &self,
        client: &mut C,
        upstream: Option<UpstreamConn<D::Stream>>,
        dialer: &D,
        head: &[u8],
        cbuf: &mut HttpBuf,
    ) -> anyhow::Result<Exchange<D::Stream>>
    where
        C: AsyncRead + AsyncWrite + Unpin,
        D: Redial,
    {
        let upstream = if upstream.is_some() { upstream } else { dialer.dial().await };
        let Some(mut conn) = upstream else {
            bad_gateway(client).await?;
            return Ok(Exchange::Done(Step::Close));
        };
        let (sent, swapped) =
            Box::pin(self.send_request(client, &mut conn.stream, head, cbuf)).await?;
        let Some(resp) = conn.await_response(sent, dialer).await else {
            bad_gateway(client).await?;
            return Ok(Exchange::Done(Step::Close));
        };
        if response_status(&resp) == Some(101) {
            client.write_all(&resp).await?;
            return Ok(Exchange::Upgraded(Box::new(conn)));
        }
        let framing = response_framing(&resp, request_method(head).as_deref());
        if swapped {
            let Framing::Length(len) = framing else {
                anyhow::bail!("daemon auth answer has no bounded body; dropped, not leaked");
            };
            anyhow::ensure!(len <= MAX_SWAP_BODY, "daemon auth answer too large");
            let mut body = Vec::with_capacity(len);
            Box::pin(conn.ubuf.relay_body(&mut conn.stream, &mut body, framing)).await?;
            let body = mask_key(&body, &self.machine_key);
            client.write_all(&with_content_length(&resp, body.len())).await?;
            client.write_all(&body).await?;
        } else {
            client.write_all(&resp).await?;
            Box::pin(conn.ubuf.relay_body(&mut conn.stream, client, framing)).await?;
        }
        client.flush().await?;
        conn.reused = true;
        if framing == Framing::UntilClose || wants_close(head) || wants_close(&resp) {
            return Ok(Exchange::Done(Step::Close));
        }
        Ok(Exchange::Done(Step::KeepAlive(Box::new(conn))))
    }

    /// Writes the request through a replay tee. `true` alongside when the
    /// daemon auth key was swapped, so the answer must be masked.
    async fn send_request<C, U>(
        &self,
        client: &mut C,
        upstream: &mut U,
        head: &[u8],
        cbuf: &mut HttpBuf,
    ) -> anyhow::Result<(Sent, bool)>
    where
        C: AsyncRead + Unpin,
        U: AsyncWrite + Unpin,
    {
        let mut tee = ReplayTee::new(upstream);
        let outbound = self.outbound_head(head);
        let swapped = if request_path(head).as_deref() == Some(DAEMON_AUTH_PATH) {
            Box::pin(self.send_auth_body(cbuf, client, &mut tee, &outbound)).await?
        } else {
            tee.write_all(&outbound).await?;
            Box::pin(cbuf.relay_body(client, &mut tee, body_framing(head, false))).await?;
            false
        };
        tee.flush().await?;
        Ok((tee.into_sent(), swapped))
    }

    /// Forwards the daemon auth request, its placeholder key swapped for the
    /// real one. `true` when the swap happened, so the answer must be masked.
    async fn send_auth_body<C, U>(
        &self,
        cbuf: &mut HttpBuf,
        client: &mut C,
        upstream: &mut U,
        outbound: &[u8],
    ) -> anyhow::Result<bool>
    where
        C: AsyncRead + Unpin,
        U: AsyncWrite + Unpin,
    {
        let framing = body_framing(outbound, false);
        let Framing::Length(len) = framing else {
            upstream.write_all(outbound).await?;
            Box::pin(cbuf.relay_body(client, upstream, framing)).await?;
            return Ok(false);
        };
        anyhow::ensure!(len <= MAX_SWAP_BODY, "daemon auth body too large");
        let mut body = Vec::with_capacity(len);
        Box::pin(cbuf.relay_body(client, &mut body, framing)).await?;
        let Some(body) = swap_placeholder(&body, &self.machine_key) else {
            upstream.write_all(outbound).await?;
            upstream.write_all(&body).await?;
            return Ok(false);
        };
        upstream.write_all(&with_content_length(outbound, body.len())).await?;
        upstream.write_all(&body).await?;
        Ok(true)
    }
}

/// The server routes the worker's daemon and liveness probe call with the key.
fn is_worker_route(method: Option<&str>, path: &str) -> bool {
    let clean = !path.contains('%')
        && path.split('/').skip(1).all(|s| !s.is_empty() && s != "." && s != "..");
    clean
        && (path.starts_with("/api/v1/daemon/")
            || path == "/api/v1/manifest/daemon"
            || (path == "/api/v1/sessions" && method == Some("GET")))
}

fn swap_placeholder(body: &[u8], machine_key: &str) -> Option<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let field = value.get_mut("machine_key")?;
    if field.as_str() != Some(PLACEHOLDER_MACHINE_KEY) {
        return None;
    }
    *field = serde_json::Value::String(machine_key.to_owned());
    serde_json::to_vec(&value).ok()
}

fn mask_key(body: &[u8], machine_key: &str) -> Vec<u8> {
    let key = machine_key.as_bytes();
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if !key.is_empty() && body[i..].starts_with(key) {
            out.extend_from_slice(PLACEHOLDER_MACHINE_KEY.as_bytes());
            i += key.len();
        } else {
            out.push(body[i]);
            i += 1;
        }
    }
    out
}

fn with_content_length(head: &[u8], len: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut out = String::with_capacity(head.len() + 8);
    for line in text.split_inclusive("\r\n") {
        let name = line.split(':').next().unwrap_or_default().trim();
        if name.eq_ignore_ascii_case("content-length") {
            out.push_str("Content-Length: ");
            out.push_str(&len.to_string());
            out.push_str("\r\n");
        } else {
            out.push_str(line);
        }
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, DuplexStream, duplex};

    use super::*;
    use crate::inject::MAX_REPLAY_BYTES;

    const KEY: &str = "cctui_m_real_machine_key";

    fn injector() -> Arc<ServerInjector> {
        let target = ServerTarget::parse("http://cctui.cctui.svc:8700").unwrap();
        Arc::new(ServerInjector::new(target, KEY.to_owned(), None).unwrap())
    }

    /// Hands out pre-made upstream ends in order; `None` once exhausted.
    struct Duplexes {
        ends: Mutex<VecDeque<DuplexStream>>,
        dials: AtomicUsize,
    }

    impl Redial for Duplexes {
        type Stream = DuplexStream;

        fn host(&self) -> &str {
            "cctui.cctui.svc"
        }

        async fn dial(&self) -> Option<UpstreamConn<DuplexStream>> {
            self.dials.fetch_add(1, Ordering::SeqCst);
            let end = self.ends.lock().unwrap().pop_front();
            end.map(UpstreamConn::fresh)
        }
    }

    /// `(worker end, server ends in dial order, dialer)` with the injector
    /// pumping between them.
    fn wired_n(n: usize) -> (DuplexStream, VecDeque<DuplexStream>, Arc<Duplexes>) {
        let (worker, proxy_client) = duplex(64 * 1024);
        let mut ends = VecDeque::new();
        let mut servers = VecDeque::new();
        for _ in 0..n {
            let (proxy_upstream, server) = duplex(64 * 1024);
            ends.push_back(proxy_upstream);
            servers.push_back(server);
        }
        let dialer = Arc::new(Duplexes { ends: Mutex::new(ends), dials: AtomicUsize::new(0) });
        let (inj, d) = (injector(), dialer.clone());
        tokio::spawn(async move { Box::pin(inj.pump(proxy_client, &*d)).await });
        (worker, servers, dialer)
    }

    /// `(worker end, server end)` with the injector pumping between them.
    fn wired() -> (DuplexStream, DuplexStream) {
        let (worker, mut servers, _) = wired_n(1);
        (worker, servers.pop_front().unwrap())
    }

    async fn read_message(stream: &mut DuplexStream, is_response: bool) -> (String, Vec<u8>) {
        let mut buf = HttpBuf::default();
        let head = buf.read_head(stream).await.unwrap().unwrap();
        let mut body = Vec::new();
        let framing = if is_response {
            response_framing(&head, Some("GET"))
        } else {
            body_framing(&head, false)
        };
        if framing != Framing::UntilClose {
            Box::pin(buf.relay_body(stream, &mut body, framing)).await.unwrap();
        }
        (String::from_utf8(head).unwrap(), body)
    }

    #[test]
    fn the_placeholder_matches_what_the_dispatcher_hands_the_worker() {
        let proto = include_str!("../../cctui-proto/src/dispatch_identity.rs");
        let decl = format!("PLACEHOLDER_MACHINE_KEY: &str = \"{PLACEHOLDER_MACHINE_KEY}\";");
        assert!(proto.contains(&decl), "proto and proxy placeholders drifted");
    }

    #[tokio::test]
    async fn the_placeholder_bearer_leaves_as_the_real_key() {
        let (mut worker, mut server) = wired();
        worker
            .write_all(
                format!(
                    "GET /api/v1/sessions HTTP/1.1\r\nHost: cctui.cctui.svc:8700\r\n\
                     Authorization: Bearer {PLACEHOLDER_MACHINE_KEY}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (head, _) = read_message(&mut server, false).await;
        assert!(head.contains(&format!("Authorization: Bearer {KEY}")), "{head}");
        assert!(!head.contains(PLACEHOLDER_MACHINE_KEY), "{head}");

        server.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await.unwrap();
        let (resp, body) = read_message(&mut worker, true).await;
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert_eq!(body, b"{}");
    }

    #[test]
    fn worker_routes_cover_the_daemon_client_only() {
        for path in [
            "/api/v1/daemon/auth",
            "/api/v1/daemon/ws",
            "/api/v1/daemon/version",
            "/api/v1/daemon/binary/linux-amd64",
            "/api/v1/daemon/blobs/abc",
            "/api/v1/daemon/sessions/s-1/gateway-env",
            "/api/v1/daemon/sessions/s-1/spawn-child",
            "/api/v1/manifest/daemon",
        ] {
            assert!(is_worker_route(Some("GET"), path), "{path}");
        }
        assert!(is_worker_route(Some("GET"), "/api/v1/sessions"));
        assert!(!is_worker_route(Some("POST"), "/api/v1/sessions"));
        for path in [
            "/api/v1/sessions/dispatch",
            "/api/v1/sessions/spawn",
            "/api/v1/sessions/other/messages",
            "/api/v1/ws",
            "/api/v1/me",
            "/api/v1/daemon/../sessions/spawn",
            "/api/v1/daemon/%2e%2e/sessions/spawn",
            "/api/v1/daemon//x",
            "/metrics",
        ] {
            assert!(!is_worker_route(Some("POST"), path), "{path}");
            assert!(!is_worker_route(Some("GET"), path), "{path}");
        }
    }

    #[tokio::test]
    async fn the_placeholder_stays_a_placeholder_off_the_worker_routes() {
        for line in [
            "POST /api/v1/sessions/dispatch",
            "POST /api/v1/sessions/spawn",
            "GET /api/v1/sessions/other-session/messages",
            "GET /api/v1/daemon/../sessions/spawn",
        ] {
            let (mut worker, mut server) = wired();
            worker
                .write_all(
                    format!(
                        "{line} HTTP/1.1\r\nHost: cctui.cctui.svc:8700\r\n\
                         Authorization: Bearer {PLACEHOLDER_MACHINE_KEY}\r\n\
                         Content-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let (head, _) = read_message(&mut server, false).await;
            assert!(!head.contains(KEY), "{line}: {head}");
            assert!(head.contains(&format!("Bearer {PLACEHOLDER_MACHINE_KEY}")), "{line}: {head}");
        }
    }

    #[tokio::test]
    async fn another_credential_to_the_server_passes_untouched() {
        let (mut worker, mut server) = wired();
        worker
            .write_all(
                b"POST /gateway/anthropic/v1/messages HTTP/1.1\r\nHost: cctui.cctui.svc\r\n\
                  Authorization: Bearer session-gateway-token\r\nContent-Length: 2\r\n\r\n{}",
            )
            .await
            .unwrap();
        let (head, body) = read_message(&mut server, false).await;
        assert!(head.contains("Authorization: Bearer session-gateway-token"), "{head}");
        assert!(!head.contains(KEY));
        assert_eq!(body, b"{}");
    }

    #[tokio::test]
    async fn daemon_auth_swaps_the_body_key_and_masks_the_echo() {
        let (mut worker, mut server) = wired();
        let body = format!(r#"{{"machine_key":"{PLACEHOLDER_MACHINE_KEY}"}}"#);
        worker
            .write_all(
                format!(
                    "POST /api/v1/daemon/auth HTTP/1.1\r\nHost: cctui.cctui.svc\r\n\
                     Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (head, sent) = read_message(&mut server, false).await;
        let sent: serde_json::Value = serde_json::from_slice(&sent).unwrap();
        assert_eq!(sent["machine_key"], KEY);
        let sent_len = serde_json::to_vec(&sent).unwrap().len();
        assert!(head.contains(&format!("Content-Length: {sent_len}")), "{head}");

        let answer = format!(r#"{{"session_token":"{KEY}","machine_id":"m"}}"#);
        server
            .write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{answer}", answer.len())
                    .as_bytes(),
            )
            .await
            .unwrap();
        let (resp, got) = read_message(&mut worker, true).await;
        let got_text = String::from_utf8(got.clone()).unwrap();
        assert!(!got_text.contains(KEY), "the real key reached the worker: {got_text}");
        let got: serde_json::Value = serde_json::from_slice(&got).unwrap();
        assert_eq!(got["session_token"], PLACEHOLDER_MACHINE_KEY);
        assert!(resp.contains(&format!("Content-Length: {}", got_text.len())), "{resp}");
    }

    #[tokio::test]
    async fn a_websocket_upgrade_switches_to_a_raw_splice() {
        let (mut worker, mut server) = wired();
        worker
            .write_all(
                format!(
                    "GET /api/v1/daemon/ws HTTP/1.1\r\nHost: cctui.cctui.svc\r\n\
                     Upgrade: websocket\r\nConnection: Upgrade\r\n\
                     Authorization: Bearer {PLACEHOLDER_MACHINE_KEY}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (head, _) = read_message(&mut server, false).await;
        assert!(head.contains(&format!("Authorization: Bearer {KEY}")), "{head}");
        server
            .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\nframe-1")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("frame-1") {
            let n = worker.read(&mut buf).await.unwrap();
            assert!(n > 0, "closed before the first frame");
            seen.extend_from_slice(&buf[..n]);
        }
        worker.write_all(b"frame-2").await.unwrap();
        let mut got = [0u8; 7];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"frame-2");
    }

    #[test]
    fn masking_replaces_every_copy_of_the_key() {
        let masked = mask_key(format!("a{KEY}b{KEY}").as_bytes(), KEY);
        let want = format!("a{PLACEHOLDER_MACHINE_KEY}b{PLACEHOLDER_MACHINE_KEY}");
        assert_eq!(masked, want.as_bytes());
    }

    #[test]
    fn an_https_server_needs_the_pod_ca() {
        let target = ServerTarget::parse("https://cctui.example.test").unwrap();
        assert!(ServerInjector::new(target.clone(), KEY.to_owned(), None).is_err());
        let ca = Arc::new(PerPodCa::generate().unwrap());
        let inj = ServerInjector::new(target, KEY.to_owned(), Some(ca)).unwrap();
        assert!(inj.matches("cctui.example.test", 443));
        assert!(!inj.matches("other.example.test", 443));
    }

    // ---- Upstream keep-alive loss --------------------------------------------

    fn patch(body: &str) -> String {
        format!(
            "PATCH /api/v1/daemon/sessions/s-1 HTTP/1.1\r\nHost: cctui.cctui.svc:8700\r\n\
             Authorization: Bearer {PLACEHOLDER_MACHINE_KEY}\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    async fn answer(server: &mut DuplexStream, body: &str) {
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        server.write_all(resp.as_bytes()).await.unwrap();
    }

    async fn worker_exchange(worker: &mut DuplexStream, request: &str) -> (String, Vec<u8>) {
        worker.write_all(request.as_bytes()).await.unwrap();
        read_message(worker, true).await
    }

    #[tokio::test]
    async fn a_patch_after_the_server_closed_an_idle_keepalive_redials() {
        let (mut worker, mut servers, dialer) = wired_n(2);
        let mut first = servers.pop_front().unwrap();
        worker.write_all(patch("one").as_bytes()).await.unwrap();
        let (head, body) = read_message(&mut first, false).await;
        assert!(head.contains(&format!("Bearer {KEY}")), "{head}");
        assert_eq!(body, b"one");
        answer(&mut first, "ok-one").await;
        let (resp, body) = read_message(&mut worker, true).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert_eq!(body, b"ok-one");

        drop(first);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut second = servers.pop_front().unwrap();
        worker.write_all(patch("two").as_bytes()).await.unwrap();
        let (head, body) = read_message(&mut second, false).await;
        assert!(head.starts_with("PATCH "), "{head}");
        assert!(head.contains(&format!("Bearer {KEY}")), "{head}");
        assert_eq!(body, b"two");
        answer(&mut second, "ok-two").await;
        let (resp, body) = read_message(&mut worker, true).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "a bare EOF reached the worker: {resp}");
        assert_eq!(body, b"ok-two");
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_reused_connection_dropping_a_post_before_answering_is_replayed_once() {
        let (mut worker, mut servers, dialer) = wired_n(2);
        let mut first = servers.pop_front().unwrap();
        worker.write_all(patch("one").as_bytes()).await.unwrap();
        read_message(&mut first, false).await;
        answer(&mut first, "ok-one").await;
        read_message(&mut worker, true).await;

        let post = "POST /api/v1/daemon/sessions/s-1/messages HTTP/1.1\r\n\
                    Host: cctui.cctui.svc:8700\r\nContent-Length: 4\r\n\r\ncard";
        worker.write_all(post.as_bytes()).await.unwrap();
        let (_, body) = read_message(&mut first, false).await;
        assert_eq!(body, b"card");
        drop(first);

        let mut second = servers.pop_front().unwrap();
        let (head, body) = read_message(&mut second, false).await;
        assert!(head.starts_with("POST /api/v1/daemon/sessions/s-1/messages"), "{head}");
        assert_eq!(body, b"card", "the replay must carry the same body");
        answer(&mut second, "ok-card").await;
        let (resp, body) = read_message(&mut worker, true).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert_eq!(body, b"ok-card");
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 2);

        let request = patch("three");
        worker.write_all(request.as_bytes()).await.unwrap();
        let (_, body) = read_message(&mut second, false).await;
        assert_eq!(body, b"three", "the replayed connection is kept alive");
    }

    #[tokio::test]
    async fn a_replay_that_also_goes_unanswered_gets_502_with_connection_close() {
        let (mut worker, mut servers, dialer) = wired_n(2);
        let mut first = servers.pop_front().unwrap();
        worker.write_all(patch("one").as_bytes()).await.unwrap();
        read_message(&mut first, false).await;
        answer(&mut first, "ok-one").await;
        read_message(&mut worker, true).await;

        worker.write_all(patch("two").as_bytes()).await.unwrap();
        read_message(&mut first, false).await;
        drop(first);
        let mut second = servers.pop_front().unwrap();
        let (_, body) = read_message(&mut second, false).await;
        assert_eq!(body, b"two");
        drop(second);

        let (resp, _) = read_message(&mut worker, true).await;
        assert_eq!(response_status(resp.as_bytes()), Some(502), "{resp}");
        assert!(wants_close(resp.as_bytes()), "{resp}");
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 2, "replayed exactly once");
        let mut rest = Vec::new();
        worker.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "the worker connection is closed after the 502");
    }

    #[tokio::test]
    async fn a_post_too_large_to_replay_gets_502_without_a_redial() {
        let (mut worker, mut servers, dialer) = wired_n(2);
        let mut first = servers.pop_front().unwrap();
        worker.write_all(patch("one").as_bytes()).await.unwrap();
        read_message(&mut first, false).await;
        answer(&mut first, "ok-one").await;
        read_message(&mut worker, true).await;

        let big = patch(&"a".repeat(MAX_REPLAY_BYTES + 1));
        let server = tokio::spawn(async move {
            read_message(&mut first, false).await;
            drop(first);
        });
        let (resp, _) = worker_exchange(&mut worker, &big).await;
        server.await.unwrap();
        assert_eq!(response_status(resp.as_bytes()), Some(502), "{resp}");
        assert!(wants_close(resp.as_bytes()), "{resp}");
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_fresh_connection_closing_before_answering_gets_502() {
        let (mut worker, mut servers, dialer) = wired_n(1);
        let mut first = servers.pop_front().unwrap();
        worker.write_all(patch("one").as_bytes()).await.unwrap();
        read_message(&mut first, false).await;
        drop(first);
        let (resp, _) = read_message(&mut worker, true).await;
        assert_eq!(response_status(resp.as_bytes()), Some(502), "{resp}");
        assert!(wants_close(resp.as_bytes()), "{resp}");
        assert_eq!(dialer.dials.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_unreachable_server_gets_502() {
        let (mut worker, _, _) = wired_n(0);
        let (resp, _) = worker_exchange(&mut worker, &patch("one")).await;
        assert_eq!(response_status(resp.as_bytes()), Some(502), "{resp}");
    }

    /// Plain-HTTP server whose connection `i` answers `answers(i)` requests
    /// with `conn<i>|<body>` and then closes.
    async fn spawn_closing_server(answers: fn(usize) -> usize) -> (u16, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conns = Arc::new(AtomicUsize::new(0));
        let cc = conns.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let idx = cc.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = HttpBuf::default();
                    for _ in 0..answers(idx) {
                        let Ok(Some(req)) = buf.read_head(&mut sock).await else { return };
                        let mut body = format!("conn{idx}|").into_bytes();
                        let framing = body_framing(&req, false);
                        if Box::pin(buf.relay_body(&mut sock, &mut body, framing)).await.is_err() {
                            return;
                        }
                        let head =
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                        if sock.write_all(head.as_bytes()).await.is_err()
                            || sock.write_all(&body).await.is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        (port, conns)
    }

    #[tokio::test]
    async fn handle_redials_a_plain_server_that_closed_an_idle_keepalive() {
        let (port, conns) = spawn_closing_server(|i| if i == 0 { 1 } else { usize::MAX }).await;
        let target = ServerTarget::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let inj = Arc::new(ServerInjector::new(target, KEY.to_owned(), None).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            inj.handle(sock, Vec::new()).await.unwrap();
        });
        let mut worker = TcpStream::connect(addr).await.unwrap();
        let mut buf = HttpBuf::default();
        for (i, body) in ["one", "two"].into_iter().enumerate() {
            worker.write_all(patch(body).as_bytes()).await.unwrap();
            let resp = buf.read_head(&mut worker).await.unwrap().expect("never a bare EOF");
            assert_eq!(response_status(&resp), Some(200));
            let mut got = Vec::new();
            let framing = response_framing(&resp, Some("PATCH"));
            Box::pin(buf.relay_body(&mut worker, &mut got, framing)).await.unwrap();
            assert_eq!(String::from_utf8(got).unwrap(), format!("conn{i}|{body}"));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(conns.load(Ordering::SeqCst), 2);
    }
}
