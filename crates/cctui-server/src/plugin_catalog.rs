//! The published plugin catalog.
//!
//! `plugins/catalog.json` in this repo is the source of truth. The server reads
//! it from `CCTUI_PLUGIN_CATALOG_URL` (default: the raw file on the repo's main
//! branch, so a new plugin needs no cctui release) and caches the answer for an
//! hour. A copy embedded at build time serves offline instances, and
//! `CCTUI_PLUGIN_CATALOG_URL=off` uses only that copy. Entries the schema
//! rejects are logged and skipped, so one bad line never hides the rest.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const EMBEDDED: &str = include_str!("../../../plugins/catalog.json");
const FRESH_TTL: Duration = Duration::from_hours(1);
const RETRY_TTL: Duration = Duration::from_mins(5);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BYTES: usize = 256 * 1024;

/// One published plugin: display metadata plus the pinned archive.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub homepage: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    plugins: Vec<serde_json::Value>,
}

fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_sha256(hex: &str) -> bool {
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn valid_https(url: &str) -> bool {
    url.starts_with("https://") && reqwest::Url::parse(url).is_ok_and(|u| u.host_str().is_some())
}

/// `None` when the entry does not satisfy the schema.
fn check(entry: CatalogEntry) -> Option<CatalogEntry> {
    let ok = valid_id(&entry.id)
        && !entry.name.trim().is_empty()
        && !entry.version.trim().is_empty()
        && valid_https(&entry.url)
        && valid_sha256(&entry.sha256)
        && entry.homepage.as_deref().is_none_or(valid_https);
    ok.then_some(entry)
}

/// Every entry of a catalog document that passes validation. A document that
/// does not parse at all yields an empty list.
pub fn parse(body: &str) -> Vec<CatalogEntry> {
    let Ok(file) = serde_json::from_str::<CatalogFile>(body) else {
        tracing::warn!("plugin catalog is not valid JSON");
        return Vec::new();
    };
    let mut out = Vec::with_capacity(file.plugins.len());
    for raw in file.plugins {
        let id = raw.get("id").and_then(serde_json::Value::as_str).unwrap_or("?").to_owned();
        if let Some(entry) = serde_json::from_value::<CatalogEntry>(raw).ok().and_then(check) {
            out.push(entry);
        } else {
            tracing::warn!(id, "plugin catalog entry skipped: invalid");
        }
    }
    out
}

/// The catalog committed in this build.
pub fn embedded() -> Vec<CatalogEntry> {
    parse(EMBEDDED)
}

/// `off` disables the fetch entirely.
pub fn catalog_url() -> String {
    std::env::var("CCTUI_PLUGIN_CATALOG_URL")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            format!(
                "https://raw.githubusercontent.com/{}/main/plugins/catalog.json",
                crate::routes::manifest::repo()
            )
        })
}

struct Cached {
    at: Instant,
    ttl: Duration,
    entries: Vec<CatalogEntry>,
}

fn cache() -> &'static Mutex<Option<Cached>> {
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn cached() -> Option<Vec<CatalogEntry>> {
    let guard = cache().lock().ok()?;
    let hit = guard.as_ref()?;
    let fresh = (hit.at.elapsed() < hit.ttl).then(|| hit.entries.clone());
    drop(guard);
    fresh
}

fn remember(entries: &[CatalogEntry], ttl: Duration) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(Cached { at: Instant::now(), ttl, entries: entries.to_vec() });
    }
}

/// The catalog body from `url`, through the SSRF-guarded client. `None` on any
/// transport, status or size failure — the caller falls back to [`embedded`].
async fn fetch(url: &str) -> Option<String> {
    let allow = crate::outbound::upstream_allowlist();
    if let Err(e) = crate::outbound::validate_outbound_url(url, &allow).await {
        tracing::warn!("plugin catalog url refused: {e}");
        return None;
    }
    let resp = crate::outbound::upstream_client()
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| tracing::warn!("plugin catalog fetch failed: {e}"))
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "plugin catalog fetch rejected");
        return None;
    }
    if resp.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        tracing::warn!("plugin catalog too large");
        return None;
    }
    let body =
        resp.text().await.map_err(|e| tracing::warn!("plugin catalog read failed: {e}")).ok()?;
    if body.len() > MAX_BYTES {
        tracing::warn!("plugin catalog too large");
        return None;
    }
    Some(body)
}

