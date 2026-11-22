use axum::http::{HeaderMap, HeaderValue, header};

use super::origin_permitted;
use cctui_server_kernel::config::Config;

fn cfg() -> Config {
    Config::for_test(vec!["https://cctui.example.com".to_owned()])
}

#[test]
fn absent_origin_is_allowed() {
    assert!(origin_permitted(&cfg(), &HeaderMap::new()));
}

#[test]
fn allowlisted_origin_is_allowed() {
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGIN, HeaderValue::from_static("https://cctui.example.com"));
    assert!(origin_permitted(&cfg(), &headers));
}

#[test]
fn foreign_origin_is_rejected() {
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example.com"));
    assert!(!origin_permitted(&cfg(), &headers));
}
