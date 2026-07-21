//! Chunked WS transfer (CCT-738): split a large serialized up-frame into
//! ordered, acked, resumable chunks and reassemble them server-side.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};

use crate::ws::DaemonFrameUp;

/// Serialized up-frames larger than this are chunked; at or below it they keep
/// the single-message fast path.
pub const CHUNK_THRESHOLD: usize = 256 * 1024;

/// Raw payload bytes per chunk.
pub const CHUNK_SIZE: usize = 256 * 1024;

/// Content-hash transfer id (hex sha256) for a serialized payload.
#[must_use]
pub fn transfer_id(payload: &[u8]) -> String {
    hex::encode(Sha256::digest(payload))
}

/// Number of chunks a `len`-byte payload splits into.
#[must_use]
pub fn chunk_count(len: usize) -> u32 {
    len.div_ceil(CHUNK_SIZE).try_into().unwrap_or(u32::MAX)
}

/// Build the `index`-th chunk frame of `payload` under `id`. `codec` tags the
/// whole transfer so the server decompresses the reassembled bytes (CCT-740).
#[must_use]
pub fn chunk_frame(
    id: &str,
    payload: &[u8],
    index: u32,
    total: u32,
    codec: Option<&str>,
) -> DaemonFrameUp {
    let start = (index as usize).saturating_mul(CHUNK_SIZE).min(payload.len());
    let end = start.saturating_add(CHUNK_SIZE).min(payload.len());
    DaemonFrameUp::Chunk {
        transfer_id: id.to_owned(),
        chunk_index: index,
        total_chunks: total,
        data: BASE64.encode(&payload[start..end]),
        codec: codec.map(str::to_owned),
    }
}

/// Split a serialized up-frame into ordered chunk frames, or `None` when it is
/// small enough for the single-message fast path.
#[must_use]
pub fn split(payload: &[u8]) -> Option<Vec<DaemonFrameUp>> {
    if payload.len() <= CHUNK_THRESHOLD {
        return None;
    }
    let id = transfer_id(payload);
    let total = chunk_count(payload.len());
    Some((0..total).map(|i| chunk_frame(&id, payload, i, total, None)).collect())
}

/// Outcome of feeding one chunk into a [`Reassembler`].
pub enum Accept {
    /// Still incomplete; ack the highest contiguous chunk index held so far
    /// (`None` when even chunk 0 is missing).
    Pending(Option<u32>),
    /// Every chunk is present; the reassembled payload.
    Complete(Vec<u8>),
    /// The chunk was inconsistent with the buffered transfer, over the size
    /// bound, or malformed — the daemon should restart from chunk 0.
    Restart,
}

struct Partial {
    total: u32,
    chunks: Vec<Option<Vec<u8>>>,
    bytes: usize,
    created: Instant,
}

impl Partial {
    fn highest_contiguous(&self) -> Option<u32> {
        let mut last = None;
        for (i, c) in self.chunks.iter().enumerate() {
            if c.is_none() {
                break;
            }
            last = Some(u32::try_from(i).unwrap_or(u32::MAX));
        }
        last
    }

    fn is_complete(&self) -> bool {
        self.chunks.iter().all(Option::is_some)
    }

    fn assemble(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes);
        for c in self.chunks.iter().flatten() {
            out.extend_from_slice(c);
        }
        out
    }
}

/// Per-connection chunk reassembly with a per-transfer byte bound and age-based
/// eviction (CCT-738). Bounds memory against a stalled or malicious daemon.
pub struct Reassembler {
    max_bytes: usize,
    transfers: HashMap<String, Partial>,
}

