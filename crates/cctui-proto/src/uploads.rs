//! Attachment caps, shared by the server, the clients and the TS bindings.
//!
//! A route's `DefaultBodyLimit` must stay above the effective total cap, or an
//! over-cap upload fails with a generic body-limit error instead of a 413.

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
