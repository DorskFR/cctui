use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use cctui_proto::chunk::{Accept, Reassembler};
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use serde_json::Value;

/// Bound the memory a single in-flight chunked transfer may buffer: a fixed
/// memory-safety backstop, not a product setting, so it stays unconfigurable.
pub(super) const MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

/// Drop partial chunked transfers idle past this age.
pub(super) const STALE_TRANSFER: std::time::Duration = std::time::Duration::from_mins(10);

/// Feed one chunk into the connection's reassembler and produce the ack to send
/// back plus, on completion, the reassembled inner frame to process.
/// `codec` decompresses the joined payload before parsing when set.
pub(super) fn handle_chunk(
    reasm: &mut Reassembler,
    transfer_id: String,
    chunk_index: u32,
    total_chunks: u32,
    data: &str,
    codec: Option<&str>,
) -> (DaemonFrameDown, Option<DaemonFrameUp>) {
    match reasm.accept(&transfer_id, chunk_index, total_chunks, data) {
        Accept::Pending(highest_contiguous_chunk) => {
            (DaemonFrameDown::ChunkAck { transfer_id, highest_contiguous_chunk }, None)
        }
        Accept::Complete(bytes) => {
            let ack = DaemonFrameDown::ChunkAck {
                transfer_id,
                highest_contiguous_chunk: total_chunks.checked_sub(1),
            };
            let joined = match codec {
                Some(codec) => match cctui_proto::compress::decompress_codec(codec, &bytes) {
                    Ok(b) => b,
                    Err(err) => {
                        tracing::warn!(%err, "reassembled chunk failed to decompress");
                        return (ack, None);
                    }
                },
                None => bytes,
            };
            (ack, parse_frame(&joined))
        }
        Accept::Restart => {
            (DaemonFrameDown::ChunkAck { transfer_id, highest_contiguous_chunk: None }, None)
        }
    }
}

/// Decode a `Compressed` envelope to its inner frame, or `None` on a
/// bad codec / base64 / payload — logged and dropped like any malformed frame.
pub(super) fn decode_compressed_frame(codec: &str, data: &str) -> Option<DaemonFrameUp> {
    match cctui_proto::compress::decode_compressed(codec, data) {
        Ok(bytes) => parse_frame(&bytes),
        Err(err) => {
            tracing::warn!(%err, "compressed frame failed to decode");
            None
        }
    }
}

/// Decode a binary WS message: the raw zstd payload a `Compressed` frame
/// carries base64-encoded.
pub(super) fn decode_binary_frame(data: &[u8]) -> Option<DaemonFrameUp> {
    match cctui_proto::compress::decompress_codec(cctui_proto::compress::CODEC_ZSTD, data) {
        Ok(bytes) => parse_frame(&bytes),
        Err(err) => {
            tracing::warn!(%err, "binary frame failed to decompress");
            None
        }
    }
}

/// Parse one up-frame. A frame this build cannot decode, such as an event kind
/// from a newer daemon or a malformed one, is skipped on its own: the rest of
/// its `Batch` still goes through. The whole-frame parse runs first, so a
/// well-formed frame never pays for the per-item fallback.
pub(super) fn parse_frame(bytes: &[u8]) -> Option<DaemonFrameUp> {
    match serde_json::from_slice::<DaemonFrameUp>(bytes) {
        Ok(frame) => Some(frame),
        Err(err) => serde_json::from_slice::<Value>(bytes).map_or_else(
            |_| {
                tracing::warn!(%err, "bad daemon frame");
                None
            },
            parse_lenient,
        ),
    }
}

fn parse_lenient(mut value: Value) -> Option<DaemonFrameUp> {
    if value.get("type").and_then(Value::as_str) == Some("batch")
        && let Some(Value::Array(items)) = value.get_mut("frames").map(Value::take)
    {
        let frames = items.into_iter().filter_map(parse_lenient).collect();
        return Some(DaemonFrameUp::Batch { frames });
    }
    let kind = frame_kind(&value);
    match serde_json::from_value::<DaemonFrameUp>(value) {
        Ok(frame) => Some(frame),
        Err(err) => {
            if first_skip_of(&kind) {
                tracing::warn!(%err, %kind, "skipping undecodable daemon frames of this kind");
            }
            None
        }
    }
}

