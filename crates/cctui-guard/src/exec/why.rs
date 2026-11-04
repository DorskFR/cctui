//! The last few exec denials, for `cctui-guard why`: the fallback when the
//! denied process's stderr went nowhere or the message landed on a wrapper
//! shell.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, Read, Write as _};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const WHY_CAPACITY: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Denial {
    pub pid: u32,
    /// The denied argv as shown to humans, or the path when it could not be read.
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct WhyRing {
    inner: Mutex<VecDeque<Denial>>,
}

impl WhyRing {
    pub fn push(&self, denial: Denial) {
        let mut ring = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if ring.len() == WHY_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(denial);
    }

    /// Newest first.
    #[must_use]
    pub fn recent(&self) -> Vec<Denial> {
        let ring = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ring.iter().rev().cloned().collect()
    }
}

/// Plain-text rendering of [`WhyRing::recent`].
#[must_use]
pub fn render(recent: &[Denial]) -> String {
    if recent.is_empty() {
        return "no exec denials recorded\n".to_string();
    }
    let mut out = String::new();
    for d in recent {
        let _ = writeln!(out, "denied `{}` (pid {}) — {}", d.command, d.pid, d.reason);
    }
    out
}

/// `GET /why` from the guard at `addr` (`host:port`), returning the body.
pub fn fetch(addr: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write!(stream, "GET /why HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n")?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) =
        raw.split_once("\r\n\r\n").ok_or_else(|| io::Error::other("malformed guard response"))?;
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(io::Error::other(format!("guard answered `{status}`")));
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    fn denial(n: u32) -> Denial {
        Denial { pid: n, command: format!("git push {n}"), reason: format!("reason {n}") }
    }

    #[test]
    fn keeps_the_last_five_newest_first() {
        let ring = WhyRing::default();
        for n in 1..=7 {
            ring.push(denial(n));
        }
        let pids: Vec<u32> = ring.recent().iter().map(|d| d.pid).collect();
        assert_eq!(pids, [7, 6, 5, 4, 3]);
    }

    #[test]
    fn renders_each_denial_or_an_empty_note() {
        assert_eq!(render(&[]), "no exec denials recorded\n");
        let text = render(&[denial(2), denial(1)]);
        assert_eq!(
            text,
            "denied `git push 2` (pid 2) — reason 2\ndenied `git push 1` (pid 1) — reason 1\n"
        );
    }

    fn fake_guard(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap() == 1 {
                seen.push(byte[0]);
            }
            assert!(seen.starts_with(b"GET /why HTTP/1.0\r\n"));
            conn.write_all(response.as_bytes()).unwrap();
        });
        addr
    }

    #[test]
    fn fetch_returns_the_body() {
        let addr = fake_guard("HTTP/1.0 200 OK\r\ncontent-type: text/plain\r\n\r\ndenied `x`\n");
        assert_eq!(fetch(&addr).unwrap(), "denied `x`\n");
    }

    #[test]
    fn fetch_surfaces_a_non_200() {
        let addr = fake_guard("HTTP/1.0 404 Not Found\r\n\r\n");
        let err = fetch(&addr).unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }
}
