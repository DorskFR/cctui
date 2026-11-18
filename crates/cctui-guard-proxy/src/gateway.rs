//! A guard pod's road to the cctui server: the daemon's WebSocket client
//! ignores proxy env, so the token rides as the first path segment instead.

use std::sync::Arc;

use tokio::net::TcpListener;

use crate::auth::constant_time_eq;
use crate::dispatch::ServerTarget;
use crate::server_inject::ServerInjector;

pub struct GatewayRewrite {
    token: String,
    host: String,
}

impl GatewayRewrite {
    pub fn new(token: &str, target: &ServerTarget) -> Self {
        let default_port = if target.https { 443 } else { 80 };
        let host = if target.port == default_port {
            target.host.clone()
        } else {
            format!("{}:{}", target.host, target.port)
        };
        Self { token: token.to_owned(), host }
    }

    pub fn rewrite(&self, head: &[u8]) -> Option<Vec<u8>> {
        let text = std::str::from_utf8(head).ok()?;
        let (line, headers) = text.split_once("\r\n")?;
        let mut parts = line.splitn(3, ' ');
        let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
        let rest = target.strip_prefix('/')?;
        let (segment, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (segment, query) =
            segment.split_once('?').map_or((segment, None), |(s, q)| (s, Some(q)));
        if !constant_time_eq(segment.as_bytes(), self.token.as_bytes()) {
            return None;
        }
        let mut out = String::with_capacity(head.len());
        out.push_str(method);
        out.push_str(" /");
        out.push_str(path);
        if let Some(query) = query {
            out.push('?');
            out.push_str(query);
        }
        out.push(' ');
        out.push_str(version);
        out.push_str("\r\n");
        for header in headers.split_inclusive("\r\n") {
            let name = header.split(':').next().unwrap_or_default().trim();
            if name.eq_ignore_ascii_case("host") {
                out.push_str("Host: ");
                out.push_str(&self.host);
                out.push_str("\r\n");
            } else {
                out.push_str(header);
            }
        }
        Some(out.into_bytes())
    }
}

pub async fn serve(addr: &str, injector: Arc<ServerInjector>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("cctui server gateway listening on {addr}");
    loop {
        let (stream, _) = listener.accept().await?;
        let injector = injector.clone();
        tokio::spawn(async move {
            if let Err(e) = injector.handle_gateway(stream).await {
                tracing::debug!("gateway connection ended: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn gateway(url: &str) -> GatewayRewrite {
        GatewayRewrite::new(TOKEN, &ServerTarget::parse(url).unwrap())
    }

    fn rewrite(g: &GatewayRewrite, line: &str) -> Option<String> {
        let head = format!("{line}\r\nHost: guard:15003\r\nAuthorization: Bearer k\r\n\r\n");
        g.rewrite(head.as_bytes()).map(|h| String::from_utf8(h).unwrap())
    }

    #[test]
    fn the_token_segment_is_stripped_and_the_host_is_the_servers() {
        let g = gateway("http://cctui.cctui.svc:8700");
        let out = rewrite(&g, &format!("GET /{TOKEN}/api/v1/daemon/ws HTTP/1.1")).unwrap();
        assert_eq!(
            out,
            "GET /api/v1/daemon/ws HTTP/1.1\r\nHost: cctui.cctui.svc:8700\r\n\
             Authorization: Bearer k\r\n\r\n"
        );
        let out = rewrite(&g, &format!("GET /{TOKEN}?x=1 HTTP/1.1")).unwrap();
        assert!(out.starts_with("GET /?x=1 HTTP/1.1\r\n"), "{out}");
        let tls = gateway("https://cctui.example.test");
        let out = rewrite(&tls, &format!("GET /{TOKEN}/health HTTP/1.1")).unwrap();
        assert!(out.contains("\r\nHost: cctui.example.test\r\n"), "{out}");
    }

    #[test]
    fn a_request_without_the_token_is_refused() {
        let g = gateway("http://cctui:8700");
        assert!(rewrite(&g, "GET /api/v1/daemon/ws HTTP/1.1").is_none());
        assert!(rewrite(&g, "GET /0123456789abcdef0123456789abcdee/api HTTP/1.1").is_none());
        assert!(rewrite(&g, &format!("GET /x{TOKEN}/api HTTP/1.1")).is_none());
        assert!(rewrite(&g, &format!("GET http://cctui:8700/{TOKEN}/api HTTP/1.1")).is_none());
        assert!(rewrite(&g, "GET / HTTP/1.1").is_none());
    }
}