/// `event/<kind>` for an `Event`, else the frame's own `type`.
fn frame_kind(value: &Value) -> String {
    let tag = |v: &Value| v.get("type").and_then(Value::as_str).unwrap_or("?").to_owned();
    match value.get("event") {
        Some(event) if tag(value) == "event" => format!("event/{}", tag(event)),
        _ => tag(value),
    }
}

/// Bounded so a daemon inventing kinds cannot grow the set without limit.
const MAX_SKIPPED_KINDS: usize = 64;

static SKIPPED_KINDS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn first_skip_of(kind: &str) -> bool {
    let Ok(mut seen) = SKIPPED_KINDS.lock() else { return false };
    if seen.contains(kind) || seen.len() >= MAX_SKIPPED_KINDS {
        return false;
    }
    seen.insert(kind.to_owned())
}

/// Flatten a decoded inner frame into the leaf frames to process: a `Batch`
/// yields its events in order, anything else is a single leaf.
pub(super) fn expand_batch(frame: DaemonFrameUp) -> Vec<DaemonFrameUp> {
    match frame {
        DaemonFrameUp::Batch { frames } => frames,
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cctui_proto::chunk::split;
    use serde_json::json;

    use super::*;

    fn big_event(local_id: &str, filler: char) -> (DaemonFrameUp, Vec<DaemonFrameUp>) {
        let event = DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: cctui_proto::adapter::AdapterEvent::Message {
                local_id: local_id.into(),
                payload: json!({ "text": filler.to_string().repeat(600 * 1024) }),
                turn_id: None,
            },
        };
        let bytes = serde_json::to_vec(&event).unwrap();
        let chunks = split(&bytes).expect("payload must exceed the chunk threshold");
        (event, chunks)
    }

    fn feed(
        reasm: &mut Reassembler,
        frame: &DaemonFrameUp,
    ) -> (DaemonFrameDown, Option<DaemonFrameUp>) {
        let DaemonFrameUp::Chunk { transfer_id, chunk_index, total_chunks, data, codec } = frame
        else {
            panic!("not a chunk frame");
        };
        handle_chunk(
            reasm,
            transfer_id.clone(),
            *chunk_index,
            *total_chunks,
            data,
            codec.as_deref(),
        )
    }

    #[test]
    fn chunk_ack_reports_highest_contiguous_prefix() {
        let (_event, chunks) = big_event("s1", 'a');
        assert!(chunks.len() >= 3);
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);

        let (ack, inner) = feed(&mut reasm, &chunks[0]);
        assert!(inner.is_none());
        assert!(matches!(ack, DaemonFrameDown::ChunkAck { highest_contiguous_chunk: Some(0), .. }));

        // A gap at chunk 1 keeps the acked prefix at 0 even after chunk 2 lands.
        let (ack, _) = feed(&mut reasm, &chunks[2]);
        assert!(matches!(ack, DaemonFrameDown::ChunkAck { highest_contiguous_chunk: Some(0), .. }));
    }

    #[test]
    fn completed_transfer_yields_the_original_inner_frame() {
        let (event, chunks) = big_event("s1", 'b');
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let mut recovered = None;
        for c in &chunks {
            if let (_, Some(inner)) = feed(&mut reasm, c) {
                recovered = Some(inner);
            }
        }
        let recovered = recovered.expect("transfer never completed");
        assert_eq!(
            serde_json::to_value(&recovered).unwrap(),
            serde_json::to_value(&event).unwrap(),
        );
        assert!(reasm.is_empty(), "completed transfer is dropped from the buffer");
    }

    #[test]
    fn interleaved_transfers_reassemble_independently() {
        let (ev_a, ca) = big_event("s-a", 'a');
        let (ev_b, cb) = big_event("s-b", 'b');
        assert_eq!(ca.len(), cb.len());
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let mut done = vec![];
        for i in 0..ca.len() {
            if let (_, Some(inner)) = feed(&mut reasm, &ca[i]) {
                done.push(serde_json::to_value(&inner).unwrap());
            }
            if let (_, Some(inner)) = feed(&mut reasm, &cb[i]) {
                done.push(serde_json::to_value(&inner).unwrap());
            }
        }
        assert_eq!(
            done,
            vec![serde_json::to_value(&ev_a).unwrap(), serde_json::to_value(&ev_b).unwrap()],
        );
        assert!(reasm.is_empty());
    }

    #[test]
    fn no_usable_prefix_nacks_restart() {
        let (_event, chunks) = big_event("s1", 'c');
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        // Chunk 1 before chunk 0: nothing contiguous yet, so the daemon restarts.
        let (ack, inner) = feed(&mut reasm, &chunks[1]);
        assert!(inner.is_none());
        assert!(matches!(ack, DaemonFrameDown::ChunkAck { highest_contiguous_chunk: None, .. }));
    }

    #[test]
    fn stale_buffers_are_evicted() {
        let (_event, chunks) = big_event("s1", 'd');
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let _ = feed(&mut reasm, &chunks[0]);
        assert_eq!(reasm.len(), 1);
        reasm.evict_older_than(Duration::ZERO);
        assert!(reasm.is_empty(), "the server drops partial transfers past the stale age");
    }

    fn event(local_id: &str) -> DaemonFrameUp {
        DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: cctui_proto::adapter::AdapterEvent::Message {
                local_id: local_id.into(),
                payload: json!({ "text": "x".repeat(8 * 1024) }),
                turn_id: None,
            },
        }
    }

    #[test]
    fn decodes_legacy_plain_frame() {
        // An old daemon's plain Event (no compression/batching) is a single leaf.
        let leaves = expand_batch(event("s1"));
        assert_eq!(leaves.len(), 1);
        assert!(matches!(leaves[0], DaemonFrameUp::Event { .. }));
    }

    #[test]
    fn decodes_compressed_frame() {
        let inner = event("s1");
        let json = serde_json::to_vec(&inner).unwrap();
        let compressed = cctui_proto::compress::zstd_compress(&json);
        let DaemonFrameUp::Compressed { codec, data } =
            cctui_proto::compress::compressed_frame("zstd", &compressed)
        else {
            panic!("compressed_frame must build a Compressed");
        };
        let decoded = decode_compressed_frame(&codec, &data).expect("must decode");
        assert_eq!(serde_json::to_value(&decoded).unwrap(), serde_json::to_value(&inner).unwrap());
    }

    #[test]
    fn decodes_binary_frame_like_its_compressed_twin() {
        let inner =
            DaemonFrameUp::Batch { frames: (0..3).map(|i| event(&format!("s{i}"))).collect() };
        let compressed = cctui_proto::compress::zstd_compress(&serde_json::to_vec(&inner).unwrap());
        let DaemonFrameUp::Compressed { codec, data } =
            cctui_proto::compress::compressed_frame("zstd", &compressed)
        else {
            panic!("compressed_frame must build a Compressed");
        };
        let via_text = expand_batch(decode_compressed_frame(&codec, &data).expect("text decodes"));
        let via_binary = expand_batch(decode_binary_frame(&compressed).expect("binary decodes"));
        assert_eq!(via_binary.len(), 3);
        assert_eq!(
            serde_json::to_value(&via_binary).unwrap(),
            serde_json::to_value(&via_text).unwrap(),
        );
        assert!(decode_binary_frame(b"{\"not\":\"zstd\"}").is_none());
    }

    #[test]
    fn decodes_batch_frame_in_order() {
        let batch =
            DaemonFrameUp::Batch { frames: (0..4).map(|i| event(&format!("s{i}"))).collect() };
        let leaves = expand_batch(batch);
        assert_eq!(leaves.len(), 4);
    }

    #[test]
    fn bad_compressed_frame_is_dropped() {
        assert!(decode_compressed_frame("zstd", "!!not base64!!").is_none());
        assert!(decode_compressed_frame("brotli", "AAAA").is_none());
    }

    #[test]
    fn an_unknown_event_is_skipped_and_its_batch_siblings_survive() {
        let known = |id: &str| serde_json::to_value(event(id)).unwrap();
        let unknown = json!({
            "type": "event",
            "adapter_id": "claude-code",
            "event": { "type": "from_the_future", "local_id": "s1", "x": 1 },
        });
        let malformed = json!({
            "type": "event",
            "adapter_id": "claude-code",
            "event": { "type": "message", "local_id": 7 },
        });
        let garbage = json!({ "type": "nope" });
        let mut newer = known("s1");
        newer["from_the_future"] = json!(1);
        newer["event"]["from_the_future"] = json!({ "x": true });
        let frame = json!({
            "type": "batch",
            "frames": [known("s0"), unknown, newer, malformed, garbage, known("s2")],
        });
        let bytes = serde_json::to_vec(&frame).unwrap();
        let leaves = expand_batch(parse_frame(&bytes).expect("the batch survives"));
        let ids: Vec<_> = leaves
            .iter()
            .map(|f| match f {
                DaemonFrameUp::Event {
                    event: cctui_proto::adapter::AdapterEvent::Message { local_id, .. },
                    ..
                } => local_id.as_str(),
                other => panic!("only the known events remain, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["s0", "s1", "s2"]);
    }

    #[test]
    fn an_unknown_event_inside_a_compressed_batch_spares_its_siblings() {
        let mut frame =
            serde_json::to_value(DaemonFrameUp::Batch { frames: vec![event("s0"), event("s1")] })
                .unwrap();
        let unknown = json!({"type":"event","adapter_id":"a","event":{"type":"from_the_future"}});
        frame["frames"].as_array_mut().unwrap().insert(1, unknown);
        let compressed = cctui_proto::compress::zstd_compress(&serde_json::to_vec(&frame).unwrap());
        let leaves = expand_batch(decode_binary_frame(&compressed).expect("decodes"));
        assert_eq!(leaves.len(), 2);
    }

    #[test]
    fn unknown_fields_on_known_events_take_the_fast_path() {
        let mut frame =
            serde_json::to_value(DaemonFrameUp::Batch { frames: vec![event("s0"), event("s1")] })
                .unwrap();
        frame["frames"][0]["event"]["from_the_future"] = json!([1, 2]);
        frame["from_the_future"] = json!("x");
        let bytes = serde_json::to_vec(&frame).unwrap();
        assert!(serde_json::from_slice::<DaemonFrameUp>(&bytes).is_ok());
        assert_eq!(expand_batch(parse_frame(&bytes).unwrap()).len(), 2);
    }

    #[test]
    fn a_lone_unknown_frame_or_garbage_is_dropped() {
        let lone = json!({"type":"event","adapter_id":"a","event":{"type":"from_the_future"}});
        assert!(parse_frame(&serde_json::to_vec(&lone).unwrap()).is_none());
        assert!(parse_frame(b"not json").is_none());
    }

    #[test]
    fn a_skipped_kind_is_logged_once() {
        let kind = "event/once_only_probe";
        assert!(first_skip_of(kind));
        assert!(!first_skip_of(kind));
    }

    #[test]
    fn frame_kind_names_the_event_inside_an_event_frame() {
        assert_eq!(frame_kind(&json!({"type":"event","event":{"type":"title"}})), "event/title");
        assert_eq!(frame_kind(&json!({"type":"heartbeat"})), "heartbeat");
        assert_eq!(frame_kind(&json!({})), "?");
    }

    #[test]
    fn decodes_chunked_compressed_batch() {
        // Full compose: a batch too big for one message, compressed then chunked.
        // The server reassembles, decompresses via the chunk codec, parses the
        // inner Batch, and expands it back to its events.
        let mut rng = 0x1234_5678_9abc_def0_u64;
        let mut blob = |n: usize| {
            let bytes: Vec<u8> = (0..n)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    rng as u8
                })
                .collect();
            hex::encode(bytes)
        };
        let frames: Vec<DaemonFrameUp> = (0..80)
            .map(|i| DaemonFrameUp::Event {
                adapter_id: "claude-code".into(),
                event: cctui_proto::adapter::AdapterEvent::Message {
                    local_id: format!("s{i}"),
                    payload: json!({ "n": i, "blob": blob(4000) }),
                    turn_id: None,
                },
            })
            .collect();
        let want = frames.len();
        let inner = DaemonFrameUp::Batch { frames };
        let serialized = serde_json::to_vec(&inner).unwrap();
        let compressed = cctui_proto::compress::zstd_compress(&serialized);
        let id = cctui_proto::chunk::transfer_id(&compressed);
        let total = cctui_proto::chunk::chunk_count(compressed.len());
        assert!(total > 1, "compressed batch must span multiple chunks");

        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let mut recovered = None;
        for i in 0..total {
            let frame = cctui_proto::chunk::chunk_frame(&id, &compressed, i, total, Some("zstd"));
            if let (_, Some(inner)) = feed(&mut reasm, &frame) {
                recovered = Some(inner);
            }
        }
        let leaves = expand_batch(recovered.expect("transfer completed"));
        assert_eq!(leaves.len(), want, "chunked+compressed batch expands to its events");
    }
}
