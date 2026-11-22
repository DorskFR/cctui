//! Blob types shared with other domains.
use cctui_proto::adapter::AdapterEvent;
use serde_json::{Map, Value};

pub struct StoredBlob {
    pub hash: String,
    pub created: bool,
}

fn image_label(block: &Map<String, Value>) -> String {
    let source = block.get("source").and_then(Value::as_object);
    let field = |k: &str| source.and_then(|s| s.get(k));
    let media_type = field("media_type").and_then(Value::as_str).unwrap_or("image");
    let size = field("size").and_then(Value::as_u64).or_else(|| {
        field("data").and_then(Value::as_str).map(|d| {
            let pad = d.bytes().rev().take_while(|b| *b == b'=').count();
            (d.len() / 4 * 3).saturating_sub(pad) as u64
        })
    });
    size.map_or_else(
        || format!("[image {media_type}]"),
        |n| format!("[image {media_type} {} KB]", n.div_ceil(1024)),
    )
}

/// The text blocks of a stringified content array that carries images, with
/// a label per image; `None` for any other summary.
#[must_use]
pub fn readable_result(summary: &str) -> Option<String> {
    let head = summary.trim_start();
    if !head.starts_with('[') || !head.contains("\"image\"") {
        return None;
    }
    let Value::Array(blocks) = serde_json::from_str::<Value>(head).ok()? else {
        return None;
    };
    let mut parts = Vec::new();
    let mut images = 0;
    for block in &blocks {
        let block = block.as_object()?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(block.get("text").and_then(Value::as_str)?.to_owned()),
            Some("image") => {
                images += 1;
                parts.push(image_label(block));
            }
            _ => return None,
        }
    }
    (images > 0).then(|| parts.join("\n\n"))
}

pub const INLINE_IMAGES_MAX_BYTES: usize = 8 * 1024 * 1024;

pub fn set_payload(event: &mut AdapterEvent, rewritten: &Value) {
    if let AdapterEvent::Message { payload, .. } | AdapterEvent::ToolUse { payload, .. } = event {
        payload.clone_from(rewritten);
    }
}
