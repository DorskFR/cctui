//! Signed, short-lived, single-use tickets that turn into the preview cookie.
//!
//! Both are `base64url(payload).base64url(hmac)` with a key derived from the
//! vault key. A ticket carries `preview_id|user_id|exp|nonce` and is burnt
//! on first use; the cookie carries `preview_id|user_id|iat` and lives as
//! long as the preview does.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

pub const TICKET_TTL_SECS: i64 = 60;
pub const COOKIE_NAME: &str = "cctui_preview";
const TICKET_TAG: &str = "t1";
const COOKIE_TAG: &str = "c1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    Malformed,
    BadSignature,
    Expired,
    Reused,
    WrongPreview,
    WrongUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub preview_id: String,
    pub user_id: Uuid,
}

pub struct Signer {
    key: Vec<u8>,
    used: Mutex<HashMap<String, i64>>,
}

impl Signer {
    #[must_use]
    pub fn new(root: &[u8]) -> Self {
        let mut mac = Hmac::<Sha256>::new_from_slice(root).expect("hmac accepts any key length");
        mac.update(b"cctui-preview-ticket");
        Self { key: mac.finalize().into_bytes().to_vec(), used: Mutex::new(HashMap::new()) }
    }

    fn sign(&self, payload: &str) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("hmac accepts any key length");
        mac.update(payload.as_bytes());
        format!("{}.{}", B64.encode(payload), B64.encode(mac.finalize().into_bytes()))
    }

    fn verify(&self, token: &str) -> Result<String, Reject> {
        let (payload, sig) = token.split_once('.').ok_or(Reject::Malformed)?;
        let payload = B64.decode(payload).map_err(|_| Reject::Malformed)?;
        let sig = B64.decode(sig).map_err(|_| Reject::Malformed)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("hmac accepts any key length");
        mac.update(&payload);
        mac.verify_slice(&sig).map_err(|_| Reject::BadSignature)?;
        String::from_utf8(payload).map_err(|_| Reject::Malformed)
    }

    #[must_use]
    pub fn mint_ticket(&self, preview_id: &str, user_id: Uuid) -> String {
        self.mint_ticket_at(preview_id, user_id, chrono::Utc::now().timestamp())
    }

    fn mint_ticket_at(&self, preview_id: &str, user_id: Uuid, now: i64) -> String {
        let nonce = Uuid::new_v4().simple();
        let exp = now + TICKET_TTL_SECS;
        self.sign(&format!("{TICKET_TAG}|{preview_id}|{user_id}|{exp}|{nonce}"))
    }

    /// Validate and burn a ticket for `preview_id`.
    pub fn redeem_ticket(&self, ticket: &str, preview_id: &str) -> Result<Grant, Reject> {
        self.redeem_ticket_at(ticket, preview_id, chrono::Utc::now().timestamp())
    }

    fn redeem_ticket_at(&self, ticket: &str, preview_id: &str, now: i64) -> Result<Grant, Reject> {
        let payload = self.verify(ticket)?;
        let parts: Vec<&str> = payload.split('|').collect();
        let [TICKET_TAG, pid, user, exp, nonce] = parts.as_slice() else {
            return Err(Reject::Malformed);
        };
        let exp: i64 = exp.parse().map_err(|_| Reject::Malformed)?;
        let user_id = Uuid::parse_str(user).map_err(|_| Reject::Malformed)?;
        if exp < now {
            return Err(Reject::Expired);
        }
        if *pid != preview_id {
            return Err(Reject::WrongPreview);
        }
        let reused = {
            let mut burnt = self.used.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            burnt.retain(|_, until| *until >= now);
            burnt.insert((*nonce).to_owned(), exp).is_some()
        };
        if reused {
            return Err(Reject::Reused);
        }
        Ok(Grant { preview_id: (*pid).to_owned(), user_id })
    }

    #[must_use]
    pub fn mint_cookie(&self, grant: &Grant) -> String {
        let iat = chrono::Utc::now().timestamp();
        self.sign(&format!("{COOKIE_TAG}|{}|{}|{iat}", grant.preview_id, grant.user_id))
    }

    /// Check a cookie against the preview it is presented to and its owner.
    pub fn check_cookie(
        &self,
        cookie: &str,
        preview_id: &str,
        owner: Uuid,
    ) -> Result<Grant, Reject> {
        let payload = self.verify(cookie)?;
        let parts: Vec<&str> = payload.split('|').collect();
        let [COOKIE_TAG, pid, user, _iat] = parts.as_slice() else {
            return Err(Reject::Malformed);
        };
        let user_id = Uuid::parse_str(user).map_err(|_| Reject::Malformed)?;
        if *pid != preview_id {
            return Err(Reject::WrongPreview);
        }
        if user_id != owner {
            return Err(Reject::WrongUser);
        }
        Ok(Grant { preview_id: (*pid).to_owned(), user_id })
    }
}

