use std::sync::{Arc, Mutex};

use aws_lc_rs::error::Unspecified;
use aws_lc_rs::{aead, agreement, signature};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    ACTION_TITLE, FALLBACK_SUBJECT, NOTICE_TITLE, SELECT_KEY_SQL, SETTINGS_KEY, Subscription,
    URGENT_TITLE, Vapid, WebPush, close_payload, derive, encrypt, encrypt_with, show_payload,
    stored_or_generated, subject,
};
use crate::notify::channel::{Button, Channel, Endpoint, Press, ProviderRef, Rendered, SendError};

const PLAINTEXT: &str = "When I grow up, I want to be a watermelon";
const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
const AS_PUBLIC: &str = concat!(
    "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBr",
    "u3jl7A8",
);
const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str = concat!(
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkN",
    "toIAiw4",
);
const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
const EXPECTED: &str = concat!(
    "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vC",
    "YLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXL",
    "WyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
);

fn b64(s: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(s).unwrap()
}

fn subscription(endpoint: &str) -> Value {
    json!({ "endpoint": endpoint, "keys": { "p256dh": UA_PUBLIC, "auth": AUTH } })
}

fn rfc_subscription() -> Subscription {
    let endpoint = "https://push.example.net/push/JzLQ3raZJfFBR0aqvOMsLrt54w4rJUsV";
    Subscription::parse(&subscription(endpoint)).unwrap()
}

fn rfc_vapid() -> Vapid {
    Vapid::parse(AS_PRIVATE).unwrap()
}

fn open(body: &[u8]) -> Value {
    let (salt, rest) = body.split_at(16);
    assert_eq!(rest[..4], 4096u32.to_be_bytes());
    let (sender_pub, record) = rest[5..].split_at(usize::from(rest[4]));
    let ua =
        agreement::PrivateKey::from_private_key(&agreement::ECDH_P256, &b64(UA_PRIVATE)).unwrap();
    let to = rfc_subscription();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sender_pub);
    let keys = |secret: &[u8]| derive(secret, &to, sender_pub, salt);
    let (cek, nonce) = agreement::agree(&ua, peer, Unspecified, keys).unwrap();
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
    let mut record = record.to_vec();
    let plain = key
        .open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut record)
        .unwrap();
    let (delimiter, json) = plain.split_last().unwrap();
    assert_eq!(*delimiter, 0x02);
    serde_json::from_slice(json).unwrap()
}

#[test]
fn rfc8291_appendix_a_vector() {
    let sender =
        agreement::PrivateKey::from_private_key(&agreement::ECDH_P256, &b64(AS_PRIVATE)).unwrap();
    let salt: [u8; 16] = b64(SALT).try_into().unwrap();
    let body = encrypt_with(PLAINTEXT.as_bytes(), &rfc_subscription(), &sender, salt).unwrap();
    assert_eq!(URL_SAFE_NO_PAD.encode(&body), EXPECTED);
}

#[test]
fn a_sealed_payload_opens_for_the_subscriber_only_with_a_fresh_salt_and_key() {
    let payload = json!({ "type": "show", "title": "hello" });
    let first = encrypt(payload.to_string().as_bytes(), &rfc_subscription()).unwrap();
    let second = encrypt(payload.to_string().as_bytes(), &rfc_subscription()).unwrap();
    assert_eq!(open(&first), payload);
    assert_ne!(first[..16], second[..16]);
    assert_ne!(first[21..86], second[21..86]);
}

#[test]
fn a_raw_vapid_scalar_derives_its_public_point() {
    assert_eq!(rfc_vapid().public_key(), AS_PUBLIC);
}

#[test]
fn a_pem_or_raw_vapid_key_parses_to_the_same_keypair() {
    let generated = Vapid::generate().unwrap();
    let der = generated.key.to_pkcs8v1().unwrap();
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        STANDARD.encode(der.as_ref())
    );
    assert_eq!(Vapid::parse(&pem).unwrap().public_key(), generated.public_key());
    let raw = generated.scalar().unwrap();
    assert_eq!(Vapid::parse(&raw).unwrap().public_key(), generated.public_key());
    assert!(Vapid::parse("not a key").is_err());
}

