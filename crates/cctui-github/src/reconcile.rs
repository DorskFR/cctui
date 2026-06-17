//! GH-CONN-4: the reconcile poll loop.
//!
//! Webhooks (GH-CONN-2) are push-first but lossy — a missed delivery, a
//! connector added *after* PRs already existed, or a server restart all leave
//! `github.pulls` stale. This loop heals that drift: every `interval` seconds
//! (and once on start / first install) it asks GitHub for the PRs that **involve
//! me** for each connector, parses them into the same [`crate::store`] `*Upsert`
//! types the webhook uses, and upserts them (which broadcasts a `ServerEvent`).
//!
//! Scope of "involves me" (docs/github-integration.md §8.2, recommended v1):
//! **authored + review-requested, direct & team**. That maps to GitHub's issue
//! search qualifiers `author:@me`, `review-requested:@me`, and
//! `team-review-requested:@me`, restricted to each connector's `repo:` slugs and
//! to `is:pr is:open`.
//!
//! Rate-limit hygiene (docs §6.1 / §11 — "not optional"):
//! - **Conditional requests (`ETag`).** Each query's last `ETag` is cached
//!   in-memory and replayed as `If-None-Match`; a `304 Not Modified` costs no
//!   primary-rate quota and means "nothing changed, skip".
//! - **Back off on limits.** A `403`/`429` with `Retry-After` or an exhausted
//!   `x-ratelimit-remaining` parks the loop until the reset, so we never hammer
//!   into a secondary limit.
//!
//! The GitHub HTTP surface is hidden behind the [`SearchClient`] trait so the
//! query construction, ETag-handling, and upsert mapping are unit-testable
//! without a network. The reqwest-backed [`HttpSearchClient`] is the production
//! impl.
//!
//! Secrets: the connector credential is decrypted only to build the
//! `Authorization` header and is **never** logged. Search responses carry no
//! secret material.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::github::PullUpsert;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{GithubState, classifier_feed, crypto, store};

/// Default poll cadence. Overridable via `CCTUI_GITHUB_RECONCILE_SECS`; a value
/// of `0` disables the loop entirely (webhooks only).
const DEFAULT_INTERVAL_SECS: u64 = 300;

/// The outcome of one search query.
pub enum SearchOutcome {
    /// GitHub returned `304 Not Modified` — the result is unchanged since the
    /// cached `ETag`, so there is nothing to upsert.
    NotModified,
    /// A fresh result: the matched pulls plus the response `ETag` (if any) to
    /// cache for the next conditional request.
    Modified { pulls: Vec<PullUpsert>, etag: Option<String> },
    /// The request was rate-limited; the loop should pause for `retry_after`
    /// before its next attempt.
    RateLimited { retry_after: Duration },
}

/// Abstracts the GitHub search round-trip so the reconcile logic is testable
/// without a live API. Implementors perform the conditional request and parse
/// matched PRs into [`PullUpsert`]s.
#[async_trait::async_trait]
pub trait SearchClient: Send + Sync {
    /// Run the GitHub issue search for `query` with the connector's credential,
    /// replaying `etag` as `If-None-Match` when present.
    async fn search(
        &self,
        credential: &str,
        query: &str,
        etag: Option<&str>,
    ) -> anyhow::Result<SearchOutcome>;

