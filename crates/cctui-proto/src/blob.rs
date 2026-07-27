//! Content-addressed blob references: oversized base64 attachments in
//! transcript payloads are uploaded by sha256 and replaced with a [`BlobRef`].

use serde::{Deserialize, Serialize};

/// Embedded base64 blobs whose decoded size exceeds this ride the HTTP blob
/// store instead of the WS event.
pub const BLOB_THRESHOLD_BYTES: usize = 512 * 1024;

/// The `source.type` a base64 block is rewritten to once uploaded.
pub const BLOB_SOURCE_TYPE: &str = "cctui-blob";

/// Replaces an oversized base64 `source` object. `blob_id` is the lowercase
/// sha256 hex of the raw bytes — the store key and GET path segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    #[serde(rename = "type")]
    pub kind: String,
    pub blob_id: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

impl BlobRef {
    #[must_use]
    pub fn new(blob_id: impl Into<String>, size: u64, media_type: Option<String>) -> Self {
        Self { kind: BLOB_SOURCE_TYPE.to_owned(), blob_id: blob_id.into(), size, media_type }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_ref_serializes_with_type_key() {
        let r = BlobRef::new("abc123", 1024, Some("image/png".to_owned()));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["type"], BLOB_SOURCE_TYPE);
        assert_eq!(v["blob_id"], "abc123");
        assert_eq!(v["size"], 1024);
        assert_eq!(v["media_type"], "image/png");
    }

    #[test]
    fn blob_ref_omits_absent_media_type() {
        let r = BlobRef::new("abc123", 1, None);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("media_type").is_none());
    }
}
