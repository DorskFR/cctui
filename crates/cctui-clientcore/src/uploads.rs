pub use cctui_proto::uploads::{MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES, UploadCaps};

#[must_use]
pub fn default_caps() -> UploadCaps {
    UploadCaps::default()
}

#[must_use]
pub fn over_file_cap(size: u64, caps: UploadCaps) -> bool {
    size > caps.max_file_bytes
}

#[must_use]
pub fn over_total_cap(sizes: &[u64], caps: UploadCaps) -> bool {
    sizes.iter().sum::<u64>() > caps.max_total_bytes
}

#[must_use]
pub fn over_count_cap(count: usize, caps: UploadCaps) -> bool {
    count > caps.max_files as usize
}
