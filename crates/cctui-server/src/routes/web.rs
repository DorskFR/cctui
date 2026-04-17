use axum::http::header;
use axum::response::IntoResponse;

static INDEX_HTML: &str = include_str!("../../web/index.html");

pub async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        INDEX_HTML,
    )
}
