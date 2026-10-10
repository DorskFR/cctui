//! The RFC 9421 subset every server-to-server request carries: Ed25519 over
//! `@method`, `@path` and `content-digest`, with `created`, `nonce`, `keyid`
//! and `alg` parameters.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SecureRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const MAX_SKEW_SECS: i64 = 60;
const COMPONENTS: &str = r#"("@method" "@path" "content-digest")"#;
const LABEL: &str = "sig1";

/// A link's Ed25519 seed. Overwritten on drop.
pub struct Seed([u8; 32]);

impl Seed {
    #[must_use]
    pub fn generate() -> Self {
        let mut s = [0u8; 32];
        ring::rand::SystemRandom::new().fill(&mut s).expect("system rng");
        Self(s)
    }

    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Some(Self(b.try_into().ok()?))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn pair(&self) -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&self.0).expect("32-byte seed")
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.pair().public_key().as_ref().try_into().expect("32-byte public key")
    }

    #[must_use]
    pub fn sign_bytes(&self, msg: &[u8]) -> Vec<u8> {
        self.pair().sign(msg).as_ref().to_vec()
    }
}

impl Drop for Seed {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new().fill(&mut b).expect("system rng");
    b
}

#[must_use]
pub fn random_b64url<const N: usize>() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>())
}

#[must_use]
pub fn content_digest(body: &[u8]) -> String {
    format!("sha-256=:{}:", STANDARD.encode(Sha256::digest(body)))
}

#[must_use]
pub fn signature_params(created: i64, nonce: &str, keyid: Uuid) -> String {
    format!(r#"{COMPONENTS};created={created};nonce="{nonce}";keyid="{keyid}";alg="ed25519""#)
}

/// RFC 9421 §2.5 signature base.
#[must_use]
pub fn signature_base(method: &str, path: &str, digest: &str, params: &str) -> String {
    format!(
        "\"@method\": {}\n\"@path\": {path}\n\"content-digest\": {digest}\n\"@signature-params\": {params}",
        method.to_ascii_uppercase()
    )
}

/// The three headers a signed request carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    pub content_digest: String,
    pub signature_input: String,
    pub signature: String,
}

