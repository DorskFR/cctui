//! Hand-off of `policy.json` to guard-proxy, confirmed through its `/ready`.

use std::io::{self, Read, Write as _};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const IO_TIMEOUT: Duration = Duration::from_millis(500);

/// The temp file must sit in the same directory for the rename to be atomic.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_owned();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A restarted guard must not reuse a generation the proxy already loaded.
#[must_use]
pub fn existing_generation(path: &Path) -> u64 {
    std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .and_then(|doc| doc.get("generation").and_then(Value::as_u64))
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// `None`: no policy loaded.
    Generation(Option<u64>),
    /// A proxy that predates the handshake.
    Unsupported,
}

pub fn probe(addr: &str) -> io::Result<Probe> {
    let target: SocketAddr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other(format!("{addr} resolves to nothing")))?;
    let mut stream = TcpStream::connect_timeout(&target, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let request = format!("GET /ready HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let raw = String::from_utf8_lossy(&raw);
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    Ok(parse_ready(body))
}

fn parse_ready(body: &str) -> Probe {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|doc| doc.get("generation").map(Value::as_u64))
        .map_or(Probe::Unsupported, Probe::Generation)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handshake {
    Confirmed,
    TimedOut { last_seen: Option<u64> },
    Unsupported,
    Unreachable(String),
}

/// A proxy that cannot take part (unreachable before any answer, or too old)
/// ends the wait at once.
#[must_use]
pub fn await_generation(addr: &str, generation: u64, timeout: Duration) -> Handshake {
    let deadline = Instant::now() + timeout;
    let mut last_seen = None;
    loop {
        match probe(addr) {
            Ok(Probe::Generation(Some(seen))) if seen == generation => {
                return Handshake::Confirmed;
            }
            Ok(Probe::Generation(seen)) => last_seen = seen,
            Ok(Probe::Unsupported) => return Handshake::Unsupported,
            Err(e) if last_seen.is_none() => return Handshake::Unreachable(e.to_string()),
            Err(_) => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Handshake::TimedOut { last_seen };
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_body_parsing_distinguishes_old_and_new_proxies() {
        assert_eq!(parse_ready(r#"{"ready":true,"generation":4}"#), Probe::Generation(Some(4)));
        assert_eq!(parse_ready(r#"{"ready":false,"generation":null}"#), Probe::Generation(None));
        assert_eq!(parse_ready("Ready"), Probe::Unsupported);
        assert_eq!(parse_ready(r#"{"ready":true}"#), Probe::Unsupported);
    }

    #[test]
    fn existing_generation_reads_the_stamp_or_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        assert_eq!(existing_generation(&path), 0);
        std::fs::write(&path, r#"{"allowed_hosts":[],"default":"deny"}"#).unwrap();
        assert_eq!(existing_generation(&path), 0);
        std::fs::write(&path, r#"{"allowed_hosts":[],"default":"deny","generation":9}"#).unwrap();
        assert_eq!(existing_generation(&path), 9);
    }

    #[test]
    fn atomic_write_never_exposes_a_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let small =
            serde_json::json!({ "allowed_hosts": ["a:443"], "default": "deny" }).to_string();
        let hosts: Vec<String> = (0..20_000).map(|i| format!("host-{i}.example.com:443")).collect();
        let large = serde_json::json!({ "allowed_hosts": hosts, "default": "deny" }).to_string();
        write_atomic(&path, small.as_bytes()).unwrap();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (path, stop) = (path.clone(), stop.clone());
            let (small, large) = (small.clone(), large.clone());
            std::thread::spawn(move || {
                let mut reads = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let seen = std::fs::read_to_string(&path).unwrap();
                    assert!(seen == small || seen == large, "partial read of {} bytes", seen.len());
                    reads += 1;
                }
                reads
            })
        };
        for i in 0..200 {
            let doc = if i % 2 == 0 { &large } else { &small };
            write_atomic(&path, doc.as_bytes()).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0);
        assert!(!dir.path().join("policy.json.tmp").exists());
    }
}
