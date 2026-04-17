use axum::http::header;
use axum::response::IntoResponse;

static INSTALL_SH: &str = include_str!("../../../../scripts/install.sh");

pub async fn install_sh() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        INSTALL_SH,
    )
}
