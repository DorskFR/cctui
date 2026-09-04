//! The WebAuthn relying party — the passkey half of browser auth.
//!
//! ## Why this exists
//!
//! The webui's only credential was a bearer token typed into a login box. A
//! passkey is a second door onto the *same* model: a successful assertion mints
//! an ordinary, expiring `auth_keys` row (kind `passkey`) whose token rides the
//! existing `HttpOnly` cookie. Nothing downstream of `auth_middleware` knows or
//! cares which door the request came through, and the token path is never
//! removed — a lost passkey is an inconvenience, not a lockout.
//!
//! ## Relying-party identity
//!
//! WebAuthn binds every credential to an **RP ID** (a domain) and validates the
//! ceremony against an **origin**. Both are derived from `CCTUI_EXTERNAL_URL`,
//! which a real deployment already sets to its public URL, so a correctly
//! configured server needs no new env. `CCTUI_RP_ID` overrides the derived host
//! for the one case deriving gets wrong: serving the UI from a subdomain of the
//! domain the credentials should be scoped to.
//!
//! Two consequences worth knowing before enrolling anything:
//!   * a credential enrolled against one RP ID does not work on another
//!     hostname (an IP, a tunnel, a second domain) — that is WebAuthn, not us;
//!   * the ceremony demands a secure context, so `https://` (or `localhost`).
//!
//! When `CCTUI_EXTERNAL_URL` is a bare `http://` host that is not `localhost`,
//! we refuse to build an RP rather than hand out challenges no browser will
//! honour: passkeys simply report themselves unavailable and the login box
//! behaves exactly as it did before.

// "WebAuthn" and the authenticator brand names below are proper nouns that trip
// clippy's camel-case doc heuristic throughout this module; none is a code item.
#![allow(clippy::doc_markdown)]

use webauthn_rs::prelude::*;
use webauthn_rs_proto::ResidentKeyRequirement;

/// Build the relying party from config, or `None` when this deployment can't
/// support passkeys (unparseable/insecure external URL). `None` is not an
/// error: every passkey route answers "unavailable" and the token login is
/// untouched.
pub fn build(external_url: &str, rp_id_override: Option<&str>) -> Option<Webauthn> {
    let origin = Url::parse(external_url.trim_end_matches('/'))
        .map_err(|e| tracing::warn!("passkeys off: CCTUI_EXTERNAL_URL is not a URL: {e}"))
        .ok()?;
    let host = origin.host_str()?.to_owned();
    let is_secure = origin.scheme() == "https" || host == "localhost" || host == "127.0.0.1";
    if !is_secure {
        tracing::info!(
            %origin,
            "passkeys off: WebAuthn needs a secure context (https, or localhost)"
        );
        return None;
    }
    let rp_id = rp_id_override.map_or(host, str::to_owned);
    WebauthnBuilder::new(&rp_id, &origin)
        .and_then(|b| b.rp_name("cctui").build())
        .map_err(|e| tracing::warn!("passkeys off: relying party rejected ({rp_id}): {e}"))
        .ok()
}

/// Turn the registration options webauthn-rs produces into ones that actually
/// yield a *discoverable* credential.
///
/// `start_passkey_registration` emits `residentKey: discouraged`, which is the
/// right default for a site that knows who is logging in. cctui's login screen
/// does not: it asks the browser to discover the credential and tells us which
/// user it belongs to, so the credential must be resident. Without this a
/// hardware key would enrol happily and then be invisible at login.
pub const fn require_resident_key(ccr: &mut CreationChallengeResponse) {
    if let Some(sel) = ccr.public_key.authenticator_selection.as_mut() {
        sel.resident_key = Some(ResidentKeyRequirement::Required);
        sel.require_resident_key = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_url_yields_a_relying_party() {
        assert!(build("https://cctui.example.com", None).is_some());
        // A trailing slash is the shape an operator most often pastes.
        assert!(build("https://cctui.example.com/", None).is_some());
    }

    #[test]
    fn localhost_is_a_secure_context_even_over_http() {
        assert!(build("http://localhost:8700", None).is_some());
    }

    #[test]
    fn an_ip_address_is_not_a_relying_party() {
        // A bare IP is a secure context for the browser but is not a valid RP
        // ID (which must be a domain), so passkeys stay off rather than
        // producing challenges the browser will refuse.
        assert!(build("http://127.0.0.1:8700", None).is_none());
        assert!(build("https://10.0.0.1", None).is_none());
    }

    #[test]
    fn plain_http_host_disables_passkeys_instead_of_half_working() {
        assert!(build("http://cctui.example.com", None).is_none());
        assert!(build("not a url", None).is_none());
    }

    #[test]
    fn rp_id_override_must_still_cover_the_origin() {
        // A parent domain of the origin host is the legitimate override.
        assert!(build("https://cctui.example.com", Some("example.com")).is_some());
        // An unrelated domain is rejected by the builder, not silently accepted.
        assert!(build("https://cctui.example.com", Some("elsewhere.test")).is_none());
    }

    #[test]
    fn registration_options_ask_for_a_resident_key() {
        let webauthn = build("https://cctui.example.com", None).unwrap();
        let (mut ccr, _state) = webauthn
            .start_passkey_registration(Uuid::new_v4(), "alice", "alice", None)
            .unwrap();
        require_resident_key(&mut ccr);
        let sel = ccr.public_key.authenticator_selection.unwrap();
        assert!(sel.require_resident_key);
        assert_eq!(sel.resident_key, Some(ResidentKeyRequirement::Required));
    }
}