impl Reassembler {
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes, transfers: HashMap::new() }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.transfers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }

    pub fn accept(&mut self, id: &str, index: u32, total: u32, data: &str) -> Accept {
        let max_bytes = self.max_bytes;
        let Ok(bytes) = BASE64.decode(data) else {
            return Accept::Restart;
        };
        if total == 0 || index >= total {
            return Accept::Restart;
        }
        if self.transfers.get(id).is_some_and(|p| p.total != total) {
            self.transfers.remove(id);
            return Accept::Restart;
        }
        let entry = self.transfers.entry(id.to_owned()).or_insert_with(|| Partial {
            total,
            chunks: vec![None; total as usize],
            bytes: 0,
            created: Instant::now(),
        });
        let slot = index as usize;
        match entry.chunks[slot].as_ref().map(Vec::len) {
            None => entry.bytes = entry.bytes.saturating_add(bytes.len()),
            Some(prev) => {
                entry.bytes = entry.bytes.saturating_sub(prev).saturating_add(bytes.len());
            }
        }
        entry.chunks[slot] = Some(bytes);
        if entry.bytes > max_bytes {
            self.transfers.remove(id);
            return Accept::Restart;
        }
        if entry.is_complete() {
            let payload = entry.assemble();
            self.transfers.remove(id);
            return Accept::Complete(payload);
        }
        Accept::Pending(entry.highest_contiguous())
    }

    /// Drop partial transfers older than `max_age`.
    pub fn evict_older_than(&mut self, max_age: Duration) {
        self.transfers.retain(|_, p| p.created.elapsed() < max_age);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
    }

    fn parts(frame: &DaemonFrameUp) -> (String, u32, u32, String) {
        match frame {
            DaemonFrameUp::Chunk { transfer_id, chunk_index, total_chunks, data, .. } => {
                (transfer_id.clone(), *chunk_index, *total_chunks, data.clone())
            }
            _ => panic!("not a chunk frame"),
        }
    }

    fn reassemble(frames: &[DaemonFrameUp]) -> Vec<u8> {
        let mut r = Reassembler::new(usize::MAX);
        let mut out = None;
        for f in frames {
            let (id, idx, total, data) = parts(f);
            if let Accept::Complete(p) = r.accept(&id, idx, total, &data) {
                out = Some(p);
            }
        }
        out.expect("reassembly never completed")
    }

    #[test]
    fn small_payloads_take_the_fast_path() {
        assert!(split(&pattern(0)).is_none());
        assert!(split(&pattern(1024)).is_none());
        assert!(split(&pattern(CHUNK_THRESHOLD)).is_none(), "exactly threshold must not split");
    }

    #[test]
    fn threshold_plus_one_splits_into_two_chunks() {
        let payload = pattern(CHUNK_THRESHOLD + 1);
        let frames = split(&payload).expect("must split");
        assert_eq!(frames.len(), 2);
        assert_eq!(reassemble(&frames), payload);
    }

    #[test]
    fn split_reassemble_roundtrips_various_sizes() {
        for len in [
            CHUNK_THRESHOLD + 1,
            CHUNK_SIZE * 3,
            CHUNK_SIZE * 3 + 17,
            5 * 1024 * 1024,
            20 * 1024 * 1024,
        ] {
            let payload = pattern(len);
            let frames = split(&payload).expect("must split");
            assert_eq!(frames.len() as usize, len.div_ceil(CHUNK_SIZE));
            assert_eq!(reassemble(&frames), payload, "roundtrip failed at len={len}");
        }
    }

    #[test]
    fn reassembly_interleaves_two_transfers() {
        let a = pattern(CHUNK_SIZE * 2 + 3);
        let b = pattern(CHUNK_SIZE * 2 + 99);
        let (fa, fb) = (split(&a).unwrap(), split(&b).unwrap());
        let mut r = Reassembler::new(usize::MAX);
        let order = [&fa[0], &fb[0], &fa[1], &fb[1], &fa[2], &fb[2]];
        let mut done = vec![];
        for f in order {
            let (id, idx, total, data) = parts(f);
            if let Accept::Complete(p) = r.accept(&id, idx, total, &data) {
                done.push(p);
            }
        }
        assert_eq!(done, vec![a, b]);
        assert!(r.is_empty(), "completed transfers are dropped");
    }

    #[test]
    fn ack_reports_highest_contiguous_and_gaps_hold() {
        let payload = pattern(CHUNK_SIZE * 3 + 1);
        let f = split(&payload).unwrap();
        let mut r = Reassembler::new(usize::MAX);
        let ack = |o: Accept| match o {
            Accept::Pending(h) => h,
            _ => panic!("expected pending"),
        };
        assert_eq!(ack(feed(&mut r, &f[0])), Some(0));
        // A gap at chunk 1 pins the contiguous prefix at 0 even once 2 arrives.
        assert_eq!(ack(feed(&mut r, &f[2])), Some(0));
        assert_eq!(ack(feed(&mut r, &f[1])), Some(2));
    }

    fn feed(r: &mut Reassembler, f: &DaemonFrameUp) -> Accept {
        let (id, idx, total, data) = parts(f);
        r.accept(&id, idx, total, &data)
    }

    #[test]
    fn unknown_index_or_bad_data_requests_restart() {
        let mut r = Reassembler::new(usize::MAX);
        assert!(matches!(r.accept("x", 5, 3, "AAAA"), Accept::Restart), "index >= total");
        assert!(matches!(r.accept("x", 0, 0, "AAAA"), Accept::Restart), "zero total");
        assert!(matches!(r.accept("x", 0, 2, "!!not-base64!!"), Accept::Restart), "bad base64");
    }

    #[test]
    fn over_size_bound_evicts_and_restarts() {
        let payload = pattern(CHUNK_SIZE * 4);
        let f = split(&payload).unwrap();
        let mut r = Reassembler::new(CHUNK_SIZE / 2);
        assert!(matches!(feed(&mut r, &f[0]), Accept::Restart));
        assert!(r.is_empty(), "over-bound buffer is dropped");
    }

    #[test]
    fn stale_transfers_are_evicted() {
        let payload = pattern(CHUNK_SIZE * 2);
        let f = split(&payload).unwrap();
        let mut r = Reassembler::new(usize::MAX);
        let _ = feed(&mut r, &f[0]);
        assert_eq!(r.len(), 1);
        r.evict_older_than(Duration::ZERO);
        assert!(r.is_empty(), "a zero max-age evicts every buffered transfer");
    }

    #[test]
    fn idempotent_resend_keeps_byte_accounting_stable() {
        let payload = pattern(CHUNK_SIZE * 2 + 5);
        let f = split(&payload).unwrap();
        let mut r = Reassembler::new(payload.len() + 16);
        let _ = feed(&mut r, &f[0]);
        // A re-sent chunk must not double-count bytes toward the size bound.
        let _ = feed(&mut r, &f[0]);
        let _ = feed(&mut r, &f[0]);
        let _ = feed(&mut r, &f[1]);
        assert!(matches!(feed(&mut r, &f[2]), Accept::Complete(p) if p == payload));
    }
}
