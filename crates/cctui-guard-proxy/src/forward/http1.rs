//! Relays exactly one HTTP/1 request per upstream connection, bound to the host
//! the connection was allowed for, so a kept-alive connection cannot carry a
//! later request to another virtual host on the same address.

use std::fmt;

use anyhow::{Context as _, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_HEAD: usize = 64 * 1024;
const MAX_LINE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Length(u64),
    Chunked,
}

#[derive(Debug)]
pub struct Prepared {
    pub head: Vec<u8>,
    pub framing: Framing,
    pub body_start: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    Malformed(&'static str),
    WrongHost(String),
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed request: {why}"),
            Self::WrongHost(h) => write!(f, "Host {h:?} is not the allowed host"),
        }
    }
}

#[must_use]
pub fn head_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

pub async fn read_head<R: AsyncRead + Unpin>(
    r: &mut R,
    mut buf: Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    let mut chunk = [0u8; 4096];
    while head_end(&buf).is_none() {
        ensure!(buf.len() <= MAX_HEAD, "request header too large");
        let n = r.read(&mut chunk).await?;
        ensure!(n > 0, "client closed mid-header");
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

#[must_use]
pub fn is_http1_request_line(line: &str) -> bool {
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    !method.is_empty()
        && method.bytes().all(is_tchar)
        && !target.is_empty()
        && matches!(version, "HTTP/1.1" | "HTTP/1.0")
}

#[must_use]
pub const fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[must_use]
pub fn same_host(a: &str, b: &str) -> bool {
    let norm = |h: &str| {
        h.trim_matches(|c| c == '[' || c == ']').trim_end_matches('.').to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

#[must_use]
pub fn split_host_header(value: &str) -> (&str, Option<&str>) {
    if let Some(rest) = value.strip_prefix('[') {
        return rest.split_once(']').map_or((value, None), |(h, tail)| (h, tail.strip_prefix(':')));
    }
    value.rsplit_once(':').map_or((value, None), |(h, p)| (h, Some(p)))
}

/// Parses the head at the start of `raw`, checks `Host` against the allowed
/// `host` (and `port`, when given), and rewrites the head so the upstream closes
/// after this request. A missing `Host` is filled in with the allowed one.
pub fn prepare(raw: &[u8], host: &str, port: Option<u16>) -> Result<Prepared, Reject> {
    let end = head_end(raw).ok_or(Reject::Malformed("incomplete head"))?;
    let text =
        std::str::from_utf8(&raw[..end - 4]).map_err(|_| Reject::Malformed("non-UTF-8 head"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    if !is_http1_request_line(request_line) {
        return Err(Reject::Malformed("not an HTTP/1 request line"));
    }
    let mut head = String::with_capacity(end + 64);
    head.push_str(request_line);
    head.push_str("\r\n");
    let mut seen_host = false;
    let mut length: Option<u64> = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) =
            line.split_once(':').ok_or(Reject::Malformed("header without a colon"))?;
        if name.is_empty() || !name.bytes().all(is_tchar) || line.contains(['\r', '\n']) {
            return Err(Reject::Malformed("bad header line"));
        }
        let value = value.trim_matches([' ', '\t']);
        match name.to_ascii_lowercase().as_str() {
            "host" => {
                if seen_host {
                    return Err(Reject::Malformed("duplicate Host"));
                }
                seen_host = true;
                let (h, p) = split_host_header(value);
                let port_ok = match (p, port) {
                    (Some(p), Some(want)) => p.parse::<u16>().is_ok_and(|p| p == want),
                    _ => true,
                };
                if !same_host(h, host) || !port_ok {
                    return Err(Reject::WrongHost(value.to_owned()));
                }
            }
            "content-length" => {
                for v in value.split(',').map(str::trim) {
                    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(Reject::Malformed("bad Content-Length"));
                    }
                    let n = v.parse().map_err(|_| Reject::Malformed("bad Content-Length"))?;
                    if length.is_some_and(|l| l != n) {
                        return Err(Reject::Malformed("conflicting Content-Length"));
                    }
                    length = Some(n);
                }
            }
            "transfer-encoding" => {
                if chunked || !value.eq_ignore_ascii_case("chunked") {
                    return Err(Reject::Malformed("unsupported Transfer-Encoding"));
                }
                chunked = true;
            }
            "connection" | "proxy-connection" | "keep-alive" | "upgrade" => continue,
            _ => {}
        }
        head.push_str(line);
        head.push_str("\r\n");
    }
    if chunked && length.is_some() {
        return Err(Reject::Malformed("both Content-Length and Transfer-Encoding"));
    }
    if !seen_host {
        head.push_str("Host: ");
        head.push_str(host);
        if let Some(p) = port {
            head.push(':');
            head.push_str(&p.to_string());
        }
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    let framing = if chunked { Framing::Chunked } else { Framing::Length(length.unwrap_or(0)) };
    Ok(Prepared { head: head.into_bytes(), framing, body_start: end })
}

/// Sends `req` and its body upstream and streams the response back. Any byte
/// the client sends after the body ends the connection: it would be a second
/// request that was never checked.
pub async fn relay_one(
    client: &mut TcpStream,
    upstream: &mut TcpStream,
    req: &Prepared,
    raw: &[u8],
) -> anyhow::Result<()> {
    let (mut cr, mut cw) = client.split();
    let (mut ur, mut uw) = upstream.split();
    let upload = async {
        uw.write_all(&req.head).await?;
        let extra = send_body(&mut cr, &mut uw, req.framing, &raw[req.body_start..]).await?;
        ensure!(extra.is_empty(), "client pipelined a second request");
        let mut probe = [0u8; 1];
        ensure!(cr.read(&mut probe).await? == 0, "client sent a second request");
        anyhow::Ok(())
    };
    let download = tokio::io::copy(&mut ur, &mut cw);
    tokio::pin!(upload, download);
    tokio::select! {
        r = &mut upload => {
            r?;
            download.await?;
        }
        r = &mut download => {
            r?;
        }
    }
    Ok(())
}

async fn send_body<R, W>(
    r: &mut R,
    w: &mut W,
    framing: Framing,
    pending: &[u8],
) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    match framing {
        Framing::Length(n) => {
            let take = usize::try_from(n).unwrap_or(usize::MAX).min(pending.len());
            w.write_all(&pending[..take]).await?;
            let remaining = n - take as u64;
            if remaining > 0 {
                let copied = tokio::io::copy(&mut r.take(remaining), w).await?;
                ensure!(copied == remaining, "client closed mid-body");
            }
            Ok(pending[take..].to_vec())
        }
        Framing::Chunked => {
            let mut decoder = Chunked::default();
            let mut buf = pending.to_vec();
            loop {
                if let Some(end) = decoder.feed(&buf)? {
                    w.write_all(&buf[..end]).await?;
                    return Ok(buf.split_off(end));
                }
                w.write_all(&buf).await?;
                buf.resize(8192, 0);
                let n = r.read(&mut buf).await?;
                ensure!(n > 0, "client closed mid-body");
                buf.truncate(n);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Size,
    Data(u64),
    DataCr,
    DataLf,
    Trailer,
}

#[derive(Debug, Default)]
struct Chunked {
    state: State,
    line: Vec<u8>,
}

impl Chunked {
    fn feed(&mut self, data: &[u8]) -> anyhow::Result<Option<usize>> {
        let mut i = 0;
        while i < data.len() {
            match self.state {
                State::Data(rem) => {
                    let take = rem.min((data.len() - i) as u64);
                    i += usize::try_from(take)?;
                    self.state = if take == rem { State::DataCr } else { State::Data(rem - take) };
                }
                State::DataCr | State::DataLf => {
                    let (want, next) = if self.state == State::DataCr {
                        (b'\r', State::DataLf)
                    } else {
                        (b'\n', State::Size)
                    };
                    ensure!(data[i] == want, "bad chunk terminator");
                    i += 1;
                    self.state = next;
                }
                State::Size | State::Trailer => {
                    let b = data[i];
                    i += 1;
                    if b != b'\n' {
                        self.line.push(b);
                        ensure!(self.line.len() <= MAX_LINE, "chunk line too long");
                        continue;
                    }
                    let line = std::mem::take(&mut self.line);
                    let line = line.strip_suffix(b"\r").context("bare LF in chunked body")?;
                    if self.state == State::Trailer {
                        if line.is_empty() {
                            return Ok(Some(i));
                        }
                        continue;
                    }
                    let size = chunk_size(line)?;
                    self.state = if size == 0 { State::Trailer } else { State::Data(size) };
                }
            }
        }
        Ok(None)
    }
}

fn chunk_size(line: &[u8]) -> anyhow::Result<u64> {
    let digits = line.split(|&b| b == b';').next().unwrap_or_default();
    let digits = std::str::from_utf8(digits)?.trim_matches([' ', '\t']);
    ensure!(
        !digits.is_empty() && digits.len() <= 16 && digits.bytes().all(|b| b.is_ascii_hexdigit()),
        "bad chunk size"
    );
    Ok(u64::from_str_radix(digits, 16)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prep(raw: &str, host: &str, port: Option<u16>) -> Result<Prepared, Reject> {
        prepare(raw.as_bytes(), host, port)
    }

    #[test]
    fn rewrites_to_a_single_closing_request() {
        let raw = "GET http://a.example/x HTTP/1.1\r\nHost: a.example\r\nConnection: keep-alive\r\n\
                   Keep-Alive: timeout=5\r\nProxy-Connection: keep-alive\r\nAccept: */*\r\n\r\n";
        let p = prep(raw, "a.example", Some(80)).unwrap();
        let head = String::from_utf8(p.head).unwrap();
        assert_eq!(
            head,
            "GET http://a.example/x HTTP/1.1\r\nHost: a.example\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        assert_eq!(p.framing, Framing::Length(0));
        assert_eq!(p.body_start, raw.len());
    }

    #[test]
    fn host_must_be_the_allowed_host() {
        let raw = "GET http://a.example/ HTTP/1.1\r\nHost: evil.example\r\n\r\n";
        assert_eq!(
            prep(raw, "a.example", Some(80)).unwrap_err(),
            Reject::WrongHost("evil.example".to_owned())
        );
        let raw = "GET / HTTP/1.1\r\nHost: A.Example.:8080\r\n\r\n";
        assert!(prep(raw, "a.example", Some(8080)).is_ok());
        assert!(matches!(prep(raw, "a.example", Some(80)), Err(Reject::WrongHost(_))));
        assert!(prep(raw, "a.example", None).is_ok());
        let raw = "GET / HTTP/1.1\r\nHost: [::1]:80\r\n\r\n";
        assert!(prep(raw, "::1", Some(80)).is_ok());
    }

    #[test]
    fn missing_host_is_filled_in() {
        let p = prep("GET / HTTP/1.1\r\n\r\n", "a.example", Some(8080)).unwrap();
        let head = String::from_utf8(p.head).unwrap();
        assert!(head.contains("\r\nHost: a.example:8080\r\n"), "{head}");
    }

    #[test]
    fn ambiguous_requests_are_rejected() {
        for raw in [
            "GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: a\r\nContent-Length: +3\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: a\r\n X-Folded: y\r\n\r\n",
            "GET / HTTP/1.1\r\nHost : a\r\n\r\n",
            "PRI * HTTP/2.0\r\n\r\n",
        ] {
            assert!(matches!(prep(raw, "a", None), Err(Reject::Malformed(_))), "{raw:?}");
        }
    }

    #[test]
    fn framing_follows_the_headers() {
        let p = prep("POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nhello", "a", None)
            .unwrap();
        assert_eq!(p.framing, Framing::Length(5));
        let p = prep("POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: Chunked\r\n\r\n", "a", None)
            .unwrap();
        assert_eq!(p.framing, Framing::Chunked);
    }

    #[test]
    fn chunked_decoder_finds_the_end() {
        let body = b"5;ext=1\r\nhello\r\n0\r\nX-Trailer: y\r\n\r\nGET /next";
        assert_eq!(Chunked::default().feed(body).unwrap(), Some(body.len() - 9));
        let mut d = Chunked::default();
        let mut end = None;
        for (i, b) in body.iter().enumerate() {
            if let Some(e) = d.feed(std::slice::from_ref(b)).unwrap() {
                end = Some(i + e);
                break;
            }
        }
        assert_eq!(end, Some(body.len() - 9), "byte-at-a-time feed agrees");
        assert!(Chunked::default().feed(b"zz\r\n").is_err());
        assert!(Chunked::default().feed(b"3\r\nabcX").is_err());
        assert!(Chunked::default().feed(b"3\nabc").is_err());
    }

    #[tokio::test]
    async fn send_body_returns_what_follows() {
        let mut out = Vec::new();
        let mut rest: &[u8] = b"lo";
        let extra = send_body(&mut rest, &mut out, Framing::Length(5), b"hel").await.unwrap();
        assert_eq!((out.as_slice(), extra.as_slice()), (b"hello".as_slice(), b"".as_slice()));
        let mut out = Vec::new();
        let mut rest: &[u8] = b"";
        let extra = send_body(&mut rest, &mut out, Framing::Length(2), b"hiGET").await.unwrap();
        assert_eq!((out.as_slice(), extra.as_slice()), (b"hi".as_slice(), b"GET".as_slice()));
        let mut out = Vec::new();
        let mut rest: &[u8] = b"\r\n0\r\n\r\nGET";
        let extra = send_body(&mut rest, &mut out, Framing::Chunked, b"2\r\nhi").await.unwrap();
        assert_eq!(out, b"2\r\nhi\r\n0\r\n\r\n");
        assert_eq!(extra, b"GET");
    }
}
