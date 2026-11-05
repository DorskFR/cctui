//! Authoritative DNS sink for one zone, over UDP.
//!
//! Answers queries under the zone, refuses the rest and never recurses. Every
//! well-formed query is recorded: a resolver reaching it means DNS carried data
//! out, and a canary label names the run that leaked it.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, UdpSocket};

use crate::canary;
use crate::sink::{Channel, SinkHit};

const HEADER: usize = 12;
const QR: u16 = 0x8000;
const AA: u16 = 0x0400;
const RD: u16 = 0x0100;
const OPCODE_MASK: u16 = 0x7800;
const NOERROR: u16 = 0;
const NOTIMP: u16 = 4;
const REFUSED: u16 = 5;
pub const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;

type Hits = Arc<Mutex<Vec<SinkHit>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    apex: String,
}

impl Zone {
    #[must_use]
    pub fn new(apex: &str) -> Self {
        Self { apex: apex.trim_end_matches('.').to_ascii_lowercase() }
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        let name = name.trim_end_matches('.');
        name == self.apex || name.strip_suffix(&self.apex).is_some_and(|p| p.ends_with('.'))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub id: u16,
    pub flags: u16,
    /// Lowercased, dot-joined, no trailing dot.
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    question_end: usize,
}

fn be16(p: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*p.get(at)?, *p.get(at + 1)?]))
}

/// Parse the first question of a DNS query.
///
/// Responses, empty questions, truncated packets and compressed names are not
/// queries a stub resolver sends, so they yield `None`.
#[must_use]
pub fn parse_query(packet: &[u8]) -> Option<Query> {
    let id = be16(packet, 0)?;
    let flags = be16(packet, 2)?;
    if flags & QR != 0 || be16(packet, 4)? == 0 {
        return None;
    }
    let mut at = HEADER;
    let mut labels = Vec::new();
    let mut total = 0usize;
    loop {
        let len = usize::from(*packet.get(at)?);
        at += 1;
        if len == 0 {
            break;
        }
        if len > 63 {
            return None;
        }
        total += len + 1;
        if total > 255 {
            return None;
        }
        labels.push(String::from_utf8_lossy(packet.get(at..at + len)?).to_ascii_lowercase());
        at += len;
    }
    let qtype = be16(packet, at)?;
    let qclass = be16(packet, at + 2)?;
    Some(Query { id, flags, name: labels.join("."), qtype, qclass, question_end: at + 4 })
}

/// Build the reply to `query` (parsed from `packet`).
///
/// In-zone names get an authoritative answer: the `answer` address for an `A`
/// query, no data otherwise. Anything else is refused.
#[must_use]
pub fn respond(packet: &[u8], query: &Query, zone: &Zone, answer: Option<Ipv4Addr>) -> Vec<u8> {
    let opcode = query.flags & OPCODE_MASK;
    let (rcode, aa) = if opcode != 0 {
        (NOTIMP, 0)
    } else if zone.contains(&query.name) {
        (NOERROR, AA)
    } else {
        (REFUSED, 0)
    };
    let ip =
        answer.filter(|_| rcode == NOERROR && query.qtype == TYPE_A && query.qclass == CLASS_IN);
    let flags = QR | opcode | aa | (query.flags & RD) | rcode;
    let mut out = Vec::with_capacity(query.question_end + 16);
    out.extend_from_slice(&query.id.to_be_bytes());
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&u16::from(ip.is_some()).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(packet.get(HEADER..query.question_end).unwrap_or_default());
    if let Some(ip) = ip {
        out.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4]);
        out.extend_from_slice(&ip.octets());
    }
    out
}

fn record(hits: &Hits, hit: SinkHit) {
    hits.lock().unwrap_or_else(PoisonError::into_inner).push(hit);
}

fn snapshot_json(hits: &Hits) -> String {
    let snap = hits.lock().unwrap_or_else(PoisonError::into_inner).clone();
    serde_json::to_string(&snap).unwrap_or_else(|_| "[]".to_owned())
}

