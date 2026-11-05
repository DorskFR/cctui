use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Http,
    Tcp,
}

/// One connection that reached the sink.
///
/// Its mere existence is the finding: the sink sits where no sanctioned
/// destination is, so any hit is egress or lateral reach. `token` carries any
/// canary echoed by the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkHit {
    pub channel: Channel,
    pub peer: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub detail: String,
}

impl SinkHit {
    #[must_use]
    pub fn tcp(peer: &str, token: &str) -> Self {
        Self {
            channel: Channel::Tcp,
            peer: peer.to_owned(),
            token: token.to_owned(),
            detail: String::new(),
        }
    }
}

type Hits = Arc<Mutex<Vec<SinkHit>>>;

fn record(hits: &Hits, hit: SinkHit) {
    hits.lock().unwrap_or_else(PoisonError::into_inner).push(hit);
}

fn snapshot(hits: &Hits) -> Vec<SinkHit> {
    hits.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Pull a canary out of an HTTP request line or headers.
///
/// The probes echo the token they were handed in the path (`/canary/<tok>`) or
/// an `X-Canary` header, so a hit ties back to the credential it proves use of.
fn extract_token(request: &str) -> String {
    for line in request.lines() {
        if let Some(rest) = line.strip_prefix("GET /canary/") {
            return rest.split_whitespace().next().unwrap_or_default().to_owned();
        }
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("x-canary:") {
            return rest.trim().to_owned();
        }
    }
    String::new()
}

fn request_line(request: &str) -> &str {
    request.lines().next().unwrap_or_default()
}

async fn handle_http(mut conn: TcpStream, peer: String, hits: Hits) -> std::io::Result<()> {
    let mut buf = vec![0u8; 8192];
    let n = conn.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let first = request_line(&request);
    let (body, status) = if first.starts_with("GET /hits") {
        (serde_json::to_string(&snapshot(&hits)).unwrap_or_else(|_| "[]".to_owned()), "200 OK")
    } else if first.starts_with("GET /ready") {
        ("ready".to_owned(), "200 OK")
    } else {
        record(
            &hits,
            SinkHit {
                channel: Channel::Http,
                peer,
                token: extract_token(&request),
                detail: first.to_owned(),
            },
        );
        ("sink".to_owned(), "200 OK")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    conn.write_all(response.as_bytes()).await?;
    conn.shutdown().await
}

async fn handle_raw(mut conn: TcpStream, peer: String, hits: Hits) {
    let mut buf = vec![0u8; 512];
    let n = conn.read(&mut buf).await.unwrap_or(0);
    let token = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
    record(&hits, SinkHit { channel: Channel::Tcp, peer, token, detail: String::new() });
    let _ = conn.shutdown().await;
}

async fn serve(listener: TcpListener, hits: Hits, http: bool) {
    loop {
        let Ok((conn, addr)) = listener.accept().await else { continue };
        let hits = Arc::clone(&hits);
        let peer = addr.to_string();
        tokio::spawn(async move {
            if http {
                let _ = handle_http(conn, peer, hits).await;
            } else {
                handle_raw(conn, peer, hits).await;
            }
        });
    }
}

/// Run the sink: an HTTP listener (also answering `/hits` and `/ready`) and a
/// raw-TCP listener, sharing one hit log. Never returns.
pub async fn run(http_addr: &str, tcp_addr: &str) -> anyhow::Result<()> {
    let hits: Hits = Arc::new(Mutex::new(Vec::new()));
    let http = TcpListener::bind(http_addr).await?;
    let tcp = TcpListener::bind(tcp_addr).await?;
    let http_task = tokio::spawn(serve(http, Arc::clone(&hits), true));
    let tcp_task = tokio::spawn(serve(tcp, hits, false));
    let _ = tokio::try_join!(http_task, tcp_task)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hits_request_is_not_itself_recorded_as_a_hit() {
        assert_eq!(request_line("GET /hits HTTP/1.1\r\nHost: x\r\n"), "GET /hits HTTP/1.1");
    }

    #[test]
    fn a_canary_in_the_path_is_recovered() {
        assert_eq!(extract_token("GET /canary/cnry-123 HTTP/1.1\r\n"), "cnry-123");
    }

    #[test]
    fn a_canary_header_is_recovered_case_insensitively() {
        assert_eq!(extract_token("GET / HTTP/1.1\r\nX-Canary: cnry-abc\r\n"), "cnry-abc");
    }

    #[test]
    fn no_canary_yields_an_empty_token() {
        assert!(extract_token("GET / HTTP/1.1\r\nHost: sink\r\n").is_empty());
    }

    #[tokio::test]
    async fn an_http_connection_is_logged_and_a_raw_tcp_connection_is_logged() {
        let hits: Hits = Arc::new(Mutex::new(Vec::new()));
        record(
            &hits,
            SinkHit {
                channel: Channel::Http,
                peer: "1.2.3.4:5".to_owned(),
                token: "t".to_owned(),
                detail: "GET /x".to_owned(),
            },
        );
        record(&hits, SinkHit::tcp("1.2.3.4:6", ""));
        let snap = snapshot(&hits);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].channel, Channel::Http);
        assert_eq!(snap[1].channel, Channel::Tcp);
    }
}
