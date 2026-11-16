use std::sync::{Arc, OnceLock};

use aws_lc_rs::encoding::AsBigEndian;
use aws_lc_rs::error::Unspecified;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use aws_lc_rs::{aead, agreement, hkdf, rand};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sqlx::PgPool;

use super::channel::{Capabilities, Channel, Endpoint, Press, ProviderRef, Rendered, SendError};
use crate::config::Config;
use crate::outbound::{AllowedHost, OutboundUrlError, validate_outbound_url};

pub const ID: &str = "web_push";
pub const ENV_KEY: &str = "CCTUI_VAPID_PRIVATE_KEY";
const SETTINGS_KEY: &str = "web_push_vapid";
const FALLBACK_SUBJECT: &str = "mailto:cctui@localhost";
const RESOLVED_TITLE: &str = "Resolved";

const RECORD_SIZE: u32 = 4096;
const TTL_SECS: u32 = 86_400;
const JWT_LIFETIME_SECS: i64 = 12 * 3600;
const MAX_TITLE_CHARS: usize = 200;
const MAX_BODY_CHARS: usize = 1000;
const MAX_ACTIONS: usize = 2;
const P256_POINT_LEN: usize = 65;
const AUTH_SECRET_LEN: usize = 16;

const SELECT_KEY_SQL: &str = "SELECT value FROM instance_settings WHERE key = $1";
const INSERT_KEY_SQL: &str = "INSERT INTO instance_settings (key, value, updated_at) \
    VALUES ($1, $2, now()) ON CONFLICT (key) DO NOTHING";

const ACTION_PATH: &str = "/api/v1/notifications/actions/";
const CONFIRM_PATH: &str = "/n/";

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(crate::build_http_client)
}

fn decode(field: &str, raw: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(raw.trim().trim_end_matches('='))
        .map_err(|_| format!("{field} is not base64url"))
}

fn text<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, String> {
    value.and_then(Value::as_str).ok_or_else(|| format!("subscription has no {field}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub endpoint: String,
    p256dh: Vec<u8>,
    auth: Vec<u8>,
}

impl Subscription {
    pub fn parse(config: &Value) -> Result<Self, String> {
        let sub = config.get("subscription").unwrap_or(config);
        let endpoint = text(sub.get("endpoint"), "endpoint")?.to_owned();
        let keys = sub.get("keys");
        let p256dh =
            decode("keys.p256dh", text(keys.and_then(|k| k.get("p256dh")), "keys.p256dh")?)?;
        let auth = decode("keys.auth", text(keys.and_then(|k| k.get("auth")), "keys.auth")?)?;
        if p256dh.len() != P256_POINT_LEN || p256dh[0] != 0x04 {
            return Err("keys.p256dh must be an uncompressed P-256 point".into());
        }
        if auth.len() != AUTH_SECRET_LEN {
            return Err("keys.auth must be 16 bytes".into());
        }
        Ok(Self { endpoint, p256dh, auth })
    }
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf_into(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) -> Result<(), Unspecified> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    prk.expand(&[info], Len(out.len()))?.fill(out)
}

fn derive(
    secret: &[u8],
    to: &Subscription,
    sender_pub: &[u8],
    salt: &[u8],
) -> Result<([u8; 16], [u8; 12]), Unspecified> {
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(&to.p256dh);
    info.extend_from_slice(sender_pub);
    let mut ikm = [0u8; 32];
    hkdf_into(&to.auth, secret, &info, &mut ikm)?;
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    hkdf_into(salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek)?;
    hkdf_into(salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce)?;
    Ok((cek, nonce))
}

fn encrypt_with(
    plaintext: &[u8],
    to: &Subscription,
    sender: &agreement::PrivateKey,
    salt: [u8; 16],
) -> Result<Vec<u8>, Unspecified> {
    let sender_pub = sender.compute_public_key()?;
    let sender_pub = sender_pub.as_ref();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, to.p256dh.as_slice());
    let keys = |secret: &[u8]| derive(secret, to, sender_pub, &salt);
    let (cek, nonce) = agreement::agree(sender, peer, Unspecified, keys)?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek)?);
    let mut record = Vec::with_capacity(plaintext.len() + 17);
    record.extend_from_slice(plaintext);
    record.push(0x02);
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut record,
    )?;
    let id_len = u8::try_from(sender_pub.len()).map_err(|_| Unspecified)?;
    let mut body = Vec::with_capacity(21 + sender_pub.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(id_len);
    body.extend_from_slice(sender_pub);
    body.extend_from_slice(&record);
    Ok(body)
}

fn encrypt(plaintext: &[u8], to: &Subscription) -> Result<Vec<u8>, Unspecified> {
    let sender = agreement::PrivateKey::generate(&agreement::ECDH_P256)?;
    let mut salt = [0u8; 16];
    rand::fill(&mut salt)?;
    encrypt_with(plaintext, to, &sender, salt)
}

