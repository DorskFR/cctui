/// The connection's peer address when the server was started with connect info.
///
/// `None` otherwise (a test router, or a serve path without it). Infallible so a
/// missing extension degrades to "unknown" instead of refusing the request.
pub struct PeerAddr(pub Option<std::net::SocketAddr>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for PeerAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

/// Who to charge for an unauthenticated request.
///
/// `X-Forwarded-For` is a list each proxy *appends* to, so the entries nearest
/// the right were written by the hops nearest us and everything a client sent
/// arrives pushed to the left. With `trusted_hops = n` the caller is therefore
/// the n-th entry from the right; a client that injects its own XFF only adds
/// entries we never read.
///
/// `trusted_hops = 0` means no header is believed and the connection's peer
/// address is used. That is the safe default: taking the leftmost entry (or any
/// entry at all without knowing the hop count) lets a caller mint a fresh
/// rate-limit bucket per request simply by rotating the header, which is the
/// one adversary this throttle exists for.
pub fn caller_key(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::SocketAddr>,
    trusted_hops: usize,
) -> String {
    if trusted_hops > 0
        && let Some(raw) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
    {
        let hops: Vec<&str> = raw.split(',').map(str::trim).filter(|h| !h.is_empty()).collect();
        // A chain shorter than the operator described means the configuration
        // does not match reality, so nothing in the header is known to come from
        // a trusted hop: fall through to the peer address rather than reach for
        // an entry the caller may have written.
        if let Some(addr) = hops.len().checked_sub(trusted_hops).and_then(|i| hops.get(i)) {
            return addr.chars().take(64).collect();
        }
    }
    if trusted_hops == 0 && headers.contains_key("x-forwarded-for") {
        tracing::debug!(
            "X-Forwarded-For present but CCTUI_TRUSTED_PROXY_HOPS is 0: rate limiting on the \
             peer address, so every caller behind the proxy shares one bucket"
        );
    }
    peer.map_or_else(|| "unknown".to_owned(), |addr| addr.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::caller_key;

    fn peer(ip: &str) -> std::net::SocketAddr {
        std::net::SocketAddr::new(ip.parse().unwrap(), 4000)
    }

    /// The whole point of the throttle: a caller must not be able to choose its
    /// own bucket. A proxy appends, so anything the client sent sits left of the
    /// entry our own hop wrote.
    #[test]
    fn a_forged_x_forwarded_for_cannot_move_the_caller_to_a_fresh_bucket() {
        let mut headers = axum::http::HeaderMap::new();
        // One trusted proxy: it appended the real client, 203.0.113.5. The two
        // entries to the left are whatever the client chose to send.
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8, 203.0.113.5".parse().unwrap());
        assert_eq!(caller_key(&headers, Some(peer("10.0.0.1")), 1), "203.0.113.5");

        // Rotating the forged prefix must not change the key.
        let mut rotated = axum::http::HeaderMap::new();
        rotated.insert("x-forwarded-for", "9.9.9.9, 8.8.8.8, 203.0.113.5".parse().unwrap());
        assert_eq!(
            caller_key(&rotated, Some(peer("10.0.0.1")), 1),
            caller_key(&headers, Some(peer("10.0.0.1")), 1),
            "the bucket is the trusted hop's view, not the client's"
        );

        // Two trusted hops: the caller is the second from the right.
        assert_eq!(caller_key(&headers, Some(peer("10.0.0.1")), 2), "5.6.7.8");
    }

    /// With no declared proxy, a header is not evidence of anything.
    #[test]
    fn with_no_trusted_hop_the_peer_address_is_used_and_headers_are_ignored() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        headers.insert("x-real-ip", "9.9.9.9".parse().unwrap());
        assert_eq!(caller_key(&headers, Some(peer("10.0.0.7")), 0), "10.0.0.7");

        // And a spoofer gains nothing by varying them.
        headers.insert("x-forwarded-for", "5.5.5.5".parse().unwrap());
        assert_eq!(caller_key(&headers, Some(peer("10.0.0.7")), 0), "10.0.0.7");

        // No peer address either (a router without connect info): one bucket.
        assert_eq!(caller_key(&headers, None, 0), "unknown");
    }

    /// A chain shorter than the operator claimed means the hop count is wrong, so
    /// no entry is known to be trustworthy and the header must be ignored.
    #[test]
    fn a_chain_shorter_than_the_configured_hops_falls_back_to_the_peer_address() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 203.0.113.5".parse().unwrap());
        assert_eq!(caller_key(&headers, Some(peer("10.0.0.1")), 3), "10.0.0.1");
        // Exactly as many entries as hops still resolves to the leftmost, which
        // is the entry the outermost trusted proxy wrote.
        assert_eq!(caller_key(&headers, Some(peer("10.0.0.1")), 2), "1.2.3.4");

        // An empty or whitespace-only header is no evidence; peer wins.
        let mut blank = axum::http::HeaderMap::new();
        blank.insert("x-forwarded-for", " , ".parse().unwrap());
        assert_eq!(caller_key(&blank, Some(peer("10.0.0.9")), 1), "10.0.0.9");
    }
}