/// Record one datagram and build its reply; `None` for anything not a query.
#[must_use]
pub fn handle(
    packet: &[u8],
    peer: &str,
    zone: &Zone,
    answer: Option<Ipv4Addr>,
    hits: &Arc<Mutex<Vec<SinkHit>>>,
) -> Option<Vec<u8>> {
    let query = parse_query(packet)?;
    let token = canary::scan(&query.name).into_iter().next().map_or_else(String::new, |c| c.token);
    record(
        hits,
        SinkHit {
            channel: Channel::Dns,
            peer: peer.to_owned(),
            token,
            detail: format!("qtype={} {}", query.qtype, query.name),
        },
    );
    Some(respond(packet, &query, zone, answer))
}

async fn serve_udp(socket: UdpSocket, zone: Zone, answer: Option<Ipv4Addr>, hits: Hits) {
    let mut buf = vec![0u8; 1500];
    loop {
        let Ok((n, peer)) = socket.recv_from(&mut buf).await else { continue };
        if let Some(reply) = handle(&buf[..n], &peer.to_string(), &zone, answer, &hits) {
            let _ = socket.send_to(&reply, peer).await;
        }
    }
}

async fn serve_hits(listener: TcpListener, hits: Hits) {
    loop {
        let Ok((mut conn, _)) = listener.accept().await else { continue };
        let hits = Arc::clone(&hits);
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let n = conn.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let (status, body) = if request.starts_with("GET /hits") {
                ("200 OK", snapshot_json(&hits))
            } else if request.starts_with("GET /ready") {
                ("200 OK", "ready".to_owned())
            } else {
                ("404 Not Found", String::new())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = conn.write_all(response.as_bytes()).await;
            let _ = conn.shutdown().await;
        });
    }
}