/// The catalog to serve: the cached copy while fresh, else a fetch, else the
/// embedded copy. A failed fetch is remembered briefly so a down remote does
/// not cost every request a timeout.
pub async fn entries() -> Vec<CatalogEntry> {
    if let Some(hit) = cached() {
        return hit;
    }
    let (list, ttl) = resolve(&catalog_url()).await;
    remember(&list, ttl);
    list
}

/// The cache-free half of [`entries`]: what `url` yields, and how long that
/// answer is worth keeping.
async fn resolve(url: &str) -> (Vec<CatalogEntry>, Duration) {
    if url == "off" {
        return (embedded(), FRESH_TTL);
    }
    match fetch(url).await.map(|body| parse(&body)) {
        Some(fetched) if !fetched.is_empty() => (fetched, FRESH_TTL),
        _ => (embedded(), RETRY_TTL),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::{FRESH_TTL, RETRY_TTL, embedded, parse, resolve, sha256_hex};

    fn demo() -> String {
        format!(
            r#"{{"id":"demo","name":"Demo","description":"d","version":"1.0.0",
               "url":"https://example.com/demo.tgz","sha256":"{}",
               "homepage":"https://example.com"}}"#,
            "a".repeat(64)
        )
    }

    fn doc(plugins: &[&str]) -> String {
        format!(r#"{{"version":1,"plugins":[{}]}}"#, plugins.join(","))
    }

    fn one(json: &str) -> String {
        doc(&[json])
    }

    #[test]
    fn the_committed_catalog_parses_and_validates() {
        let list = embedded();
        assert!(!list.is_empty(), "plugins/catalog.json must hold at least one valid entry");
        let yubi = list.iter().find(|e| e.id == "yubisashi").expect("yubisashi is published");
        assert!(yubi.url.starts_with("https://github.com/DorskFR/yubisashi/releases/download/"));
        assert_eq!(yubi.sha256.len(), 64);
        assert!(yubi.url.contains(&yubi.version), "the asset url pins the catalog version");
    }

    #[test]
    fn a_well_formed_entry_survives_parsing() {
        let list = parse(&one(&demo()));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "demo");
        assert_eq!(list[0].url, "https://example.com/demo.tgz");
        assert_eq!(list[0].homepage.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn bad_entries_are_skipped_not_fatal() {
        let bad = |body: &str| assert!(parse(body).is_empty(), "should have been rejected: {body}");
        let broken = |from: &str, to: &str| one(&demo().replace(from, to));
        bad(&broken(r#""id":"demo""#, r#""id":"Demo!""#));
        bad(&broken(r#""id":"demo""#, r#""id":"""#));
        bad(&broken("https://example.com/demo.tgz", "http://example.com/demo.tgz"));
        bad(&broken(&"a".repeat(64), &"a".repeat(63)));
        bad(&broken(&"a".repeat(64), &"A".repeat(64)));
        bad(&broken(r#""name":"Demo""#, r#""name":" ""#));
        bad(&broken(r#""version":"1.0.0""#, r#""version":"""#));
        bad(&broken(r#""homepage":"https://example.com""#, r#""homepage":"javascript:0""#));
        bad("not json at all");
        bad(&one(r#"{"id":"demo"}"#));
    }

    #[test]
    fn one_bad_entry_does_not_hide_the_good_one() {
        let list = parse(&doc(&[r#"{"id":"BAD"}"#, &demo()]));
        assert_eq!(list.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["demo"]);
    }

    #[tokio::test]
    async fn off_serves_only_the_embedded_copy() {
        assert_eq!(resolve("off").await, (embedded(), FRESH_TTL));
    }

    #[tokio::test]
    async fn an_unreachable_url_falls_back_to_the_embedded_copy() {
        let (list, ttl) = resolve("https://catalog.invalid.example.test/catalog.json").await;
        assert_eq!(list, embedded());
        assert_eq!(ttl, RETRY_TTL, "a failed fetch is retried sooner than a good one");
    }

    #[tokio::test]
    async fn a_refused_url_never_leaves_the_embedded_copy() {
        assert_eq!(resolve("http://127.0.0.1/catalog.json").await.0, embedded());
        assert_eq!(resolve("file:///etc/passwd").await.0, embedded());
    }

    #[test]
    fn sha256_matches_the_reference_digest_of_the_empty_input() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
