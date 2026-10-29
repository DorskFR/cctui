use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// Serialize `value` with a content-hash `ETag`; a matching `If-None-Match`
/// yields an empty 304 so unchanged polls cost ~0 bytes on the wire.
/// `Cache-Control: private, no-cache` makes browsers store the body but
/// revalidate every time (the fetch layer then sends `If-None-Match` itself).
pub fn json_with_etag<T: serde::Serialize>(req_headers: &HeaderMap, value: &T) -> Response {
    let body = match serde_json::to_vec(value) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("response serialization failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let etag = format!("\"{:x}\"", Sha256::digest(&body));

    let matched = req_headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|inm| inm.split(',').any(|t| t.trim().trim_start_matches("W/") == etag));

    let mut headers = HeaderMap::new();
    headers.insert(ETAG, etag.parse().expect("hex etag is a valid header value"));
    // Envoy's compressor strips `ETag` from responses it compresses; the
    // webui reads this mirror instead and revalidates via `If-None-Match`.
    headers.insert("x-etag", etag.parse().expect("hex etag is a valid header value"));
    headers.insert(CACHE_CONTROL, "private, no-cache".parse().expect("static header"));
    if matched {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    headers.insert(CONTENT_TYPE, "application/json".parse().expect("static header"));
    (StatusCode::OK, headers, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn etag_of(resp: &Response) -> String {
        resp.headers().get(ETAG).unwrap().to_str().unwrap().to_owned()
    }

    #[test]
    fn fresh_request_gets_200_with_etag() {
        let resp = json_with_etag(&HeaderMap::new(), &serde_json::json!({"a": 1}));
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(etag_of(&resp).starts_with('"'));
        assert_eq!(resp.headers().get("x-etag").unwrap().to_str().unwrap(), etag_of(&resp));
        assert_eq!(resp.headers().get(CACHE_CONTROL).unwrap(), "private, no-cache");
    }

    #[test]
    fn matching_if_none_match_gets_304() {
        let value = serde_json::json!({"a": 1});
        let etag = etag_of(&json_with_etag(&HeaderMap::new(), &value));
        let mut req = HeaderMap::new();
        req.insert(IF_NONE_MATCH, etag.parse().unwrap());
        let resp = json_with_etag(&req, &value);
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn changed_body_gets_200_again() {
        let etag = etag_of(&json_with_etag(&HeaderMap::new(), &serde_json::json!({"a": 1})));
        let mut req = HeaderMap::new();
        req.insert(IF_NONE_MATCH, etag.parse().unwrap());
        let resp = json_with_etag(&req, &serde_json::json!({"a": 2}));
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn weak_and_multi_etag_lists_match() {
        let value = serde_json::json!([1, 2, 3]);
        let etag = etag_of(&json_with_etag(&HeaderMap::new(), &value));
        let mut req = HeaderMap::new();
        req.insert(IF_NONE_MATCH, format!("\"zzz\", W/{etag}").parse().unwrap());
        let resp = json_with_etag(&req, &value);
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }
}
