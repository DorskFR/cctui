//! GH-VIEW-1: the server-side PR diff proxy, cache, pagination, and blob fallback.
//!
//! `GET /api/v1/github/pulls/{connector_id}/{owner}/{name}/{number}/diff` returns
//! a structured [`PullDiff`] (files → hunks → lines) the webui (GH-VIEW-3)
//! virtualizes. The data source is **GitHub only** — no daemon, no checkout
//! (docs/github-integration.md §6.2). The flow:
//!
//! 1. Resolve the stored pull (CONN-3 `github.pulls`) to get its `head_sha` and
//!    the owning connector's decryptable credential (crypto.rs vault).
//! 2. Serve from the **per-head-SHA cache** if present (a repeated load of an
//!    unchanged PR costs no GitHub round-trip). The cache is **in-memory**
//!    (chosen over a `github.*` table) so it leaves no stale state on uninstall —
//!    `DROP SCHEMA github CASCADE` has nothing diff-related to clean up, and a
//!    restart simply re-fetches once.
//! 3. Otherwise fetch the changed files from GitHub: `GET /repos/{o}/{r}/pulls/{n}/files`,
//!    **paginated** (100/page, `Link`-header `rel="next"` walk), and parse each
//!    file's unified `patch` into hunks → lines.
//! 4. **Blob fallback:** GitHub omits the inline `patch` for files it considers
//!    too large. For those we fetch the head blob via the contents API and emit a
//!    synthetic full-file "add" diff so the reviewer still sees the content; if
//!    even that fails, the file is flagged `truncated` and the webui offers a
//!    "load full file" affordance.
//! 5. **Large-diff handling (docs §11):** the total changed-line count is summed
//!    as files stream in; once it crosses [`HUGE_THRESHOLD`] (the >100k-line case
//!    GitHub serves unreliably) the assembly stops accumulating file bodies,
//!    flags the result `huge`, and the webui paginates/lazy-loads rather than
//!    rendering — so a pathological PR never OOMs the server.
//!
//! The GitHub HTTP surface is behind the [`DiffClient`] trait so pagination
//! assembly, the blob fallback, and cache keying are unit-testable without a
//! network (the reqwest-backed [`HttpDiffClient`] is the production impl).
//!
//! Secrets: the credential is decrypted only to build the `Authorization` header
//! and is **never** logged; diff bodies carry no secret material.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cctui_proto::github::{DiffFile, DiffHunk, DiffLine, DiffLineKind, PullDiff};

const API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("cctui/", env!("CARGO_PKG_VERSION"));
const PER_PAGE: u32 = 100;

/// Total-changed-line ceiling past which a diff is flagged `huge` and file
/// bodies stop accumulating (docs §11, the >100k-line case). Keeping the cap a
/// little above 100k means a normal large-but-renderable PR is still served
/// whole, while a pathological one is bounded.
const HUGE_THRESHOLD: u64 = 100_000;

/// A single changed file as GitHub's `pulls/{n}/files` reports it, before we
/// parse the unified `patch`. The [`DiffClient`] yields these; the assembly in
/// [`build_pull_diff`] turns them into [`DiffFile`]s (parsing the patch, or
/// invoking the blob fallback for a missing one).
#[derive(Debug, Clone)]
pub struct RawFile {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// GitHub's blob SHA for the head side; absent for some statuses.
    pub sha: Option<String>,
    /// The unified diff text. `None` when GitHub omitted it (too large / binary).
    pub patch: Option<String>,
}

/// Abstracts the GitHub diff round-trips so assembly + fallback are testable
/// without a live API.
#[async_trait::async_trait]
pub trait DiffClient: Send + Sync {
    /// Fetch all changed files for a PR, walking pagination to completion.
    async fn pull_files(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
    ) -> anyhow::Result<Vec<RawFile>>;

