/// `reqwest` has no built-in provider: a `Client` built before this panics.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// No whole-request `timeout`: this client carries long gateway streams.
#[must_use]
pub fn build_http_client() -> reqwest::Client {
    install_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(HTTP_READ_TIMEOUT)
        .build()
        .expect("build http client")
}

pub const HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub const HTTP_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);