/// Run the DNS sink on `dns_addr` (UDP) with its hit log on `http_addr`
/// (`GET /hits`, `GET /ready`). Never returns.
pub async fn run(
    dns_addr: &str,
    http_addr: &str,
    zone: &str,
    answer: Option<Ipv4Addr>,
) -> anyhow::Result<()> {
    let hits: Hits = Arc::default();
    let socket = UdpSocket::bind(dns_addr).await?;
    let listener = TcpListener::bind(http_addr).await?;
    let udp = tokio::spawn(serve_udp(socket, Zone::new(zone), answer, Arc::clone(&hits)));
    let http = tokio::spawn(serve_hits(listener, hits));
    let _ = tokio::try_join!(udp, http)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const CANARY: &str = "cnry-0123456789ab-gh-0123456789abcdef";
    const ANSWER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

    fn query(id: u16, name: &str, qtype: u16, flags: u16) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&id.to_be_bytes());
        p.extend_from_slice(&flags.to_be_bytes());
        p.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        for label in name.split('.') {
            p.push(u8::try_from(label.len()).unwrap());
            p.extend_from_slice(label.as_bytes());
        }
        p.push(0);
        p.extend_from_slice(&qtype.to_be_bytes());
        p.extend_from_slice(&CLASS_IN.to_be_bytes());
        p
    }

    fn zone() -> Zone {
        Zone::new("sink.example.com.")
    }

    fn reply_flags(reply: &[u8]) -> u16 {
        be16(reply, 2).unwrap()
    }

    #[test]
    fn a_query_parses_to_a_lowercased_name_and_its_type() {
        let q = parse_query(&query(7, "AbC.Sink.Example.COM", 16, RD)).unwrap();
        assert_eq!(q.id, 7);
        assert_eq!(q.name, "abc.sink.example.com");
        assert_eq!(q.qtype, 16);
        assert_eq!(q.qclass, CLASS_IN);
    }

    #[test]
    fn an_in_zone_canary_query_is_answered_and_recorded_with_its_canary() {
        let hits: Hits = Arc::default();
        let name = format!("{}.sink.example.com", CANARY.to_ascii_uppercase());
        let reply = handle(
            &query(42, &name, TYPE_A, RD),
            "198.51.100.7:5353",
            &zone(),
            Some(ANSWER),
            &hits,
        )
        .unwrap();
        assert_eq!(be16(&reply, 0), Some(42));
        let flags = reply_flags(&reply);
        assert_eq!(flags & QR, QR);
        assert_eq!(flags & AA, AA);
        assert_eq!(flags & RD, RD);
        assert_eq!(flags & 0x000F, NOERROR);
        assert_eq!(be16(&reply, 6), Some(1), "one answer");
        assert_eq!(&reply[reply.len() - 4..], &ANSWER.octets());
        let snap = hits.lock().unwrap().clone();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].channel, Channel::Dns);
        assert_eq!(snap[0].token, CANARY);
        assert_eq!(snap[0].peer, "198.51.100.7:5353");
        assert!(snap[0].detail.contains("qtype=1"));
    }

    #[test]
    fn an_out_of_zone_query_is_refused_but_still_recorded() {
        let hits: Hits = Arc::default();
        let reply =
            handle(&query(1, "www.example.org", TYPE_A, RD), "p", &zone(), Some(ANSWER), &hits)
                .unwrap();
        let flags = reply_flags(&reply);
        assert_eq!(flags & 0x000F, REFUSED);
        assert_eq!(flags & AA, 0);
        assert_eq!(be16(&reply, 6), Some(0));
        let snap = hits.lock().unwrap().clone();
        assert_eq!(snap.len(), 1);
        assert!(snap[0].token.is_empty());
    }

    #[test]
    fn a_non_a_in_zone_query_gets_an_authoritative_empty_answer() {
        let hits: Hits = Arc::default();
        let reply =
            handle(&query(3, "x.sink.example.com", 28, 0), "p", &zone(), Some(ANSWER), &hits)
                .unwrap();
        assert_eq!(reply_flags(&reply) & 0x000F, NOERROR);
        assert_eq!(reply_flags(&reply) & AA, AA);
        assert_eq!(be16(&reply, 6), Some(0));
    }

    #[test]
    fn zone_membership_is_by_label_not_by_suffix() {
        let z = zone();
        assert!(z.contains("sink.example.com"));
        assert!(z.contains("a.b.sink.example.com."));
        assert!(!z.contains("evilsink.example.com"));
        assert!(!z.contains("example.com"));
    }

    #[test]
    fn responses_truncations_compression_and_empty_questions_are_not_queries() {
        let hits: Hits = Arc::default();
        let good = query(9, "a.sink.example.com", TYPE_A, 0);
        let mut response = good.clone();
        response[2] |= 0x80;
        let mut empty = good.clone();
        empty[5] = 0;
        let mut compressed = good[..HEADER].to_vec();
        compressed.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1]);
        for bad in
            [&response[..], &good[..good.len() - 2], &empty[..], &compressed[..], &[0u8; 5][..]]
        {
            assert!(handle(bad, "p", &zone(), None, &hits).is_none());
        }
        assert!(hits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_udp_responder_answers_over_a_real_socket_and_logs_the_hit() {
        let hits: Hits = Arc::default();
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let task = tokio::spawn(serve_udp(server, zone(), Some(ANSWER), Arc::clone(&hits)));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let name = format!("{CANARY}.sink.example.com");
        client.send_to(&query(5, &name, TYPE_A, RD), addr).await.unwrap();
        let mut buf = [0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
            .await
            .expect("reply in time")
            .unwrap();
        assert_eq!(be16(&buf[..n], 0), Some(5));
        assert_eq!(&buf[n - 4..n], &ANSWER.octets());
        assert!(snapshot_json(&hits).contains(CANARY));
        task.abort();
    }
}