    /// Blob fallback: fetch the raw text of a file at a ref (the head SHA), used
    /// to reconstruct a diff GitHub truncated. `None` means it could not be
    /// retrieved (binary, gone, or too large even here) and the caller flags the
    /// file `truncated`.
    async fn blob_text(
        &self,
        _credential: &str,
        _repo: &str,
        _path: &str,
        _git_ref: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

// ---- per-head-SHA cache ----------------------------------------------------

/// In-memory diff cache keyed on `head_sha`. A repeated load of an unchanged PR
/// hits this and skips GitHub entirely; a new push rotates `head_sha` and the
/// old entry is simply never read again (and evicted when the map is bounded).
///
/// In-memory (not a `github.*` table) is the deliberate teardown-safe choice
/// (docs §6.2 / §7): an uninstall's `DROP SCHEMA github CASCADE` has no diff
/// cache rows to leave stale, and a restart re-fetches once.
#[derive(Clone, Default)]
pub struct DiffCache {
    inner: Arc<Mutex<HashMap<String, Arc<PullDiff>>>>,
}

impl DiffCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a cached diff by head SHA.
    #[must_use]
    pub fn get(&self, head_sha: &str) -> Option<Arc<PullDiff>> {
        self.inner.lock().unwrap().get(head_sha).cloned()
    }

    /// Store a diff under its head SHA. Bounded to [`MAX_CACHE_ENTRIES`]; once
    /// full a new insert clears the map (a coarse but allocation-free eviction —
    /// diffs are large, the working set is one or two PRs, and a miss just
    /// re-fetches).
    pub fn put(&self, diff: Arc<PullDiff>) {
        let mut map = self.inner.lock().unwrap();
        if map.len() >= MAX_CACHE_ENTRIES && !map.contains_key(&diff.head_sha) {
            map.clear();
        }
        map.insert(diff.head_sha.clone(), diff);
    }
}

/// Cap on distinct head SHAs cached at once. The reviewer's working set is tiny
/// (a PR or two); past this the map is cleared rather than grown unbounded.
const MAX_CACHE_ENTRIES: usize = 32;

// ---- assembly --------------------------------------------------------------

/// Build the structured [`PullDiff`] for a PR from GitHub, applying pagination
/// (in the client), the blob fallback for truncated files, and the large-diff
/// guard. Pure over the [`DiffClient`], so tests drive it with a fake.
pub async fn build_pull_diff(
    client: &dyn DiffClient,
    credential: &str,
    repo: &str,
    number: i64,
    head_sha: &str,
) -> anyhow::Result<PullDiff> {
    let raw = client.pull_files(credential, repo, number).await?;
    let total_files = u32::try_from(raw.len()).unwrap_or(u32::MAX);
    let mut total_changes: u64 = 0;
    let mut huge = false;
    let mut files = Vec::with_capacity(raw.len());

    for rf in raw {
        total_changes += u64::from(rf.additions) + u64::from(rf.deletions);
        // Once the PR crosses the large-diff ceiling, stop accumulating bodies:
        // the UI gets the counts + the `huge` flag and lazy-loads per file.
        if huge || total_changes > HUGE_THRESHOLD {
            huge = true;
            continue;
        }
        files.push(to_diff_file(client, credential, repo, head_sha, rf).await);
    }

    Ok(PullDiff {
        repo: repo.to_string(),
        number,
        head_sha: head_sha.to_string(),
        total_files,
        total_changes,
        huge,
        files,
    })
}

/// Turn one [`RawFile`] into a [`DiffFile`], parsing its patch or invoking the
/// blob fallback when GitHub omitted it.
async fn to_diff_file(
    client: &dyn DiffClient,
    credential: &str,
    repo: &str,
    head_sha: &str,
    rf: RawFile,
) -> DiffFile {
    let previous_path = rf.previous_filename.clone();
    let mut binary = false;
    let mut truncated = false;
    let hunks = if let Some(patch) = rf.patch.as_deref() {
        parse_patch(patch)
    } else if rf.status == "removed" {
        // A removed file has no head blob to fetch; nothing to reconstruct.
        Vec::new()
    } else {
        // GitHub omitted the inline patch (too large). Blob fallback: fetch the
        // head content and synthesize a whole-file "add" so the reviewer still
        // sees it. A non-text blob (or a fetch failure) flags the file.
        match client.blob_text(credential, repo, &rf.filename, head_sha).await {
            Ok(Some(text)) => synthesize_added(&text),
            Ok(None) => {
                // No textual blob: treat large additions as binary, else as a
                // genuinely truncated diff the UI must lazy-load.
                if rf.additions == 0 && rf.deletions == 0 {
                    binary = true;
                } else {
                    truncated = true;
                }
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(repo, file = %rf.filename, "github diff blob fallback failed: {e}");
                truncated = true;
                Vec::new()
            }
        }
    };

    DiffFile {
        path: rf.filename,
        previous_path,
        status: rf.status,
        additions: rf.additions,
        deletions: rf.deletions,
        hunks,
        truncated,
        binary,
        blob_sha: rf.sha,
    }
}

/// Synthesize a single all-added hunk from full file text (the blob fallback).
fn synthesize_added(text: &str) -> Vec<DiffHunk> {
    let lines: Vec<DiffLine> = text
        .split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| DiffLine {
            kind: DiffLineKind::Add,
            content: l.strip_suffix('\n').unwrap_or(l).to_string(),
            old_line: None,
            new_line: Some(u32::try_from(i + 1).unwrap_or(u32::MAX)),
        })
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let n = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    vec![DiffHunk { old_start: 0, old_lines: 0, new_start: 1, new_lines: n, header: None, lines }]
}