pub struct Vapid {
    key: EcdsaKeyPair,
}

impl Vapid {
    /// A PEM private key (PKCS#8 or SEC1), or the raw 32-byte scalar in base64url.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.starts_with("-----BEGIN") {
            return Self::from_pem(raw);
        }
        Self::from_scalar(&decode(ENV_KEY, raw)?)
    }

    fn from_pem(pem: &str) -> Result<Self, String> {
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = STANDARD.decode(b64.trim()).map_err(|_| "VAPID PEM is not base64".to_owned())?;
        EcdsaKeyPair::from_private_key_der(&ECDSA_P256_SHA256_FIXED_SIGNING, &der)
            .map(|key| Self { key })
            .map_err(|e| format!("VAPID PEM is not a P-256 private key: {e}"))
    }

    fn from_scalar(scalar: &[u8]) -> Result<Self, String> {
        let rejected = |e: aws_lc_rs::error::KeyRejected| format!("VAPID key rejected: {e}");
        let public = agreement::PrivateKey::from_private_key(&agreement::ECDH_P256, scalar)
            .map_err(rejected)?
            .compute_public_key()
            .map_err(|_| "VAPID public key derivation failed".to_owned())?;
        EcdsaKeyPair::from_private_key_and_public_key(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            scalar,
            public.as_ref(),
        )
        .map(|key| Self { key })
        .map_err(rejected)
    }

    fn generate() -> Result<Self, String> {
        EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
            .map(|key| Self { key })
            .map_err(|_| "VAPID key generation failed".to_owned())
    }

    fn scalar(&self) -> Result<String, String> {
        let bin = self
            .key
            .private_key()
            .as_be_bytes()
            .map_err(|_| "VAPID key export failed".to_owned())?;
        let bytes: &[u8] = bin.as_ref();
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref())
    }

    fn jwt(&self, audience: &str, subject: &str, now: i64) -> Result<String, String> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = json!({ "aud": audience, "exp": now + JWT_LIFETIME_SECS, "sub": subject });
        let input = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
        let sig = self
            .key
            .sign(&rand::SystemRandom::new(), input.as_bytes())
            .map_err(|_| "VAPID signing failed".to_owned())?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())))
    }

    fn authorization(&self, endpoint: &str, subject: &str, now: i64) -> Result<String, String> {
        let url =
            reqwest::Url::parse(endpoint).map_err(|_| "malformed push endpoint".to_owned())?;
        let audience = url.origin().ascii_serialization();
        Ok(format!("vapid t={}, k={}", self.jwt(&audience, subject, now)?, self.public_key()))
    }
}

async fn stored(pool: &PgPool) -> Result<Option<Vapid>, String> {
    let value: Option<Value> = sqlx::query_scalar(SELECT_KEY_SQL)
        .bind(SETTINGS_KEY)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("VAPID key read failed: {e}"))?;
    let sealed = value.as_ref().and_then(|v| v.get("encrypted_key")).and_then(Value::as_str);
    let Some(sealed) = sealed else { return Ok(None) };
    let scalar = crate::crypto::decrypt(sealed, &crate::crypto::vault_key())
        .ok_or_else(|| "stored VAPID key does not decrypt with the vault key".to_owned())?;
    Vapid::parse(&scalar).map(Some)
}

/// Concurrent first calls agree on whichever insert lands first.
async fn stored_or_generated(pool: &PgPool) -> Result<Vapid, String> {
    if let Some(key) = stored(pool).await? {
        return Ok(key);
    }
    let scalar = Vapid::generate()?.scalar()?;
    let sealed = crate::crypto::encrypt(&scalar, &crate::crypto::vault_key());
    sqlx::query(INSERT_KEY_SQL)
        .bind(SETTINGS_KEY)
        .bind(json!({ "encrypted_key": sealed }))
        .execute(pool)
        .await
        .map_err(|e| format!("VAPID key write failed: {e}"))?;
    stored(pool).await?.ok_or_else(|| "VAPID key was not stored".to_owned())
}

async fn load(pool: &PgPool) -> Result<Vapid, String> {
    match std::env::var(ENV_KEY) {
        Ok(raw) if !raw.trim().is_empty() => Vapid::parse(&raw),
        _ => stored_or_generated(pool).await,
    }
}

pub async fn vapid(pool: &PgPool) -> Result<&'static Vapid, String> {
    static VAPID: tokio::sync::OnceCell<Vapid> = tokio::sync::OnceCell::const_new();
    VAPID.get_or_try_init(|| load(pool)).await
}

