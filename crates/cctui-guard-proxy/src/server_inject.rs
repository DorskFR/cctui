//! Machine-key injection for the cctui server host. The worker holds only the
//! placeholder; the proxy swaps it for the redeemed key on the way out and
//! masks the key wherever the server echoes it back.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::dispatch::ServerTarget;
use crate::inject::{
    AuthShape, Framing, HttpBuf, Injector, PerPodCa, PrefixedStream, SniResolver, body_framing,
    header_value, request_method, request_path, response_framing, response_status, rewrite_head,
    wants_close,
};

/// Must equal `cctui_proto::dispatch_identity::PLACEHOLDER_MACHINE_KEY`, which
/// the dispatcher hands the worker (checked by a test).
pub const PLACEHOLDER_MACHINE_KEY: &str = "cctui-dispatch-placeholder-key";
/// The daemon also presents its key in this request's JSON body, and the
/// server echoes it back as the session token.
const DAEMON_AUTH_PATH: &str = "/api/v1/daemon/auth";
const MAX_SWAP_BODY: usize = 64 * 1024;
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

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
        let upstream = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(addrs.as_slice()))
            .await
            .map_err(|_| anyhow::anyhow!("dialing the cctui server timed out"))??;
        let client = PrefixedStream::new(conn, prefix);
        let Some(acceptor) = &self.acceptor else {
            return self.pump(client, upstream).await;
        };
        let client = acceptor.accept(client).await?;
        let name = ServerName::try_from(self.target.host.clone())?;
        let upstream = self.connector.connect(name, upstream).await?;
        self.pump(client, upstream).await
    }

    fn outbound_head(&self, head: &[u8]) -> Vec<u8> {
        let placeholder = format!("Bearer {PLACEHOLDER_MACHINE_KEY}");
        if header_value(head, "authorization") == Some(placeholder.as_bytes()) {
            rewrite_head(head, &AuthShape::Bearer, &self.machine_key, None)
        } else {
            head.to_vec()
        }
    }

    pub async fn pump<C, U>(&self, mut client: C, mut upstream: U) -> anyhow::Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin,
        U: AsyncRead + AsyncWrite + Unpin,
    {
        let mut cbuf = HttpBuf::default();
        let mut ubuf = HttpBuf::default();
        loop {
            let Some(head) = cbuf.read_head(&mut client).await? else { return Ok(()) };
            let method = request_method(&head);
            let outbound = self.outbound_head(&head);
            let swapped = if request_path(&head).as_deref() == Some(DAEMON_AUTH_PATH) {
                self.send_auth_body(&mut cbuf, &mut client, &mut upstream, &outbound).await?
            } else {
                upstream.write_all(&outbound).await?;
                cbuf.relay_body(&mut client, &mut upstream, body_framing(&head, false)).await?;
                false
            };
            upstream.flush().await?;

            let Some(resp) = ubuf.read_head(&mut upstream).await? else { return Ok(()) };
            if response_status(&resp) == Some(101) {
                client.write_all(&resp).await?;
                client.write_all(&ubuf.take_buffered()).await?;
                upstream.write_all(&cbuf.take_buffered()).await?;
                client.flush().await?;
                upstream.flush().await?;
                tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
                return Ok(());
            }
            let framing = response_framing(&resp, method.as_deref());
            if swapped {
                let Framing::Length(len) = framing else {
                    anyhow::bail!("daemon auth answer has no bounded body; dropped, not leaked");
                };
                anyhow::ensure!(len <= MAX_SWAP_BODY, "daemon auth answer too large");
                let mut body = Vec::with_capacity(len);
                ubuf.relay_body(&mut upstream, &mut body, framing).await?;
                let body = mask_key(&body, &self.machine_key);
                client.write_all(&with_content_length(&resp, body.len())).await?;
                client.write_all(&body).await?;
            } else {
                client.write_all(&resp).await?;
                ubuf.relay_body(&mut upstream, &mut client, framing).await?;
            }
            client.flush().await?;
            if wants_close(&head) || wants_close(&resp) {
                return Ok(());
            }
        }
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
            cbuf.relay_body(client, upstream, framing).await?;
            return Ok(false);
        };
        anyhow::ensure!(len <= MAX_SWAP_BODY, "daemon auth body too large");
        let mut body = Vec::with_capacity(len);
        cbuf.relay_body(client, &mut body, framing).await?;
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
    use tokio::io::{AsyncReadExt, DuplexStream, duplex};

    use super::*;

    const KEY: &str = "cctui_m_real_machine_key";

    fn injector() -> Arc<ServerInjector> {
        let target = ServerTarget::parse("http://cctui.cctui.svc:8700").unwrap();
        Arc::new(ServerInjector::new(target, KEY.to_owned(), None).unwrap())
    }

    /// `(worker end, server end)` with the injector pumping between them.
    fn wired() -> (DuplexStream, DuplexStream) {
        let (worker, proxy_client) = duplex(64 * 1024);
        let (proxy_upstream, server) = duplex(64 * 1024);
        let inj = injector();
        tokio::spawn(async move { inj.pump(proxy_client, proxy_upstream).await });
        (worker, server)
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
            buf.relay_body(stream, &mut body, framing).await.unwrap();
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
}