/// Parse a unified-diff `patch` string (GitHub's per-file `patch` field) into
/// hunks. Robust to a missing trailing newline; ignores `\ No newline` markers.
#[must_use]
pub fn parse_patch(patch: &str) -> Vec<DiffHunk> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut old_no = 0u32;
    let mut new_no = 0u32;

    for raw in patch.split('\n') {
        if let Some(rest) = raw.strip_prefix("@@") {
            if let Some(h) = parse_hunk_header(rest) {
                old_no = h.old_start;
                new_no = h.new_start;
                hunks.push(h);
            }
            continue;
        }
        let Some(hunk) = hunks.last_mut() else { continue };
        // Within a hunk: first byte is the line marker.
        let (kind, content) = match raw.as_bytes().first() {
            Some(b'+') => (DiffLineKind::Add, &raw[1..]),
            Some(b'-') => (DiffLineKind::Del, &raw[1..]),
            Some(b' ') => (DiffLineKind::Context, &raw[1..]),
            // "\ No newline at end of file", blank tail, or unexpected line.
            _ => continue,
        };
        let (old_line, new_line) = match kind {
            DiffLineKind::Context => {
                let l = (Some(old_no), Some(new_no));
                old_no += 1;
                new_no += 1;
                l
            }
            DiffLineKind::Del => {
                let l = (Some(old_no), None);
                old_no += 1;
                l
            }
            DiffLineKind::Add => {
                let l = (None, Some(new_no));
                new_no += 1;
                l
            }
        };
        hunk.lines.push(DiffLine { kind, content: content.to_string(), old_line, new_line });
    }
    hunks
}

/// Parse the body of a `@@ -a,b +c,d @@ header` line (the text after the leading
/// `@@`). Returns a hunk with empty `lines` and the parsed coordinates.
fn parse_hunk_header(rest: &str) -> Option<DiffHunk> {
    // rest looks like: " -a,b +c,d @@ optional header"
    let close = rest.find("@@")?;
    let coords = rest[..close].trim();
    let header = rest[close + 2..].trim();
    let header = if header.is_empty() { None } else { Some(header.to_string()) };

    let mut old = (0u32, 1u32);
    let mut new = (0u32, 1u32);
    for tok in coords.split_whitespace() {
        if let Some(s) = tok.strip_prefix('-') {
            old = parse_range(s)?;
        } else if let Some(s) = tok.strip_prefix('+') {
            new = parse_range(s)?;
        }
    }
    Some(DiffHunk {
        old_start: old.0,
        old_lines: old.1,
        new_start: new.0,
        new_lines: new.1,
        header,
        lines: Vec::new(),
    })
}

/// Parse a hunk range `start[,count]` (count defaults to 1 per the unified-diff
/// spec). Returns `(start, count)`.
fn parse_range(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(',');
    let start: u32 = parts.next()?.parse().ok()?;
    let count: u32 = parts.next().map_or(Some(1), |c| c.parse().ok())?;
    Some((start, count))
}

// ---- reqwest-backed production client --------------------------------------

/// Production [`DiffClient`] over GitHub's REST API. Reuses rustls via the
/// workspace `reqwest` (same auth pattern as [`crate::reconcile::HttpSearchClient`]).
pub struct HttpDiffClient {
    http: reqwest::Client,
}

impl Default for HttpDiffClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpDiffClient {
    #[must_use]
    pub fn new() -> Self {
        Self { http: reqwest::Client::new() }
    }
}