#[test]
fn the_vapid_jwt_has_the_es256_header_and_push_claims() {
    let auth = rfc_vapid()
        .authorization("https://push.example.net/push/abc?x=1", "https://cctui.example", 1_000)
        .unwrap();
    let (jwt, k) = auth.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
    assert_eq!(k, AS_PUBLIC);
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: Value = serde_json::from_slice(&b64(parts[0])).unwrap();
    assert_eq!(header, json!({ "typ": "JWT", "alg": "ES256" }));
    let claims: Value = serde_json::from_slice(&b64(parts[1])).unwrap();
    let expected = json!({
        "aud": "https://push.example.net", "exp": 1_000 + 12 * 3600, "sub": "https://cctui.example",
    });
    assert_eq!(claims, expected);
    let sig = b64(parts[2]);
    assert_eq!(sig.len(), 64);
    let signed = format!("{}.{}", parts[0], parts[1]);
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, b64(AS_PUBLIC))
        .verify(signed.as_bytes(), &sig)
        .unwrap();
}

#[test]
fn the_subject_is_the_external_url_or_a_mailto() {
    assert_eq!(subject("https://cctui.example/"), "https://cctui.example");
    assert_eq!(subject("http://localhost:5173"), FALLBACK_SUBJECT);
    assert_eq!(subject(""), FALLBACK_SUBJECT);
}

#[test]
fn a_subscription_parses_bare_or_wrapped_and_rejects_bad_keys() {
    let bare = subscription("https://push.example.net/x");
    let wrapped = json!({ "subscription": bare });
    assert_eq!(Subscription::parse(&bare), Subscription::parse(&wrapped));
    assert_eq!(Subscription::parse(&bare).unwrap().endpoint, "https://push.example.net/x");
    let short =
        json!({ "endpoint": "https://p.example", "keys": { "p256dh": AUTH, "auth": AUTH } });
    assert!(Subscription::parse(&short).unwrap_err().contains("p256dh"));
    let no_auth = json!({ "endpoint": "https://p.example", "keys": { "p256dh": UA_PUBLIC } });
    assert!(Subscription::parse(&no_auth).unwrap_err().contains("keys.auth"));
    assert!(Subscription::parse(&json!({ "keys": {} })).unwrap_err().contains("endpoint"));
}

fn action(token: &str) -> String {
    format!("https://cctui.example/api/v1/notifications/actions/{token}")
}

fn rendered() -> Rendered {
    Rendered {
        delivery_id: Uuid::new_v4(),
        title: "Approve deploy".into(),
        body: Some("prod".into()),
        priority: 4,
        tags: None,
        link: Some("https://cctui.example/sessions/s1".into()),
        buttons: vec![
            Button {
                label: "Open".into(),
                url: "https://cctui.example/sessions/s1".into(),
                press: Press::Open,
            },
            Button { label: "Approve".into(), url: action("tokA"), press: Press::Post },
            Button { label: "Deny".into(), url: action("tokD"), press: Press::Post },
        ],
    }
}

#[test]
fn token_buttons_open_the_confirm_page_and_the_body_opens_the_link() {
    let payload = show_payload(&rendered(), "t1", true);
    assert_eq!(payload["type"], "show");
    assert_eq!(payload["tag"], "t1");
    assert_eq!(payload["renotify"], true);
    assert_eq!(payload["data"]["url"], "https://cctui.example/sessions/s1");
    assert_eq!(
        payload["actions"],
        json!([
            { "action": "a0", "title": "Approve", "url": "https://cctui.example/n/tokA" },
            { "action": "a1", "title": "Deny", "url": "https://cctui.example/n/tokD" },
        ])
    );
}

#[test]
fn the_retract_payload_closes_by_tag() {
    assert_eq!(close_payload("t1"), json!({ "type": "close", "tag": "t1", "title": "Resolved" }));
}

type Seen = Arc<Mutex<Vec<(String, http::HeaderMap, Vec<u8>)>>>;

async fn stub(status: u16) -> (String, Seen) {
    stub_with(status, None).await
}

