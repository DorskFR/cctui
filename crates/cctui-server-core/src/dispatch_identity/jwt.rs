use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cctui_proto::dispatch_identity::AUDIENCE;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents, UnparsedPublicKey,
};
use serde::Deserialize;

const LEEWAY_SECS: i64 = 30;
/// The guard-proxy token asks for 600s; anything far longer is not that token.
const MAX_LIFETIME_SECS: i64 = 3600;
const POD_SUFFIX_LEN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Malformed(&'static str),
    UnsupportedAlgorithm(String),
    UntrustedIssuer(String),
    NoKey { issuer: String, kid: Option<String> },
    Keys { issuer: String, error: String },
    BadSignature,
    WrongAudience,
    Expired,
    NotYetValid,
    LifetimeTooLong(i64),
    NotAPod,
    WrongNamespace { want: String, got: String },
    WrongPod { job: String, pod: String },
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(what) => write!(f, "malformed token: {what}"),
            Self::UnsupportedAlgorithm(alg) => {
                write!(f, "token algorithm `{alg}` is not accepted (RS256, ES256)")
            }
            Self::UntrustedIssuer(iss) => write!(
                f,
                "token issuer `{iss}` is not a trusted cluster issuer (add it to \
                 CCTUI_DISPATCH_TRUSTED_ISSUERS on the server)"
            ),
            Self::NoKey { issuer, kid } => write!(
                f,
                "no signing key `{}` in the JWKS of `{issuer}`",
                kid.as_deref().unwrap_or("<none>")
            ),
            Self::Keys { issuer, error } => write!(f, "reading the JWKS of `{issuer}`: {error}"),
            Self::BadSignature => f.write_str("token signature does not verify"),
            Self::WrongAudience => write!(f, "token audience is not `{AUDIENCE}`"),
            Self::Expired => f.write_str("token has expired"),
            Self::NotYetValid => f.write_str("token is not valid yet"),
            Self::LifetimeTooLong(secs) => {
                write!(f, "token lifetime {secs}s exceeds {MAX_LIFETIME_SECS}s")
            }
            Self::NotAPod => f.write_str("token is not bound to a pod"),
            Self::WrongNamespace { want, got } => {
                write!(f, "pod namespace `{got}` is not the dispatch's `{want}`")
            }
            Self::WrongPod { job, pod } => write!(f, "pod `{pod}` does not belong to Job `{job}`"),
        }
    }
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, want: &str) -> bool {
        match self {
            Self::One(a) => a == want,
            Self::Many(all) => all.iter().any(|a| a == want),
        }
    }
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    exp: i64,
    #[serde(default)]
    nbf: Option<i64>,
    #[serde(default)]
    iat: Option<i64>,
    #[serde(rename = "kubernetes.io", default)]
    kubernetes: Option<KubeClaims>,
}

#[derive(Deserialize)]
struct KubeClaims {
    namespace: String,
    #[serde(default)]
    pod: Option<ObjectRef>,
    #[serde(default)]
    serviceaccount: Option<ObjectRef>,
}

#[derive(Deserialize)]
struct ObjectRef {
    name: String,
    uid: String,
}

pub struct Unverified {
    header: Header,
    claims: Claims,
    signing_input: String,
    signature: Vec<u8>,
}

impl Unverified {
    pub fn issuer(&self) -> &str {
        &self.claims.iss
    }