fn subject(external_url: &str) -> String {
    let url = external_url.trim_end_matches('/');
    if url.starts_with("https://") { url.to_owned() } else { FALLBACK_SUBJECT.to_owned() }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn tag(n: &Rendered) -> String {
    n.delivery_id.simple().to_string()
}

fn tag_of(r: &ProviderRef) -> Result<&str, SendError> {
    r.0.get("tag")
        .and_then(Value::as_str)
        .ok_or_else(|| SendError::Permanent("web push provider ref has no tag".into()))
}

/// The service worker cannot POST, so a token button opens the confirm page instead.
fn confirm_link(url: &str) -> String {
    url.replacen(ACTION_PATH, CONFIRM_PATH, 1)
}

const fn urgency(priority: u8) -> &'static str {
    match priority {
        4.. => "high",
        3 => "normal",
        2 => "low",
        _ => "very-low",
    }
}

fn show_payload(n: &Rendered, tag: &str, renotify: bool) -> Value {
    let actions: Vec<Value> = n
        .buttons
        .iter()
        .filter(|b| b.press == Press::Post)
        .take(MAX_ACTIONS)
        .enumerate()
        .map(|(i, b)| {
            json!({ "action": format!("a{i}"), "title": b.label, "url": confirm_link(&b.url) })
        })
        .collect();
    json!({
        "type": "show",
        "title": truncate(&n.title, MAX_TITLE_CHARS),
        "body": n.body.as_deref().map(|b| truncate(b, MAX_BODY_CHARS)),
        "tag": tag,
        "renotify": renotify,
        "data": { "url": n.link },
        "actions": actions,
    })
}

/// Closes the shown notice; a browser that must show every push (iOS) shows `title`.
fn close_payload(tag: &str) -> Value {
    json!({ "type": "close", "tag": tag, "title": RESOLVED_TITLE })
}

async fn classify(resp: Result<reqwest::Response, reqwest::Error>) -> Result<(), SendError> {
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) if e.is_builder() => return Err(SendError::Permanent(e.to_string())),
        Err(e) => return Err(SendError::Retry(e.to_string())),
    };
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::GONE || status == reqwest::StatusCode::NOT_FOUND {
        return Err(SendError::Gone);
    }
    let text = resp.text().await.unwrap_or_default();
    let detail = format!("push service returned {status}: {text}");
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Err(SendError::Retry(detail))
    } else {
        Err(SendError::Permanent(detail))
    }
}

pub struct WebPush {
    subject: String,
    allow: Vec<AllowedHost>,
    key: Option<Arc<Vapid>>,
}

impl WebPush {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self { subject: subject(&config.external_url), allow: Vec::new(), key: None }
    }

    async fn key(&self) -> Result<&Vapid, SendError> {
        if let Some(key) = &self.key {
            return Ok(key.as_ref());
        }
        let pool = super::installed_pool()
            .ok_or_else(|| SendError::Retry("notification outbox not started".into()))?;
        vapid(&pool).await.map_err(SendError::Retry)
    }

    async fn push(
        &self,
        to: &Endpoint,
        payload: &Value,
        tag: &str,
        urgency: &str,
    ) -> Result<(), SendError> {
        let sub = Subscription::parse(&to.config).map_err(SendError::Permanent)?;
        validate_outbound_url(&sub.endpoint, &self.allow).await.map_err(|e| match e {
            OutboundUrlError::Unresolvable => SendError::Retry(format!("push endpoint {e}")),
            e => SendError::Permanent(format!("push endpoint {e}")),
        })?;
        let auth = self
            .key()
            .await?
            .authorization(&sub.endpoint, &self.subject, chrono::Utc::now().timestamp())
            .map_err(SendError::Permanent)?;
        let body = encrypt(payload.to_string().as_bytes(), &sub)
            .map_err(|_| SendError::Permanent("web push encryption failed".into()))?;
        let req = client()
            .post(&sub.endpoint)
            .header("Authorization", auth)
            .header("TTL", TTL_SECS.to_string())
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Urgency", urgency)
            .header("Topic", tag)
            .body(body);
        classify(req.send().await).await
    }
}

#[async_trait::async_trait]
impl Channel for WebPush {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { update: true, retract: true, buttons: 3, needs_token: true }
    }

    async fn send(&self, n: &Rendered, to: &Endpoint) -> Result<ProviderRef, SendError> {
        let tag = tag(n);
        self.push(to, &show_payload(n, &tag, true), &tag, urgency(n.priority)).await?;
        Ok(ProviderRef(json!({ "tag": tag })))
    }

    async fn update(&self, n: &Rendered, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
        let tag = tag_of(r)?;
        self.push(to, &show_payload(n, tag, false), tag, urgency(n.priority)).await
    }

    async fn retract(&self, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError> {
        let tag = tag_of(r)?;
        self.push(to, &close_payload(tag), tag, "low").await
    }
}

#[cfg(test)]
#[path = "web_push_tests.rs"]
mod tests;