async fn stub_with(status: u16, location: Option<&'static str>) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let recorder = seen.clone();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let recorder = recorder.clone();
        async move {
            let (parts, body) = req.into_parts();
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
            recorder.lock().unwrap().push((parts.uri.path().to_owned(), parts.headers, body));
            let mut resp = http::Response::new(axum::body::Body::empty());
            *resp.status_mut() = http::StatusCode::from_u16(status).unwrap();
            if let Some(location) = location {
                resp.headers_mut().insert(http::header::LOCATION, location.parse().unwrap());
            }
            resp
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

fn channel(base: &str) -> WebPush {
    WebPush {
        subject: "https://cctui.example".into(),
        allow: cctui_server_kernel::outbound::parse_allowlist(base.trim_start_matches("http://")),
        key: Some(Arc::new(rfc_vapid())),
    }
}

fn endpoint(base: &str) -> Endpoint {
    Endpoint { config: subscription(&format!("{base}/push/sub1")) }
}

fn header(headers: &http::HeaderMap, name: &str) -> String {
    headers.get(name).unwrap().to_str().unwrap().to_owned()
}

#[tokio::test]
async fn send_posts_an_encrypted_notice_tagged_with_the_delivery() {
    let (base, seen) = stub(201).await;
    let n = rendered();
    let r = channel(&base).send(&n, &endpoint(&base)).await.unwrap();
    let tag = n.delivery_id.simple().to_string();
    assert_eq!(r, ProviderRef(json!({ "tag": tag })));

    let (path, headers, body) = seen.lock().unwrap().pop().unwrap();
    assert_eq!(path, "/push/sub1");
    assert_eq!(header(&headers, "content-encoding"), "aes128gcm");
    assert_eq!(header(&headers, "topic"), tag);
    assert_eq!(header(&headers, "ttl"), "86400");
    assert_eq!(header(&headers, "urgency"), "high");
    assert!(header(&headers, "authorization").starts_with("vapid t="));
    let shown = open(&body);
    assert_eq!(shown["type"], "show");
    assert_eq!(shown["title"], ACTION_TITLE);
    assert_eq!(shown["tag"], tag.as_str());
}

const TRANSCRIPT: &str = "XYLOPHONE-TRANSCRIPT-";
const MAX_PAYLOAD_BYTES: usize = 1024;

fn with_transcript(repeat: usize) -> Rendered {
    Rendered {
        title: format!("Agent says {}", TRANSCRIPT.repeat(repeat)),
        body: Some(TRANSCRIPT.repeat(repeat)),
        ..rendered()
    }
}

#[tokio::test]
async fn a_long_body_never_reaches_the_decrypted_push() {
    let (base, seen) = stub(201).await;
    channel(&base).send(&with_transcript(500), &endpoint(&base)).await.unwrap();
    let (_, _, body) = seen.lock().unwrap().pop().unwrap();
    assert!(body.len() < MAX_PAYLOAD_BYTES, "{} bytes on the wire", body.len());
    let shown = open(&body);
    assert!(shown.get("body").is_none(), "{shown}");
    let plain = shown.to_string();
    let chars: Vec<char> = TRANSCRIPT.repeat(2).chars().collect();
    for window in chars.windows(5) {
        let piece: String = window.iter().collect();
        assert!(!plain.contains(&piece), "{piece:?} leaked into {plain}");
    }
}

#[test]
fn the_payload_size_does_not_grow_with_the_body_or_title() {
    let sizes: Vec<usize> = [0, 1, 50, 5_000]
        .into_iter()
        .map(|repeat| show_payload(&with_transcript(repeat), "t1", true).to_string().len())
        .collect();
    assert!(sizes.iter().all(|&len| len == sizes[0]), "{sizes:?}");
    assert!(sizes[0] < MAX_PAYLOAD_BYTES, "{sizes:?}");
}

#[test]
fn the_title_is_generic_and_never_the_producer_title() {
    let urgent = Rendered { buttons: Vec::new(), ..with_transcript(3) };
    let quiet = Rendered { priority: 2, ..urgent.clone() };
    assert_eq!(show_payload(&with_transcript(3), "t1", true)["title"], ACTION_TITLE);
    assert_eq!(show_payload(&urgent, "t1", true)["title"], URGENT_TITLE);
    assert_eq!(show_payload(&quiet, "t1", true)["title"], NOTICE_TITLE);
}

#[tokio::test]
async fn update_replaces_the_notice_by_tag_without_renotifying() {
    let (base, seen) = stub(201).await;
    let r = ProviderRef(json!({ "tag": "t-1" }));
    channel(&base).update(&rendered(), &endpoint(&base), &r).await.unwrap();
    let (_, headers, body) = seen.lock().unwrap().pop().unwrap();
    assert_eq!(header(&headers, "topic"), "t-1");
    let shown = open(&body);
    assert_eq!(shown["tag"], "t-1");
    assert_eq!(shown["renotify"], false);
}

#[tokio::test]
async fn retract_pushes_the_close_payload_for_the_tag() {
    let (base, seen) = stub(201).await;
    let r = ProviderRef(json!({ "tag": "t-1" }));
    channel(&base).retract(&endpoint(&base), &r).await.unwrap();
    let (_, headers, body) = seen.lock().unwrap().pop().unwrap();
    assert_eq!(header(&headers, "topic"), "t-1");
    assert_eq!(header(&headers, "urgency"), "low");
    assert_eq!(open(&body), close_payload("t-1"));
}

#[tokio::test]
async fn a_404_or_410_from_the_push_service_is_gone() {
    for status in [404, 410] {
        let (base, _) = stub(status).await;
        let err = channel(&base).send(&rendered(), &endpoint(&base)).await.expect_err("gone");
        assert_eq!(err, SendError::Gone, "{status}");
    }
}

#[tokio::test]
async fn throttled_or_failing_push_services_retry_and_other_refusals_are_permanent() {
    for status in [429, 500, 503] {
        let (base, _) = stub(status).await;
        let err = channel(&base).send(&rendered(), &endpoint(&base)).await.expect_err("retry");
        assert!(matches!(err, SendError::Retry(_)), "{status}: {err:?}");
    }
    let (base, _) = stub(403).await;
    let err = channel(&base).send(&rendered(), &endpoint(&base)).await.expect_err("refused");
    assert!(matches!(err, SendError::Permanent(_)), "{err:?}");
}

#[tokio::test]
async fn a_redirecting_push_service_is_not_followed() {
    for status in [302, 307, 308] {
        let (base, seen) = stub_with(status, Some("/internal")).await;
        let err = channel(&base).send(&rendered(), &endpoint(&base)).await.unwrap_err();
        assert!(matches!(err, SendError::Permanent(_)), "{status}: {err:?}");
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|(p, ..)| p.clone()).collect();
        assert_eq!(paths, vec!["/push/sub1".to_owned()], "{status}");
    }
}

#[tokio::test]
async fn an_endpoint_outside_the_allowlist_is_never_called() {
    let (base, seen) = stub(201).await;
    let mut push = channel(&base);
    push.allow.clear();
    let err = push.send(&rendered(), &endpoint(&base)).await.expect_err("refused");
    assert!(matches!(err, SendError::Permanent(_)), "{err:?}");
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_instance_vapid_key_is_generated_once_and_stored_sealed() {
    let Some(url) = cctui_server_kernel::test_support::db_url("web_push_vapid_key") else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![0x5a; 32]);
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    sqlx::query("DELETE FROM instance_settings WHERE key = $1")
        .bind(SETTINGS_KEY)
        .execute(&pool)
        .await
        .unwrap();

    let first = stored_or_generated(&pool).await.unwrap();
    let again = stored_or_generated(&pool).await.unwrap();
    assert_eq!(first.public_key(), again.public_key());

    let raw: Value =
        sqlx::query_scalar(SELECT_KEY_SQL).bind(SETTINGS_KEY).fetch_one(&pool).await.unwrap();
    let sealed = raw["encrypted_key"].as_str().unwrap();
    assert_ne!(sealed, first.scalar().unwrap(), "the private key is sealed at rest");
}