    pub fn kid(&self) -> Option<&str> {
        self.header.kid.as_deref()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Jwks {
    #[serde(default)]
    pub keys: Vec<Jwk>,
}

impl Jwks {
    pub fn has(&self, kid: &str) -> bool {
        self.keys.iter().any(|k| k.kid.as_deref() == Some(kid))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodIdentity {
    pub issuer: String,
    pub namespace: String,
    pub pod_name: String,
    pub pod_uid: String,
    pub service_account: Option<String>,
}

fn decode_part(part: &str, what: &'static str) -> Result<Vec<u8>, Rejection> {
    URL_SAFE_NO_PAD.decode(part).map_err(|_| Rejection::Malformed(what))
}

pub fn parse(token: &str) -> Result<Unverified, Rejection> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    let [h, c, sig] = parts[..] else {
        return Err(Rejection::Malformed("not a three-part JWS"));
    };
    let header: Header = serde_json::from_slice(&decode_part(h, "header encoding")?)
        .map_err(|_| Rejection::Malformed("header"))?;
    let claims: Claims = serde_json::from_slice(&decode_part(c, "claims encoding")?)
        .map_err(|_| Rejection::Malformed("claims"))?;
    Ok(Unverified {
        header,
        claims,
        signing_input: format!("{h}.{c}"),
        signature: decode_part(sig, "signature encoding")?,
    })
}

fn verifies(alg: &str, key: &Jwk, message: &[u8], signature: &[u8]) -> bool {
    let b64 = |v: Option<&str>| v.and_then(|s| URL_SAFE_NO_PAD.decode(s).ok());
    match (alg, key.kty.as_str()) {
        ("RS256", "RSA") => {
            let (Some(n), Some(e)) = (b64(key.n.as_deref()), b64(key.e.as_deref())) else {
                return false;
            };
            RsaPublicKeyComponents { n, e }
                .verify(&RSA_PKCS1_2048_8192_SHA256, message, signature)
                .is_ok()
        }
        ("ES256", "EC") if key.crv.as_deref() == Some("P-256") => {
            let (Some(x), Some(y)) = (b64(key.x.as_deref()), b64(key.y.as_deref())) else {
                return false;
            };
            if x.len() != 32 || y.len() != 32 {
                return false;
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                .verify(message, signature)
                .is_ok()
        }
        _ => false,
    }
}

fn check_signature(token: &Unverified, jwks: &Jwks) -> Result<(), Rejection> {
    let alg = token.header.alg.as_str();
    if !matches!(alg, "RS256" | "ES256") {
        return Err(Rejection::UnsupportedAlgorithm(alg.to_owned()));
    }
    let kid = token.kid();
    let candidates: Vec<&Jwk> =
        jwks.keys.iter().filter(|k| kid.is_none() || k.kid.as_deref() == kid).collect();
    if candidates.is_empty() {
        return Err(Rejection::NoKey {
            issuer: token.claims.iss.clone(),
            kid: kid.map(ToOwned::to_owned),
        });
    }
    let message = token.signing_input.as_bytes();
    if candidates.iter().any(|k| verifies(alg, k, message, &token.signature)) {
        Ok(())
    } else {
        Err(Rejection::BadSignature)
    }
}

fn check_times(claims: &Claims, now: i64) -> Result<(), Rejection> {
    if claims.exp + LEEWAY_SECS <= now {
        return Err(Rejection::Expired);
    }
    if claims.nbf.is_some_and(|nbf| nbf - LEEWAY_SECS > now) {
        return Err(Rejection::NotYetValid);
    }
    let lifetime = claims.exp - claims.iat.unwrap_or(now);
    if lifetime > MAX_LIFETIME_SECS + LEEWAY_SECS {
        return Err(Rejection::LifetimeTooLong(lifetime));
    }
    Ok(())
}

/// Verify a projected service-account token against its issuer's keys and
/// return the pod it was minted for. The issuer must already be trusted.
pub fn verify(token: Unverified, jwks: &Jwks, now: i64) -> Result<PodIdentity, Rejection> {
    check_signature(&token, jwks)?;
    let claims = token.claims;
    if !claims.aud.contains(AUDIENCE) {
        return Err(Rejection::WrongAudience);
    }
    check_times(&claims, now)?;
    let kube = claims.kubernetes.ok_or(Rejection::NotAPod)?;
    let pod = kube.pod.ok_or(Rejection::NotAPod)?;
    Ok(PodIdentity {
        issuer: claims.iss,
        namespace: kube.namespace,
        pod_name: pod.name,
        pod_uid: pod.uid,
        service_account: kube.serviceaccount.map(|sa| sa.name),
    })
}

/// The pod is one the Job controller made for `job_name` in `namespace`:
/// `<job>-<5 random>`, the name a non-indexed Job gives its pods.
pub fn check_binding(id: &PodIdentity, namespace: &str, job_name: &str) -> Result<(), Rejection> {
    if id.namespace != namespace {
        return Err(Rejection::WrongNamespace {
            want: namespace.to_owned(),
            got: id.namespace.clone(),
        });
    }
    let owned =
        id.pod_name.strip_prefix(job_name).and_then(|rest| rest.strip_prefix('-')).is_some_and(
            |suffix| {
                suffix.len() == POD_SUFFIX_LEN
                    && suffix.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            },
        );
    if owned {
        Ok(())
    } else {
        Err(Rejection::WrongPod { job: job_name.to_owned(), pod: id.pod_name.clone() })
    }
}

#[cfg(test)]
pub mod test_keys {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};

    use super::{Jwk, Jwks};

    pub struct Signer {
        pub kid: String,
        key: EcdsaKeyPair,
        rng: SystemRandom,
    }

    impl Signer {
        pub fn new(kid: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .expect("generate P-256 test key");
            let key =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .expect("load P-256 test key");
            Self { kid: kid.to_owned(), key, rng }
        }

        pub fn jwk(&self) -> Jwk {
            let point = self.key.public_key().as_ref();
            Jwk {
                kty: "EC".to_owned(),
                kid: Some(self.kid.clone()),
                crv: Some("P-256".to_owned()),
                x: Some(URL_SAFE_NO_PAD.encode(&point[1..33])),
                y: Some(URL_SAFE_NO_PAD.encode(&point[33..65])),
                ..Jwk::default()
            }
        }

        pub fn jwks(&self) -> Jwks {
            Jwks { keys: vec![self.jwk()] }
        }

        pub fn sign(&self, claims: &serde_json::Value) -> String {
            let header = serde_json::json!({ "alg": "ES256", "kid": self.kid, "typ": "JWT" });
            let h = URL_SAFE_NO_PAD.encode(header.to_string());
            let c = URL_SAFE_NO_PAD.encode(claims.to_string());
            let input = format!("{h}.{c}");
            let sig = self.key.sign(&self.rng, input.as_bytes()).expect("sign test token");
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
        }
    }

    pub fn pod_claims(iss: &str, ns: &str, pod: &str, now: i64) -> serde_json::Value {
        serde_json::json!({
            "iss": iss,
            "aud": ["cctui"],
            "sub": format!("system:serviceaccount:{ns}:worker"),
            "iat": now,
            "nbf": now,
            "exp": now + 600,
            "kubernetes.io": {
                "namespace": ns,
                "pod": { "name": pod, "uid": "pod-uid-1" },
                "serviceaccount": { "name": "worker", "uid": "sa-uid-1" }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_keys::{Signer, pod_claims};
    use super::*;

    const ISS: &str = "https://oidc.example.test/id/cluster-a";
    const NOW: i64 = 1_800_000_000;
    const POD: &str = "cctui-worker-0123456789ab-x7k2p";
    const JOB: &str = "cctui-worker-0123456789ab";

    fn verify_with(
        signer: &Signer,
        jwks: &Jwks,
        claims: &serde_json::Value,
    ) -> Result<PodIdentity, Rejection> {
        verify(parse(&signer.sign(claims)).unwrap(), jwks, NOW)
    }

    #[test]
    fn a_pod_token_signed_by_the_issuer_key_verifies_to_its_pod() {
        let signer = Signer::new("k1");
        let id = verify_with(&signer, &signer.jwks(), &pod_claims(ISS, "cctui", POD, NOW)).unwrap();
        assert_eq!(id.issuer, ISS);
        assert_eq!(id.namespace, "cctui");
        assert_eq!(id.pod_name, POD);
        assert_eq!(id.pod_uid, "pod-uid-1");
        assert_eq!(id.service_account.as_deref(), Some("worker"));
        assert!(check_binding(&id, "cctui", JOB).is_ok());
    }

    #[test]
    fn a_token_signed_by_another_key_is_refused() {
        let signer = Signer::new("k1");
        let impostor = Signer::new("k1");
        let token = impostor.sign(&pod_claims(ISS, "cctui", POD, NOW));
        assert_eq!(
            verify(parse(&token).unwrap(), &signer.jwks(), NOW),
            Err(Rejection::BadSignature)
        );
    }

    #[test]
    fn a_tampered_claim_breaks_the_signature() {
        let signer = Signer::new("k1");
        let token = signer.sign(&pod_claims(ISS, "cctui", POD, NOW));
        let mut parts: Vec<String> = token.split('.').map(ToOwned::to_owned).collect();
        let forged = pod_claims(ISS, "cctui", "cctui-worker-ffffffffffff-aaaaa", NOW);
        parts[1] = URL_SAFE_NO_PAD.encode(forged.to_string());
        let token = parts.join(".");
        assert_eq!(
            verify(parse(&token).unwrap(), &signer.jwks(), NOW),
            Err(Rejection::BadSignature)
        );
    }

    #[test]
    fn an_unknown_kid_names_the_missing_key() {
        let signer = Signer::new("rotated");
        let other = Signer::new("current");
        let err = verify_with(&signer, &other.jwks(), &pod_claims(ISS, "cctui", POD, NOW));
        assert_eq!(
            err,
            Err(Rejection::NoKey { issuer: ISS.to_owned(), kid: Some("rotated".into()) })
        );
    }

    #[test]
    fn the_audience_must_be_cctui() {
        let signer = Signer::new("k1");
        let mut claims = pod_claims(ISS, "cctui", POD, NOW);
        claims["aud"] = serde_json::json!(["https://kubernetes.default.svc.cluster.local"]);
        assert_eq!(verify_with(&signer, &signer.jwks(), &claims), Err(Rejection::WrongAudience));
        claims["aud"] = serde_json::json!("cctui");
        assert!(verify_with(&signer, &signer.jwks(), &claims).is_ok(), "a string aud is accepted");
    }

    #[test]
    fn an_expired_or_premature_or_long_lived_token_is_refused() {
        let signer = Signer::new("k1");
        let jwks = signer.jwks();
        let expired = pod_claims(ISS, "cctui", POD, NOW - 700);
        assert_eq!(verify_with(&signer, &jwks, &expired), Err(Rejection::Expired));
        let premature = pod_claims(ISS, "cctui", POD, NOW + 120);
        assert_eq!(verify_with(&signer, &jwks, &premature), Err(Rejection::NotYetValid));
        let mut long = pod_claims(ISS, "cctui", POD, NOW);
        long["exp"] = serde_json::json!(NOW + 86_400);
        assert_eq!(verify_with(&signer, &jwks, &long), Err(Rejection::LifetimeTooLong(86_400)));
    }

    #[test]
    fn a_token_without_a_pod_binding_is_refused() {
        let signer = Signer::new("k1");
        let mut claims = pod_claims(ISS, "cctui", POD, NOW);
        claims["kubernetes.io"].as_object_mut().unwrap().remove("pod");
        assert_eq!(verify_with(&signer, &signer.jwks(), &claims), Err(Rejection::NotAPod));
    }

    #[test]
    fn only_rs256_and_es256_are_accepted() {
        let signer = Signer::new("k1");
        let token = signer.sign(&pod_claims(ISS, "cctui", POD, NOW));
        let mut parts: Vec<String> = token.split('.').map(ToOwned::to_owned).collect();
        parts[0] = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","kid":"k1"}"#);
        let token = parts.join(".");
        assert_eq!(
            verify(parse(&token).unwrap(), &signer.jwks(), NOW),
            Err(Rejection::UnsupportedAlgorithm("none".into()))
        );
    }

    #[test]
    fn a_pod_of_another_job_or_namespace_is_not_bound_to_the_dispatch() {
        let signer = Signer::new("k1");
        let other = verify_with(
            &signer,
            &signer.jwks(),
            &pod_claims(ISS, "cctui", "cctui-worker-ffffffffffff-x7k2p", NOW),
        )
        .unwrap();
        assert!(matches!(check_binding(&other, "cctui", JOB), Err(Rejection::WrongPod { .. })));
        let mine =
            verify_with(&signer, &signer.jwks(), &pod_claims(ISS, "cctui", POD, NOW)).unwrap();
        assert!(matches!(
            check_binding(&mine, "other-ns", JOB),
            Err(Rejection::WrongNamespace { .. })
        ));
        for pod in [
            JOB,
            "cctui-worker-0123456789ab-",
            "cctui-worker-0123456789ab-x7k2pz",
            "cctui-worker-0123456789ab-X7K2P",
        ] {
            let id = PodIdentity { pod_name: pod.to_owned(), ..mine.clone() };
            assert!(check_binding(&id, "cctui", JOB).is_err(), "{pod}");
        }
    }

    #[test]
    fn malformed_tokens_are_refused_before_any_key_lookup() {
        for token in ["", "a.b", "a.b.c.d", "!!.e30.sig"] {
            assert!(matches!(parse(token), Err(Rejection::Malformed(_))), "{token}");
        }
    }
}