/// The `cctui_preview` pair from a `Cookie` header, if any.
#[must_use]
pub fn cookie_value(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.trim().to_owned())
}

/// Host-only (no `Domain`), so it never leaks to sibling preview hosts or
/// to cctui itself.
#[must_use]
pub fn set_cookie(value: &str, https: bool) -> String {
    let secure = if https { "; Secure" } else { "" };
    format!("{COOKIE_NAME}={value}; HttpOnly{secure}; SameSite=Lax; Path=/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PID: &str = "abcdefghijklmnop";

    #[test]
    fn ticket_round_trips_once_and_binds_preview_and_expiry() {
        let signer = Signer::new(b"root");
        let user = Uuid::new_v4();
        let ticket = signer.mint_ticket_at(PID, user, 1_000);
        let grant = signer.redeem_ticket_at(&ticket, PID, 1_010).unwrap();
        assert_eq!(grant, Grant { preview_id: PID.into(), user_id: user });
        assert_eq!(signer.redeem_ticket_at(&ticket, PID, 1_010), Err(Reject::Reused));

        let late = signer.mint_ticket_at(PID, user, 1_000);
        assert_eq!(
            signer.redeem_ticket_at(&late, PID, 1_000 + TICKET_TTL_SECS + 1),
            Err(Reject::Expired)
        );
        let elsewhere = signer.mint_ticket_at(PID, user, 1_000);
        assert_eq!(
            signer.redeem_ticket_at(&elsewhere, "zzzzzzzzzzzzzzzz", 1_001),
            Err(Reject::WrongPreview)
        );

        let other_key = Signer::new(b"other");
        let forged = other_key.mint_ticket_at(PID, user, 1_000);
        assert_eq!(signer.redeem_ticket_at(&forged, PID, 1_001), Err(Reject::BadSignature));
        assert_eq!(signer.redeem_ticket_at("garbage", PID, 1_001), Err(Reject::Malformed));
        assert_eq!(
            signer.redeem_ticket_at("Z2FyYmFnZQ.Z2FyYmFnZQ", PID, 1_001),
            Err(Reject::BadSignature)
        );
    }

    #[test]
    fn cookie_is_bound_to_preview_and_user_and_is_not_a_ticket() {
        let signer = Signer::new(b"root");
        let user = Uuid::new_v4();
        let grant = Grant { preview_id: PID.into(), user_id: user };
        let cookie = signer.mint_cookie(&grant);
        assert_eq!(signer.check_cookie(&cookie, PID, user).unwrap(), grant);
        assert_eq!(
            signer.check_cookie(&cookie, "zzzzzzzzzzzzzzzz", user),
            Err(Reject::WrongPreview)
        );
        assert_eq!(signer.check_cookie(&cookie, PID, Uuid::new_v4()), Err(Reject::WrongUser));
        assert_eq!(signer.redeem_ticket_at(&cookie, PID, 1), Err(Reject::Malformed));
        let ticket = signer.mint_ticket_at(PID, user, 1);
        assert_eq!(signer.check_cookie(&ticket, PID, user), Err(Reject::Malformed));
    }

    #[test]
    fn cookie_header_parsing_and_attributes() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "a=1; cctui_preview=tok.sig ; cctui_auth=x".parse().unwrap(),
        );
        assert_eq!(cookie_value(&headers).as_deref(), Some("tok.sig"));
        assert_eq!(cookie_value(&http::HeaderMap::new()), None);
        let secure = set_cookie("v", true);
        assert_eq!(secure, "cctui_preview=v; HttpOnly; Secure; SameSite=Lax; Path=/");
        assert!(!secure.contains("Domain"));
        assert!(!set_cookie("v", false).contains("Secure"));
    }
}
