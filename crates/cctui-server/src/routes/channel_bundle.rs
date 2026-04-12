use axum::http::header;
use axum::response::IntoResponse;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

static CHANNEL_BUNDLE: &[u8] = include_bytes!("../../../../channel/dist/channel.js");

struct BundleInfo {
    sha256: String,
}

static BUNDLE_INFO: OnceLock<BundleInfo> = OnceLock::new();

fn bundle_info() -> &'static BundleInfo {
    BUNDLE_INFO.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(CHANNEL_BUNDLE);
        BundleInfo { sha256: hex::encode(hasher.finalize()) }
    })
}

pub async fn latest_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], CHANNEL_BUNDLE)
}

pub async fn version_json() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "sha256": bundle_info().sha256,
    }))
}