    /// Resolve the credential's own GitHub login (`GET /user` → `login`), cached
    /// on the connector so the inbox can split authored vs. review-requested PRs
    /// (GH-UI-1 / GH-CONN-6) without a per-request GitHub call.
    ///
    /// Best-effort: a `None` result (network error, token without user scope,
    /// or — for the test fake — no implementation) just leaves `viewer_login`
    /// unchanged. The default returns `None` so test fakes need not implement it.
    async fn viewer_login(&self, _credential: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

/// Build the GitHub issue-search query string for one connector scope.
///
/// Emits `is:pr is:open` plus the three "involves me" qualifiers (authored,
/// review-requested direct, review-requested team) and a `repo:` filter per
/// configured slug. A connector with no repos searches all repos the credential
/// can see. Bare `owner` slugs (no `/name`) become a `user:`/`org:`-style
/// `repo:owner/*`-equivalent fallback via the `user:` qualifier so an
/// owner-wide connector still works.
#[must_use]
pub fn build_query(repos: &[String]) -> String {
    let mut q = String::from("is:pr is:open");
    for r in repos {
        let r = r.trim();
        if r.is_empty() {
            continue;
        }
        if r.contains('/') {
            q.push_str(" repo:");
        } else {
            q.push_str(" user:");
        }
        q.push_str(r);
    }
    // The "involves me" scope: authored + review-requested (direct & team).
    q.push_str(" (author:@me OR review-requested:@me OR team-review-requested:@me)");
    q
}

/// Per-process `ETag` cache, keyed on `(connector_id, query)`. Lives only in
/// memory: a restart simply re-fetches once (a full hydrate), which is exactly
/// the first-install behaviour we want.
type EtagCache = Arc<Mutex<HashMap<(Uuid, String), String>>>;

/// Resolve the poll interval from the environment, defaulting to
/// [`DEFAULT_INTERVAL_SECS`]. `0` (or an unparsable value treated as default)
/// means: spawn nothing.
#[must_use]
pub fn interval_secs() -> u64 {
    std::env::var("CCTUI_GITHUB_RECONCILE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

/// Spawn the reconcile loop as a detached background task (mirrors the server's
/// `reaper_task`). Returns immediately. A `0` interval disables it.
///
/// The loop ticks on the configured interval; the first tick fires immediately,
/// giving the on-start / first-install hydrate for free.
pub fn spawn(
    pool: PgPool,
    events: store::EventTx,
    pr_cache: cctui_proto::classifier::PrStatusCache,
) {
    let state = GithubState { pool, events, pr_cache, diff_cache: crate::diff::DiffCache::new() };
    let secs = interval_secs();
    if secs == 0 {
        tracing::info!("cctui-github: reconcile loop disabled (interval = 0)");
        return;
    }
    let client = Arc::new(HttpSearchClient::new());
    tokio::spawn(async move {
        reconcile_loop(state, client, Duration::from_secs(secs)).await;
    });
    tracing::info!(interval_secs = secs, "cctui-github: reconcile loop spawned");
}

/// The poll loop body, generic over the [`SearchClient`] so tests drive it with
/// a fake. `tokio::time::interval` fires its first tick immediately, so a fresh
/// connector is hydrated within one loop iteration of being added.
async fn reconcile_loop(state: GithubState, client: Arc<dyn SearchClient>, period: Duration) {
    let etags: EtagCache = Arc::new(Mutex::new(HashMap::new()));
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(e) = reconcile_once(&state, client.as_ref(), &etags).await {
            // Never let one bad cycle kill the loop; log and retry next tick.
            tracing::warn!("github reconcile cycle error: {e}");
        }
    }
}

/// One full reconcile pass over every connector. Each connector is independent;
/// a failure on one is logged and does not block the others.
async fn reconcile_once(
    state: &GithubState,
    client: &dyn SearchClient,
    etags: &EtagCache,
) -> anyhow::Result<()> {
    let key = crypto::vault_key();
    let rows: Vec<(Uuid, String, Vec<String>)> = sqlx::query_as(
        "SELECT id, encrypted_credential, repos FROM github.connectors ORDER BY created_at",
    )
    .fetch_all(&state.pool)
    .await?;

    for (connector_id, enc_credential, repos) in rows {
        let Some(credential) = crypto::deobfuscate(&enc_credential, &key) else {
            tracing::warn!(%connector_id, "github reconcile: undecryptable credential, skipping");
            continue;
        };
        let query = build_query(&repos);
        if let Err(e) =
            reconcile_connector(state, client, etags, connector_id, &credential, &query).await
        {
            tracing::warn!(%connector_id, "github reconcile connector error: {e}");
        }
    }
    Ok(())
}

/// Reconcile a single connector: conditional search, then upsert each matched
/// pull. Returns the count of upserted pulls (for tests / logging).
async fn reconcile_connector(
    state: &GithubState,
    client: &dyn SearchClient,
    etags: &EtagCache,
    connector_id: Uuid,
    credential: &str,
    query: &str,
) -> anyhow::Result<usize> {
    // Cache the credential's own login once so the inbox can derive attention
    // buckets (GH-CONN-6) without a per-request GitHub call. Skip if already
    // known; best-effort, never fatal.
    let known: Option<String> =
        sqlx::query_scalar("SELECT viewer_login FROM github.connectors WHERE id = $1")
            .bind(connector_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
    if known.is_none()
        && let Ok(Some(login)) = client.viewer_login(credential).await
    {
        let _ = sqlx::query("UPDATE github.connectors SET viewer_login = $1 WHERE id = $2")
            .bind(&login)
            .bind(connector_id)
            .execute(&state.pool)
            .await;
    }

    let cache_key = (connector_id, query.to_string());
    let prior = etags.lock().await.get(&cache_key).cloned();

    match client.search(credential, query, prior.as_deref()).await? {
        SearchOutcome::NotModified => Ok(0),
        SearchOutcome::RateLimited { retry_after } => {
            tracing::info!(
                %connector_id,
                secs = retry_after.as_secs(),
                "github reconcile rate-limited, backing off"
            );
            tokio::time::sleep(retry_after).await;
            Ok(0)
        }
        SearchOutcome::Modified { pulls, etag } => {
            let n = upsert_pulls(&state.pool, state, connector_id, &pulls).await;
            if let Some(etag) = etag {
                etags.lock().await.insert(cache_key, etag);
            }
            Ok(n)
        }
    }
}

/// Upsert each matched pull and refresh its classifier status. Mirrors the
/// webhook's `pull_request` path so reconcile and webhook converge on identical
/// rows. Per-pull errors are logged, not fatal.
async fn upsert_pulls(
    pool: &PgPool,
    state: &GithubState,
    connector_id: Uuid,
    pulls: &[PullUpsert],
) -> usize {
    let mut n = 0;
    for p in pulls {
        match store::upsert_pull(pool, &state.events, connector_id, p).await {
            Ok(_) => {
                classifier_feed::refresh(pool, &state.pr_cache, connector_id, &p.repo, p.number)
                    .await;
                n += 1;
            }
            Err(e) => tracing::warn!(%connector_id, "github reconcile upsert error: {e}"),
        }
    }
    n
}

// ---- reqwest-backed production client --------------------------------------

const API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("cctui/", env!("CARGO_PKG_VERSION"));

/// Production [`SearchClient`] over GitHub's REST search API
/// (`GET /search/issues`). Reuses rustls via the workspace `reqwest`
/// (`default-features = false`, `rustls-tls`).
pub struct HttpSearchClient {
    http: reqwest::Client,
}

impl Default for HttpSearchClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpSearchClient {
    #[must_use]
    pub fn new() -> Self {
        Self { http: reqwest::Client::new() }
    }

    /// Fetch the full PR object for one search hit and parse it into a
    /// `PullUpsert`. Search results omit `head`/`base`/`mergeable_state`, so we
    /// hydrate from `GET /repos/{repo}/pulls/{n}`.
    async fn fetch_pull(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
    ) -> anyhow::Result<Option<PullUpsert>> {
        let url = format!("{API_BASE}/repos/{repo}/pulls/{number}");
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(parse_pull_object(repo, &v))
    }
}

#[async_trait::async_trait]
impl SearchClient for HttpSearchClient {
    async fn search(
        &self,
        credential: &str,
        query: &str,
        etag: Option<&str>,
    ) -> anyhow::Result<SearchOutcome> {
        let mut req = self
            .http
            .get(format!("{API_BASE}/search/issues"))
            .query(&[("q", query), ("per_page", "100")])
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if let Some(tag) = etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(SearchOutcome::NotModified);
        }
        if let Some(retry_after) = rate_limit_backoff(&resp) {
            return Ok(SearchOutcome::RateLimited { retry_after });
        }
        let status = resp.status();
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if !status.is_success() {
            anyhow::bail!("github search returned {status}");
        }
        let body: serde_json::Value = resp.json().await?;
        let items = body.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();

        // Each search hit names a repo + number; hydrate the full PR object so
        // the upsert has head/base/mergeable_state. Hits without a PR link or
        // that 404 on fetch are skipped.
        let mut pulls = Vec::new();
        for item in &items {
            let Some((repo, number)) = search_hit_repo_number(item) else { continue };
            if let Some(pull) = self.fetch_pull(credential, &repo, number).await? {
                pulls.push(pull);
            }
        }
        Ok(SearchOutcome::Modified { pulls, etag })
    }

    async fn viewer_login(&self, credential: &str) -> anyhow::Result<Option<String>> {
        let resp = self
            .http
            .get(format!("{API_BASE}/user"))
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("login").and_then(|l| l.as_str()).map(str::to_string))
    }
}

/// Compute how long to back off from a rate-limited response, or `None` if it
/// is not rate-limited. Honours `Retry-After` (seconds) first, then a
/// `403`/`429` with `x-ratelimit-remaining: 0` (waiting until the reset).
fn rate_limit_backoff(resp: &reqwest::Response) -> Option<Duration> {
    let status = resp.status();
    let is_limit_status = status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
    if !is_limit_status {
        return None;
    }
    let headers = resp.headers();
    if let Some(secs) =
        headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok())
    {
        return Some(Duration::from_secs(secs.clamp(1, 3600)));
    }
    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());
    if remaining == Some(0) {
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok());
        let now = chrono::Utc::now().timestamp();
        let wait = reset.map_or(60, |r| (r - now).clamp(1, 3600));
        return Some(Duration::from_secs(u64::try_from(wait).unwrap_or(60)));
    }
    // A 403 without rate-limit headers is some other auth/permission error;
    // surface it as a normal failure (not a back-off) by returning None.
    None
}

