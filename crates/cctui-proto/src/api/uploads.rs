//! Upload caps shared by the spawn-bootstrap and attachment routes.

/// Built-in upload caps, used when the instance has none stored.
///
/// The bytes ride the server→daemon WS leg as base64 inside a single JSON frame, so this is
/// deliberately an "attach a screenshot / small doc" budget, not bulk transfer. A route's
/// `DefaultBodyLimit` must stay above the effective total cap so an over-cap upload is rejected
/// here with a clear 413 rather than a generic body-limit error.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_FILES: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
// The names are the JSON wire format and the exported TS binding.
#[allow(clippy::struct_field_names)]
pub struct UploadCaps {
    pub max_files: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub max_file_bytes: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub max_total_bytes: u64,
}

impl Default for UploadCaps {
    fn default() -> Self {
        Self {
            max_files: MAX_FILES,
            max_file_bytes: MAX_FILE_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
        }
    }
}
