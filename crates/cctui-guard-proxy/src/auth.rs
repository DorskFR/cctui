//! Tokens of a guard-proxy pod, which every worker in the namespace can reach:
//! the proxy token admits this dispatch's worker, the control token (hidden
//! from the agent) steers the policy.

use std::path::Path;

use base64::Engine as _;

use crate::inject::header_value;

const MIN_TOKEN_LEN: usize = 32;

#[derive(Default)]
pub struct GuardAuth {
    proxy: Option<String>,
    control: Option<String>,
}

impl GuardAuth {
    pub fn load(proxy: Option<&Path>, control: Option<&Path>) -> anyhow::Result<Self> {
        Ok(Self {
            proxy: proxy.map(read_token).transpose()?,
            control: control.map(read_token).transpose()?,
        })
    }

    #[cfg(test)]
    pub fn with_tokens(proxy: Option<&str>, control: Option<&str>) -> Self {
        Self { proxy: proxy.map(ToOwned::to_owned), control: control.map(ToOwned::to_owned) }
    }

    pub fn proxy_token(&self) -> Option<&str> {
        self.proxy.as_deref()
    }

    /// `Proxy-Authorization`: `Basic` with any user, or `Bearer`.
    pub fn proxy_ok(&self, head: &[u8]) -> bool {
        let Some(token) = &self.proxy else { return true };
        header_value(head, "proxy-authorization").is_some_and(|v| credential_matches(v, token))
    }

    pub fn worker_ok(&self, head: &[u8]) -> bool {
        self.proxy.as_ref().is_some_and(|t| bearer_matches(head, t))
    }

    pub fn control_ok(&self, head: &[u8]) -> bool {
        self.control.as_ref().is_some_and(|t| bearer_matches(head, t))
    }
}

fn read_token(path: &Path) -> anyhow::Result<String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading token file {}: {e}", path.display()))?;
    let token = raw.trim();
    anyhow::ensure!(
        token.len() >= MIN_TOKEN_LEN && token.bytes().all(|b| b.is_ascii_graphic()),
        "token file {} holds no usable token",
        path.display()
    );
    Ok(token.to_owned())
}

fn bearer_matches(head: &[u8], token: &str) -> bool {
    header_value(head, "authorization")
        .and_then(|v| strip_scheme(v, "bearer"))
        .is_some_and(|v| constant_time_eq(v, token.as_bytes()))
}

fn credential_matches(value: &[u8], token: &str) -> bool {
    if let Some(bearer) = strip_scheme(value, "bearer") {
        return constant_time_eq(bearer, token.as_bytes());
    }
    let Some(encoded) = strip_scheme(value, "basic") else { return false };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    let password =
        decoded.iter().position(|&b| b == b':').map_or(&decoded[..0], |i| &decoded[i + 1..]);
    constant_time_eq(password, token.as_bytes())
}

fn strip_scheme<'a>(value: &'a [u8], scheme: &str) -> Option<&'a [u8]> {
    let n = scheme.len();
    let (name, rest) = value.split_at_checked(n)?;
    let rest = rest.strip_prefix(b" ")?;
    name.eq_ignore_ascii_case(scheme.as_bytes()).then_some(rest.trim_ascii())
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn strip_proxy_authorization(raw: &[u8]) -> Vec<u8> {
    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { return raw.to_vec() };
    let (head, rest) = raw.split_at(end + 2);
    let mut out = Vec::with_capacity(raw.len());
    for line in head.split_inclusive(|&b| b == b'\n') {
        let name = line.split(|&b| b == b':').next().unwrap_or_default().trim_ascii();
        if !name.eq_ignore_ascii_case(b"proxy-authorization") {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn head(header: &str) -> Vec<u8> {
        format!("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n{header}\r\n\r\n")
            .into_bytes()
    }

    fn basic(user: &str, pass: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Proxy-Authorization: Basic {encoded}")
    }

    #[test]
    fn no_proxy_token_checks_nothing() {
        let auth = GuardAuth::default();
        assert!(auth.proxy_ok(&head("X-A: b")));
        assert!(!auth.worker_ok(&head(&format!("Authorization: Bearer {TOKEN}"))));
        assert!(!auth.control_ok(&head(&format!("Authorization: Bearer {TOKEN}"))));
    }

    #[test]
    fn the_proxy_token_is_required_and_a_wrong_one_is_refused() {
        let auth = GuardAuth::with_tokens(Some(TOKEN), None);
        assert!(!auth.proxy_ok(&head("X-A: b")));
        assert!(auth.proxy_ok(&head(&basic("cctui", TOKEN))));
        assert!(auth.proxy_ok(&head(&basic("", TOKEN))));
        assert!(auth.proxy_ok(&head(&format!("proxy-authorization: bearer {TOKEN}"))));
        assert!(!auth.proxy_ok(&head(&basic("cctui", "0123456789abcdef0123456789abcdee"))));
        assert!(!auth.proxy_ok(&head(&basic(TOKEN, ""))));
        assert!(!auth.proxy_ok(&head("Proxy-Authorization: Basic !!notbase64")));
        assert!(!auth.proxy_ok(&head(&format!("Authorization: Bearer {TOKEN}"))));
    }

    #[test]
    fn the_control_token_is_distinct_from_the_proxy_token() {
        let control = "fedcba9876543210fedcba9876543210";
        let auth = GuardAuth::with_tokens(Some(TOKEN), Some(control));
        assert!(auth.control_ok(&head(&format!("Authorization: Bearer {control}"))));
        assert!(!auth.control_ok(&head(&format!("Authorization: Bearer {TOKEN}"))));
        assert!(auth.worker_ok(&head(&format!("Authorization: Bearer {TOKEN}"))));
        assert!(!auth.worker_ok(&head(&format!("Authorization: Bearer {control}"))));
    }

    #[test]
    fn token_files_must_hold_a_usable_token() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good");
        std::fs::write(&good, format!("{TOKEN}\n")).unwrap();
        let short = dir.path().join("short");
        std::fs::write(&short, "abc").unwrap();
        let auth = GuardAuth::load(Some(&good), None).unwrap();
        assert_eq!(auth.proxy_token(), Some(TOKEN));
        assert!(GuardAuth::load(Some(&short), None).is_err());
        assert!(GuardAuth::load(None, Some(&dir.path().join("absent"))).is_err());
        assert!(GuardAuth::load(None, None).unwrap().proxy_token().is_none());
    }

    #[test]
    fn proxy_authorization_never_reaches_the_upstream() {
        let tail = "Accept: */*\r\n\r\nbody Proxy-Authorization: keep";
        let auth = basic("cctui", TOKEN);
        let raw = format!("GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\n{auth}\r\n{tail}");
        let out = String::from_utf8(strip_proxy_authorization(raw.as_bytes())).unwrap();
        assert_eq!(out, format!("GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\n{tail}"));
        assert!(!out.contains(TOKEN));
    }
}
