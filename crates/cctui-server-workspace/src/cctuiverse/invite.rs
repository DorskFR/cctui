//! `<external_url>/cctuiverse/join#v1.<link>.<token>.<fp>` and the safety code.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const JOIN_PATH: &str = "/cctuiverse/join";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// The inviter's base URL, everything before [`JOIN_PATH`].
    pub base_url: String,
    pub link_id: Uuid,
    pub token: [u8; 32],
    pub fingerprint: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    Url,
    Scheme,
    UserInfo,
    Path,
    Fragment,
}

#[must_use]
pub fn fingerprint(public_key: &[u8]) -> [u8; 16] {
    Sha256::digest(public_key)[..16].try_into().expect("16 bytes")
}

#[must_use]
pub fn format(external_url: &str, link_id: Uuid, token: &[u8; 32], public_key: &[u8]) -> String {
    format!(
        "{}{JOIN_PATH}#v1.{link_id}.{}.{}",
        external_url.trim_end_matches('/'),
        URL_SAFE_NO_PAD.encode(token),
        URL_SAFE_NO_PAD.encode(fingerprint(public_key)),
    )
}

fn b64<const N: usize>(raw: &str) -> Option<[u8; N]> {
    URL_SAFE_NO_PAD.decode(raw).ok()?.try_into().ok()
}

pub fn parse(raw: &str) -> Result<Invite, InviteError> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| InviteError::Url)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(InviteError::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(InviteError::UserInfo);
    }
    let host = url.host_str().ok_or(InviteError::Url)?;
    if url.query().is_some() {
        return Err(InviteError::Path);
    }
    let prefix = url.path().strip_suffix(JOIN_PATH).ok_or(InviteError::Path)?;
    let mut parts = url.fragment().ok_or(InviteError::Fragment)?.split('.');
    let (Some("v1"), Some(link), Some(token), Some(fp), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(InviteError::Fragment);
    };
    let link_id = Uuid::parse_str(link).map_err(|_| InviteError::Fragment)?;
    let token = b64::<32>(token).ok_or(InviteError::Fragment)?;
    let fingerprint = b64::<16>(fp).ok_or(InviteError::Fragment)?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(Invite {
        base_url: format!("{}://{host}{port}{prefix}", url.scheme()),
        link_id,
        token,
        fingerprint,
    })
}

/// What both owners can read aloud to each other: the same on both sides.
#[must_use]
pub fn safety_code(a: &[u8], b: &[u8]) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut h = Sha256::new();
    h.update(lo);
    h.update(hi);
    let hex = hex::encode(&h.finalize()[..8]);
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("hex"))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: Uuid = Uuid::from_u128(0xabcd);

    fn sample(base: &str) -> String {
        format(base, LINK, &[7u8; 32], &[9u8; 32])
    }

    #[test]
    fn format_then_parse_round_trips() {
        for base in [
            "https://a.example",
            "https://a.example/",
            "https://a.example:8443/cctui",
            "http://10.0.0.5:8700",
            "https://[2001:db8::1]:8443",
        ] {
            let raw = sample(base);
            assert!(raw.contains("/cctuiverse/join#v1."), "{raw}");
            let inv = parse(&raw).unwrap();
            assert_eq!(inv.base_url, base.trim_end_matches('/'), "{raw}");
            assert_eq!(inv.link_id, LINK);
            assert_eq!(inv.token, [7u8; 32]);
            assert_eq!(inv.fingerprint, fingerprint(&[9u8; 32]));
        }
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        assert!(parse(&format!("  {}\n", sample("https://a.example"))).is_ok());
    }

    #[test]
    fn malformed_invites_are_refused() {
        let good = sample("https://a.example");
        let (head, frag) = good.split_once('#').unwrap();
        let parts: Vec<&str> = frag.split('.').collect();
        let cases = [
            ("not a url".to_owned(), InviteError::Url),
            (good.replace("https://", "ftp://"), InviteError::Scheme),
            (good.replace("https://", "file://"), InviteError::Scheme),
            (good.replace("https://", "https://user:pw@"), InviteError::UserInfo),
            (good.replace("https://", "https://user@"), InviteError::UserInfo),
            (good.replace("/cctuiverse/join", "/cctuiverse/joins"), InviteError::Path),
            (good.replace("/cctuiverse/join#", "/cctuiverse/join?x=1#"), InviteError::Path),
            (head.to_owned(), InviteError::Fragment),
            (format!("{head}#v2.{}.{}.{}", parts[1], parts[2], parts[3]), InviteError::Fragment),
            (format!("{head}#v1.{}.{}", parts[1], parts[2]), InviteError::Fragment),
            (format!("{head}#v1.{}.{}.{}.x", parts[1], parts[2], parts[3]), InviteError::Fragment),
            (format!("{head}#v1.nope.{}.{}", parts[2], parts[3]), InviteError::Fragment),
            (format!("{head}#v1.{}.!!!.{}", parts[1], parts[3]), InviteError::Fragment),
            (format!("{head}#v1.{}.{}.{}", parts[1], parts[3], parts[3]), InviteError::Fragment),
            (format!("{head}#v1.{}.{}.{}", parts[1], parts[2], parts[2]), InviteError::Fragment),
        ];
        for (raw, want) in cases {
            assert_eq!(parse(&raw), Err(want), "{raw}");
        }
    }

    #[test]
    fn the_safety_code_is_symmetric_and_grouped() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let code = safety_code(&a, &b);
        assert_eq!(code, safety_code(&b, &a));
        assert_eq!(code.len(), 19);
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|g| g.len() == 4 && g.bytes().all(|c| c.is_ascii_hexdigit())));
        assert_ne!(code, safety_code(&a, &[3u8; 32]));
        let mut h = Sha256::new();
        h.update(a);
        h.update(b);
        let want = hex::encode(&h.finalize()[..8]);
        assert_eq!(code.replace('-', ""), want);
    }
}