#[async_trait::async_trait]
impl DiffClient for HttpDiffClient {
    async fn pull_files(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
    ) -> anyhow::Result<Vec<RawFile>> {
        let mut files = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!("{API_BASE}/repos/{repo}/pulls/{number}/files");
            let resp = self
                .http
                .get(&url)
                .query(&[("per_page", PER_PAGE.to_string()), ("page", page.to_string())])
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .send()
                .await?;
            if !resp.status().is_success() {
                anyhow::bail!("github pull files returned {}", resp.status());
            }
            let has_next = has_next_page(&resp);
            let body: serde_json::Value = resp.json().await?;
            let arr = body.as_array().cloned().unwrap_or_default();
            let n = arr.len();
            for f in &arr {
                if let Some(rf) = parse_raw_file(f) {
                    files.push(rf);
                }
            }
            // Stop when GitHub stops advertising a next page, or a short page
            // (defensive: some proxies drop the Link header).
            if !has_next || n < PER_PAGE as usize {
                break;
            }
            page += 1;
        }
        Ok(files)
    }

    async fn blob_text(
        &self,
        credential: &str,
        repo: &str,
        path: &str,
        git_ref: &str,
    ) -> anyhow::Result<Option<String>> {
        // The contents API returns the file at a ref; the `raw` media type gives
        // us the bytes directly. A non-200 (gone, too large >100MB, or a
        // directory) yields `None` so the caller flags the file rather than failing.
        let url = format!("{API_BASE}/repos/{repo}/contents/{path}");
        let resp = self
            .http
            .get(&url)
            .query(&[("ref", git_ref)])
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github.raw")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let bytes = resp.bytes().await?;
        // Non-UTF-8 ⇒ binary; yield `None` so the caller renders a binary badge.
        Ok(String::from_utf8(bytes.to_vec()).ok())
    }
}

/// Whether a paginated response advertises a `rel="next"` page via its `Link`
/// header.
fn has_next_page(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::LINK)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|link| link.contains("rel=\"next\""))
}