/// Extract `(owner/name, number)` from a search-issues hit. The `repository_url`
/// is `https://api.github.com/repos/{owner}/{name}`; `number` is top-level.
fn search_hit_repo_number(item: &serde_json::Value) -> Option<(String, i64)> {
    let number = item.get("number")?.as_i64()?;
    let repo_url = item.get("repository_url")?.as_str()?;
    let repo = repo_url.rsplit("/repos/").next()?;
    if repo.is_empty() || !repo.contains('/') {
        return None;
    }
    Some((repo.to_string(), number))
}

/// Parse a full `GET /repos/{repo}/pulls/{n}` object into a [`PullUpsert`].
/// Mirrors the webhook's `parse_pull` field selection but for the top-level PR
/// object (no `pull_request` wrapper, repo supplied by the caller).
fn parse_pull_object(repo: &str, pr: &serde_json::Value) -> Option<PullUpsert> {
    Some(PullUpsert {
        node_id: pr.get("node_id")?.as_str()?.to_string(),
        repo: repo.to_string(),
        number: pr.get("number")?.as_i64()?,
        title: pr.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
        state: pr.get("state").and_then(|s| s.as_str()).unwrap_or("open").to_string(),
        merged: pr.get("merged").and_then(serde_json::Value::as_bool).unwrap_or(false),
        draft: pr.get("draft").and_then(serde_json::Value::as_bool).unwrap_or(false),
        mergeable_state: pr.get("mergeable_state").and_then(|s| s.as_str()).map(str::to_string),
        author: pr
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string(),
        head_sha: pr
            .get("head")
            .and_then(|h| h.get("sha"))
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        base_ref: pr
            .get("base")
            .and_then(|b| b.get("ref"))
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string(),
        head_ref: pr
            .get("head")
            .and_then(|h| h.get("ref"))
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string(),
        gh_created_at: pr
            .get("created_at")
            .and_then(|t| t.as_str())
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
        gh_updated_at: pr
            .get("updated_at")
            .and_then(|t| t.as_str())
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_query_includes_involves_me_scope() {
        let q = build_query(&["o/r".into()]);
        assert!(q.contains("is:pr is:open"));
        assert!(q.contains("repo:o/r"));
        assert!(q.contains("author:@me"));
        assert!(q.contains("review-requested:@me"));
        assert!(q.contains("team-review-requested:@me"));
    }

    #[test]
    fn build_query_handles_owner_only_and_empty() {
        let q = build_query(&["owner".into(), "  ".into(), "o/r".into()]);
        assert!(q.contains("user:owner"));
        assert!(q.contains("repo:o/r"));
        // Blank slug is skipped, not emitted as a bare qualifier.
        assert!(!q.contains("repo: "));
    }

    #[test]
    fn build_query_no_repos_is_global() {
        let q = build_query(&[]);
        assert_eq!(
            q,
            "is:pr is:open (author:@me OR review-requested:@me OR team-review-requested:@me)"
        );
    }

    #[test]
    fn search_hit_parses_repo_and_number() {
        let item: serde_json::Value = serde_json::from_str(
            r#"{ "number": 7, "repository_url": "https://api.github.com/repos/o/r" }"#,
        )
        .unwrap();
        assert_eq!(search_hit_repo_number(&item), Some(("o/r".to_string(), 7)));
    }

    #[test]
    fn search_hit_rejects_malformed() {
        let no_number: serde_json::Value =
            serde_json::from_str(r#"{ "repository_url": "https://api.github.com/repos/o/r" }"#)
                .unwrap();
        assert!(search_hit_repo_number(&no_number).is_none());
        let bad_repo: serde_json::Value = serde_json::from_str(
            r#"{ "number": 1, "repository_url": "https://x/repos/justowner" }"#,
        )
        .unwrap();
        assert!(search_hit_repo_number(&bad_repo).is_none());
    }

    #[test]
    fn parse_pull_object_extracts_fields() {
        let pr: serde_json::Value = serde_json::from_str(
            r#"{
                "node_id": "PR_n", "number": 9, "title": "t", "state": "open",
                "merged": false, "draft": false, "mergeable_state": "clean",
                "user": { "login": "me" },
                "head": { "sha": "abc", "ref": "feat" },
                "base": { "ref": "main" },
                "created_at": "2026-06-17T00:00:00Z",
                "updated_at": "2026-06-17T01:00:00Z"
            }"#,
        )
        .unwrap();
        let p = parse_pull_object("o/r", &pr).unwrap();
        assert_eq!(p.repo, "o/r");
        assert_eq!(p.number, 9);
        assert_eq!(p.head_sha, "abc");
        assert_eq!(p.mergeable_state.as_deref(), Some("clean"));
    }

    /// A fake [`SearchClient`] driving the reconcile branch logic without a
    /// network: first call returns a fresh result + `ETag`; an `If-None-Match`
    /// replay returns `NotModified`.
    struct FakeClient;

    #[async_trait::async_trait]
    impl SearchClient for FakeClient {
        async fn search(
            &self,
            _credential: &str,
            _query: &str,
            etag: Option<&str>,
        ) -> anyhow::Result<SearchOutcome> {
            if etag == Some("etag-v1") {
                return Ok(SearchOutcome::NotModified);
            }
            Ok(SearchOutcome::Modified { pulls: vec![], etag: Some("etag-v1".to_string()) })
        }
    }

    #[tokio::test]
    async fn etag_is_cached_and_replayed_as_not_modified() {
        // Drive the conditional-request branch with no DB: an empty pull list
        // means `reconcile_connector`'s upsert path is a no-op, exercising only
        // the ETag cache + 304 logic.
        let etags: EtagCache = Arc::new(Mutex::new(HashMap::new()));
        let client = FakeClient;
        let cid = Uuid::nil();
        let q = build_query(&["o/r".into()]);

        // We cannot call reconcile_connector without a GithubState/pool, so test
        // the ETag cache transition directly against the client + cache, which
        // is the network-free contract reconcile_connector relies on.
        let prior = etags.lock().await.get(&(cid, q.clone())).cloned();
        let first = client.search("cred", &q, prior.as_deref()).await.unwrap();
        match first {
            SearchOutcome::Modified { etag: Some(tag), .. } => {
                etags.lock().await.insert((cid, q.clone()), tag);
            }
            _ => panic!("expected a fresh Modified result on first search"),
        }

        let prior = etags.lock().await.get(&(cid, q.clone())).cloned();
        assert_eq!(prior.as_deref(), Some("etag-v1"));
        let second = client.search("cred", &q, prior.as_deref()).await.unwrap();
        assert!(matches!(second, SearchOutcome::NotModified));
    }
}