#[must_use]
pub fn sign(
    seed: &Seed,
    method: &str,
    path: &str,
    body: &[u8],
    keyid: Uuid,
    created: i64,
    nonce: &str,
) -> Signed {
    let digest = content_digest(body);
    let params = signature_params(created, nonce, keyid);
    let base = signature_base(method, path, &digest, &params);
    let sig = seed.sign_bytes(base.as_bytes());
    Signed {
        content_digest: digest,
        signature_input: format!("{LABEL}={params}"),
        signature: format!("{LABEL}=:{}:", STANDARD.encode(sig)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigError {
    Malformed,
    Digest,
    Skew,
    KeyId,
    Bad,
}

struct Parsed {
    params: String,
    created: i64,
    nonce: String,
    keyid: Uuid,
}

fn quoted(v: &str) -> Option<&str> {
    v.strip_prefix('"')?.strip_suffix('"').filter(|s| !s.contains('"'))
}

fn nonce_ok(n: &str) -> bool {
    (16..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn parse_input(raw: &str) -> Option<Parsed> {
    let params = raw.trim().strip_prefix(LABEL)?.strip_prefix('=')?;
    let rest = params.strip_prefix(COMPONENTS)?;
    let (mut created, mut nonce, mut keyid, mut alg) = (None, None, None, None);
    for kv in rest.split(';').skip(1) {
        let (k, v) = kv.split_once('=')?;
        let slot = match k {
            "created" => &mut created,
            "nonce" => &mut nonce,
            "keyid" => &mut keyid,
            "alg" => &mut alg,
            _ => return None,
        };
        if slot.replace(v).is_some() {
            return None;
        }
    }
    if !rest.starts_with(';') || quoted(alg?)? != "ed25519" {
        return None;
    }
    let nonce = quoted(nonce?)?;
    if !nonce_ok(nonce) {
        return None;
    }
    Some(Parsed {
        params: params.to_owned(),
        created: created?.parse().ok()?,
        nonce: nonce.to_owned(),
        keyid: Uuid::parse_str(quoted(keyid?)?).ok()?,
    })
}

fn parse_signature(raw: &str) -> Option<Vec<u8>> {
    let b64 = raw.trim().strip_prefix(LABEL)?.strip_prefix("=:")?.strip_suffix(':')?;
    STANDARD.decode(b64).ok().filter(|s| s.len() == 64)
}

/// Check one request against `public_key`, expecting `keyid`. Returns the
/// nonce, which the caller must record to refuse a replay.
pub fn verify(
    headers: &Signed,
    method: &str,
    path: &str,
    body: &[u8],
    now: i64,
    keyid: Uuid,
    public_key: &[u8],
) -> Result<String, SigError> {
    let parsed = parse_input(&headers.signature_input).ok_or(SigError::Malformed)?;
    let sig = parse_signature(&headers.signature).ok_or(SigError::Malformed)?;
    let digest = content_digest(body);
    if !crate::cctuiverse::ct_eq(digest.as_bytes(), headers.content_digest.trim().as_bytes()) {
        return Err(SigError::Digest);
    }
    if (now - parsed.created).abs() > MAX_SKEW_SECS {
        return Err(SigError::Skew);
    }
    if parsed.keyid != keyid {
        return Err(SigError::KeyId);
    }
    let base = signature_base(method, path, &digest, &parsed.params);
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(base.as_bytes(), &sig)
        .map_err(|_| SigError::Bad)?;
    Ok(parsed.nonce)
}

/// The `Signed` headers of an inbound request, if all three are present.
#[must_use]
pub fn from_headers(h: &axum::http::HeaderMap) -> Option<Signed> {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
    Some(Signed {
        content_digest: get("content-digest")?,
        signature_input: get("signature-input")?,
        signature: get("signature")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC8032_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    const RFC8032_PK: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    const RFC8032_SIG: &str = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065\
                               224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24\
                               655141438e7a100b";
    const KEYID: Uuid = Uuid::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888);
    const NONCE: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const PATH: &str = "/cctuiverse/v1/links/00000000-0000-0000-0000-000000000001/messages";

    fn seed() -> Seed {
        Seed::from_bytes(&hex::decode(RFC8032_SEED).unwrap()).unwrap()
    }

    #[test]
    fn ed25519_matches_rfc8032_test_1() {
        let s = seed();
        assert_eq!(hex::encode(s.public_key()), RFC8032_PK);
        assert_eq!(hex::encode(s.sign_bytes(b"")), RFC8032_SIG);
    }

    #[test]
    fn signature_base_is_the_rfc9421_serialisation() {
        let params = signature_params(1_700_000_000, NONCE, KEYID);
        assert_eq!(
            params,
            "(\"@method\" \"@path\" \"content-digest\");created=1700000000;\
             nonce=\"AAAAAAAAAAAAAAAAAAAAAA\";keyid=\"11112222-3333-4444-5555-666677778888\";\
             alg=\"ed25519\""
        );
        let digest = content_digest(b"{}");
        assert_eq!(digest, "sha-256=:RBNvo1WzZ4oRRq0W9+hknpT7T8If536DEMBg9hyq/4o=:");
        assert_eq!(
            signature_base("post", PATH, &digest, &params),
            format!(
                "\"@method\": POST\n\"@path\": {PATH}\n\"content-digest\": {digest}\n\
                 \"@signature-params\": {params}"
            )
        );
    }

    #[test]
    fn a_fixed_request_signs_deterministically_and_verifies() {
        let s = seed();
        let a = sign(&s, "POST", PATH, b"{}", KEYID, 1_700_000_000, NONCE);
        let b = sign(&s, "POST", PATH, b"{}", KEYID, 1_700_000_000, NONCE);
        assert_eq!(a, b);
        assert_eq!(
            a.signature,
            "sig1=:t5/IHyPI1bezoDp1enNk9mPKL2bZlQWkO/H430OD8C4su7+/YIpUzfrs2MOWUWfzTrNU1CBLe1YHE+YuOK6nBA==:"
        );
        assert!(a.signature_input.starts_with("sig1=(\"@method\""));
        assert!(a.signature.starts_with("sig1=:") && a.signature.ends_with(':'));
        let v = verify(&a, "POST", PATH, b"{}", 1_700_000_030, KEYID, &s.public_key()).unwrap();
        assert_eq!(v, NONCE);
    }

    #[test]
    fn round_trip_with_a_fresh_key() {
        let s = Seed::generate();
        let nonce = random_b64url::<16>();
        let h = sign(&s, "POST", PATH, b"hello", KEYID, 42, &nonce);
        assert!(verify(&h, "POST", PATH, b"hello", 42, KEYID, &s.public_key()).is_ok());
    }

    #[test]
    fn every_tampered_component_is_refused() {
        let s = seed();
        let pk = s.public_key();
        let h = sign(&s, "POST", PATH, b"body", KEYID, 1000, NONCE);
        let ok = |h: &Signed, m: &str, p: &str, b: &[u8], now: i64, k: Uuid, pk: &[u8]| {
            verify(h, m, p, b, now, k, pk)
        };
        assert!(ok(&h, "POST", PATH, b"body", 1000, KEYID, &pk).is_ok());
        assert_eq!(ok(&h, "PUT", PATH, b"body", 1000, KEYID, &pk), Err(SigError::Bad));
        assert_eq!(ok(&h, "POST", "/other", b"body", 1000, KEYID, &pk), Err(SigError::Bad));
        assert_eq!(ok(&h, "POST", PATH, b"bodY", 1000, KEYID, &pk), Err(SigError::Digest));
        assert_eq!(ok(&h, "POST", PATH, b"body", 1061, KEYID, &pk), Err(SigError::Skew));
        assert_eq!(ok(&h, "POST", PATH, b"body", 939, KEYID, &pk), Err(SigError::Skew));
        assert!(ok(&h, "POST", PATH, b"body", 1060, KEYID, &pk).is_ok());
        assert_eq!(ok(&h, "POST", PATH, b"body", 1000, Uuid::nil(), &pk), Err(SigError::KeyId));
        let other = Seed::generate().public_key();
        assert_eq!(ok(&h, "POST", PATH, b"body", 1000, KEYID, &other), Err(SigError::Bad));

        let mut forged_digest = h.clone();
        forged_digest.content_digest = content_digest(b"evil");
        assert_eq!(ok(&forged_digest, "POST", PATH, b"evil", 1000, KEYID, &pk), Err(SigError::Bad));

        let mut later = h.clone();
        later.signature_input = later.signature_input.replace("created=1000", "created=2000");
        assert_eq!(ok(&later, "POST", PATH, b"body", 2000, KEYID, &pk), Err(SigError::Bad));
    }

    #[test]
    fn malformed_inputs_are_refused() {
        let s = seed();
        let pk = s.public_key();
        let good = sign(&s, "POST", PATH, b"", KEYID, 5, NONCE);
        let with_input = |input: String| Signed { signature_input: input, ..good.clone() };
        for bad in [
            good.signature_input.replace("sig1=", "sig2="),
            good.signature_input.replace("\"content-digest\"", ""),
            good.signature_input.replace("alg=\"ed25519\"", "alg=\"hmac-sha256\""),
            good.signature_input.replace(";alg=\"ed25519\"", ""),
            format!("{};created=5", good.signature_input),
            good.signature_input.replace(NONCE, "short"),
            good.signature_input.replace(NONCE, "AAAAAAAAAAAAAAAAAAAA\\\"x"),
            good.signature_input.replace("created=5", "created=x"),
        ] {
            assert_eq!(
                verify(&with_input(bad.clone()), "POST", PATH, b"", 5, KEYID, &pk),
                Err(SigError::Malformed),
                "{bad}"
            );
        }
        let truncated = Signed { signature: "sig1=:AAAA:".into(), ..good.clone() };
        assert_eq!(verify(&truncated, "POST", PATH, b"", 5, KEYID, &pk), Err(SigError::Malformed));
    }
}