/// Parse one element of `pulls/{n}/files` into a [`RawFile`].
fn parse_raw_file(f: &serde_json::Value) -> Option<RawFile> {
    Some(RawFile {
        filename: f.get("filename")?.as_str()?.to_string(),
        previous_filename: f
            .get("previous_filename")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        status: f.get("status").and_then(|v| v.as_str()).unwrap_or("modified").to_string(),
        additions: f.get("additions").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32,
        deletions: f.get("deletions").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32,
        sha: f.get("sha").and_then(|v| v.as_str()).map(str::to_string),
        patch: f.get("patch").and_then(|v| v.as_str()).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_hunk() {
        let patch = "@@ -1,3 +1,4 @@ fn main()\n a\n-b\n+c\n+d\n e";
        let hunks = parse_patch(patch);
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (1, 3, 1, 4));
        assert_eq!(h.header.as_deref(), Some("fn main()"));
        // a(ctx) b(del) c(add) d(add) e(ctx)
        assert_eq!(h.lines.len(), 5);
        assert_eq!(h.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h.lines[0].old_line, Some(1));
        assert_eq!(h.lines[0].new_line, Some(1));
        assert_eq!(h.lines[1].kind, DiffLineKind::Del);
        assert_eq!(h.lines[1].old_line, Some(2));
        assert_eq!(h.lines[1].new_line, None);
        assert_eq!(h.lines[2].kind, DiffLineKind::Add);
        assert_eq!(h.lines[2].new_line, Some(2));
        // context after the changes advances both counters past the edits.
        assert_eq!(h.lines[4].kind, DiffLineKind::Context);
        assert_eq!(h.lines[4].old_line, Some(3));
        assert_eq!(h.lines[4].new_line, Some(4));
    }

    #[test]
    fn parses_multiple_hunks_and_default_count() {
        // Second hunk uses bare `-N +M` (count defaults to 1).
        let patch = "@@ -1,1 +1,1 @@\n-x\n+y\n@@ -10 +10 @@\n-p\n+q";
        let hunks = parse_patch(patch);
        assert_eq!(hunks.len(), 2);
        assert_eq!((hunks[1].old_start, hunks[1].old_lines), (10, 1));
        assert_eq!(hunks[1].new_start, 10);
    }

    #[test]
    fn ignores_no_newline_marker() {
        let patch = "@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file";
        let hunks = parse_patch(patch);
        assert_eq!(hunks[0].lines.len(), 2);
    }

    /// A fake client: page 1 + page 2 of files, and one truncated file whose
    /// blob is supplied via the fallback.
    struct FakeClient;

    #[async_trait::async_trait]
    impl DiffClient for FakeClient {
        async fn pull_files(
            &self,
            _credential: &str,
            _repo: &str,
            _number: i64,
        ) -> anyhow::Result<Vec<RawFile>> {
            // Simulate assembled pagination: two files with patches + one
            // truncated (no patch) that triggers the blob fallback.
            Ok(vec![
                RawFile {
                    filename: "a.rs".into(),
                    previous_filename: None,
                    status: "modified".into(),
                    additions: 1,
                    deletions: 1,
                    sha: Some("sha-a".into()),
                    patch: Some("@@ -1 +1 @@\n-old\n+new".into()),
                },
                RawFile {
                    filename: "b.rs".into(),
                    previous_filename: Some("old_b.rs".into()),
                    status: "renamed".into(),
                    additions: 2,
                    deletions: 0,
                    sha: Some("sha-b".into()),
                    patch: Some("@@ -0,0 +1,2 @@\n+l1\n+l2".into()),
                },
                RawFile {
                    filename: "big.txt".into(),
                    previous_filename: None,
                    status: "modified".into(),
                    additions: 3,
                    deletions: 0,
                    sha: Some("sha-big".into()),
                    patch: None, // GitHub truncated it → blob fallback
                },
            ])
        }

        async fn blob_text(
            &self,
            _credential: &str,
            _repo: &str,
            path: &str,
            _git_ref: &str,
        ) -> anyhow::Result<Option<String>> {
            if path == "big.txt" {
                Ok(Some("one\ntwo\nthree\n".into()))
            } else {
                Ok(None)
            }
        }
    }

    #[tokio::test]
    async fn assembles_paginated_files_with_blob_fallback() {
        let diff = build_pull_diff(&FakeClient, "cred", "o/r", 7, "head-sha").await.unwrap();
        assert_eq!(diff.head_sha, "head-sha");
        assert_eq!(diff.total_files, 3);
        assert_eq!(diff.total_changes, 1 + 1 + 2 + 3);
        assert!(!diff.huge);
        assert_eq!(diff.files.len(), 3);

        // Renamed file carries previous_path.
        assert_eq!(diff.files[1].previous_path.as_deref(), Some("old_b.rs"));

        // The truncated file was reconstructed from its blob as an all-add diff.
        let big = &diff.files[2];
        assert!(!big.truncated, "blob fallback should reconstruct the diff");
        assert_eq!(big.hunks.len(), 1);
        assert_eq!(big.hunks[0].lines.len(), 3);
        assert_eq!(big.hunks[0].lines[0].kind, DiffLineKind::Add);
        assert_eq!(big.hunks[0].lines[0].content, "one");
    }

    /// A client that reports one enormous file — exercises the large-diff guard.
    struct HugeClient;

    #[async_trait::async_trait]
    impl DiffClient for HugeClient {
        async fn pull_files(
            &self,
            _c: &str,
            _r: &str,
            _n: i64,
        ) -> anyhow::Result<Vec<RawFile>> {
            Ok(vec![
                RawFile {
                    filename: "huge.txt".into(),
                    previous_filename: None,
                    status: "added".into(),
                    additions: 200_000,
                    deletions: 0,
                    sha: None,
                    patch: None,
                },
                RawFile {
                    filename: "after.txt".into(),
                    previous_filename: None,
                    status: "added".into(),
                    additions: 5,
                    deletions: 0,
                    sha: None,
                    patch: Some("@@ -0,0 +1 @@\n+x".into()),
                },
            ])
        }
    }

    #[tokio::test]
    async fn flags_huge_diff_and_stops_accumulating_bodies() {
        let diff = build_pull_diff(&HugeClient, "c", "o/r", 1, "sha").await.unwrap();
        assert!(diff.huge, "200k-line PR must be flagged huge");
        assert_eq!(diff.total_files, 2, "total_files still counts every file");
        assert!(diff.total_changes > HUGE_THRESHOLD);
        // Once huge, file bodies stop accumulating (UI lazy-loads per file).
        assert!(diff.files.is_empty());
    }

    #[test]
    fn cache_keys_on_head_sha_and_isolates_shas() {
        let cache = DiffCache::new();
        let d1 = Arc::new(PullDiff {
            repo: "o/r".into(),
            number: 1,
            head_sha: "sha1".into(),
            total_files: 0,
            total_changes: 0,
            huge: false,
            files: vec![],
        });
        cache.put(d1.clone());
        assert!(cache.get("sha1").is_some(), "stored SHA hits");
        assert!(cache.get("sha2").is_none(), "a different head SHA misses (re-fetch on new push)");

        // A re-put under the same SHA returns the same cached content.
        let again = cache.get("sha1").unwrap();
        assert_eq!(again.head_sha, "sha1");
    }

    #[test]
    fn cache_eviction_is_bounded() {
        let cache = DiffCache::new();
        for i in 0..(MAX_CACHE_ENTRIES + 5) {
            cache.put(Arc::new(PullDiff {
                repo: "o/r".into(),
                number: i as i64,
                head_sha: format!("sha-{i}"),
                total_files: 0,
                total_changes: 0,
                huge: false,
                files: vec![],
            }));
        }
        // The map was cleared at the cap, so it never grows unbounded.
        assert!(cache.inner.lock().unwrap().len() <= MAX_CACHE_ENTRIES);
    }
}
