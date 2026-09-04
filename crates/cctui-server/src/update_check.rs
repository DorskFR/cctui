//! Background "is there a newer cctui?" probe.
//!
//! Polls the GitHub releases API for the upstream repository at a slow cadence
//! and remembers the newest published tag. `/api/v1/version` surfaces it as
//! `latest_version` / `latest_url` **only when it is strictly newer** than the
//! running build, so the webui can show an update hint next to the server
//! version without any client-side network access.
//!
//! The same probe keeps the release notes of every release published since
//! the running build (newest first, capped at [`MAX_NOTES`]), so
//! `GET /version/changelog` can show what an update brings without the
//! browser ever talking to GitHub itself.
//!
//! `POST /version/refresh` runs the same probe on demand (the webui's "check
//! now" button in Settings), so a fresh answer doesn't require waiting out the
//! interval or restarting the server; back-to-back clicks inside
//! [`MANUAL_COOLDOWN`] reuse the last answer instead of hammering GitHub.
//!
//! Opt-out: `CCTUI_UPDATE_CHECK=0` (or `false`/`off`) disables the probe; an
//! air-gapped deployment then simply never reports a newer version.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::RwLock;

const RELEASES_URL: &str = "https://api.github.com/repos/DorskFR/cctui/releases?per_page=30";
/// How many releases newer than the running build are kept for the changelog.
pub const MAX_NOTES: usize = 10;
const INITIAL_DELAY: Duration = Duration::from_secs(15);
const INTERVAL: Duration = Duration::from_hours(6);
/// Shortest gap between two probes; an on-demand refresh inside this window
/// serves the answer already recorded.
pub const MANUAL_COOLDOWN: Duration = Duration::from_mins(1);

/// The newest release seen upstream, only kept when newer than [`CURRENT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatestRelease {
    pub version: String,
    pub url: String,
}

/// One release published since the running build: tag, page and Markdown notes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ReleaseNote {
    pub version: String,
    pub url: String,
    /// The release body as written on GitHub (Markdown); empty when the
    /// release has no description.
    pub body: String,
    /// ISO-8601 publication time as GitHub reports it, `null` for drafts.
    pub published_at: Option<String>,
}

#[derive(Default)]
pub struct UpdateCheck {
    latest: RwLock<Option<LatestRelease>>,
    /// Releases strictly newer than the running build, newest first.
    notes: RwLock<Vec<ReleaseNote>>,
    /// When the last probe answered — gates on-demand refreshes.
    probed_at: RwLock<Option<Instant>>,
}

const CURRENT: &str = env!("CARGO_PKG_VERSION");

impl UpdateCheck {
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The newest upstream release if it is strictly newer than this build.
    pub async fn newer(&self) -> Option<LatestRelease> {
        self.latest.read().await.clone()
    }

    /// Release notes of every release newer than this build, newest first.
    pub async fn notes(&self) -> Vec<ReleaseNote> {
        self.notes.read().await.clone()
    }

    /// Record a single candidate; kept only when strictly newer than the
    /// running build. Convenience over [`Self::record_all`] for one release
    /// without notes.
    #[cfg(test)]
    pub async fn record(&self, tag: &str, url: String) {
        self.record_all(vec![GithubRelease {
            tag_name: tag.to_owned(),
            html_url: url,
            body: None,
            published_at: None,
            draft: false,
            prerelease: false,
        }])
        .await;
    }

    /// Record a probe answer: the newest published release becomes
    /// `latest` when strictly newer than the running build, and every newer
    /// release is kept as a note (newest first, at most [`MAX_NOTES`]).
    pub async fn record_all(&self, releases: Vec<GithubRelease>) {
        let notes = newer_notes(CURRENT, releases);
        let latest =
            notes.first().map(|n| LatestRelease { version: n.version.clone(), url: n.url.clone() });
        *self.latest.write().await = latest;
        *self.notes.write().await = notes;
        *self.probed_at.write().await = Some(Instant::now());
    }

    /// Probe now unless the last answer is younger than [`MANUAL_COOLDOWN`].
    ///
    /// `Ok(true)` means GitHub was queried, `Ok(false)` that the recorded
    /// answer was reused; either way [`Self::newer`] is current afterwards.
    pub async fn refresh(&self, http: &reqwest::Client) -> Result<bool, String> {
        if self.probed_at.read().await.is_some_and(|at| at.elapsed() < MANUAL_COOLDOWN) {
            return Ok(false);
        }
        let releases = fetch_releases(http).await?;
        self.record_all(releases).await;
        Ok(true)
    }
}

/// `CCTUI_UPDATE_CHECK` — enabled unless explicitly turned off.
#[must_use]
pub fn enabled_from_env() -> bool {
    !matches!(
        std::env::var("CCTUI_UPDATE_CHECK")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

/// The subset of a GitHub release the probe reads.
#[derive(Deserialize, Debug, Clone)]
pub struct GithubRelease {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

/// Published (non-draft, non-prerelease) releases strictly newer than
/// `current`, sorted newest first and capped at [`MAX_NOTES`]. Pure so the
/// filtering is unit-testable without a network.
#[must_use]
pub fn newer_notes(current: &str, releases: Vec<GithubRelease>) -> Vec<ReleaseNote> {
    let mut notes: Vec<ReleaseNote> = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let version = r.tag_name.trim().trim_start_matches('v').to_owned();
            is_newer(current, &version).then(|| ReleaseNote {
                version,
                url: r.html_url,
                body: r.body.unwrap_or_default().trim().to_owned(),
                published_at: r.published_at,
            })
        })
        .collect();
    notes.sort_by(|a, b| {
        if is_newer(&a.version, &b.version) {
            std::cmp::Ordering::Greater
        } else if is_newer(&b.version, &a.version) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    });
    notes.dedup_by(|a, b| a.version == b.version);
    notes.truncate(MAX_NOTES);
    notes
}

/// Long-running probe; spawn once per process.
pub async fn task(check: Arc<UpdateCheck>, http: reqwest::Client) {
    tokio::time::sleep(INITIAL_DELAY).await;
    loop {
        match fetch_releases(&http).await {
            Ok(releases) => {
                check.record_all(releases).await;
                if let Some(l) = check.newer().await {
                    tracing::info!(current = CURRENT, latest = %l.version, "newer cctui release available");
                } else {
                    tracing::debug!(current = CURRENT, "cctui is up to date");
                }
            }
            Err(e) => tracing::debug!("update check failed: {e}"),
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

async fn fetch_releases(http: &reqwest::Client) -> Result<Vec<GithubRelease>, String> {
    let resp = http
        .get(RELEASES_URL)
        .header("User-Agent", format!("cctui/{CURRENT}"))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("github returned {}", resp.status()));
    }
    resp.json::<Vec<GithubRelease>>().await.map_err(|e| e.to_string())
}

/// Parse `a.b.c[-pre]` into numeric components; anything unparsable is 0.
fn parts(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Strictly newer, comparing dotted numeric components (missing = 0).
#[must_use]
pub fn is_newer(current: &str, candidate: &str) -> bool {
    let (cur, cand) = (parts(current), parts(candidate));
    let len = cur.len().max(cand.len());
    for idx in 0..len {
        let have = cur.get(idx).copied().unwrap_or(0);
        let want = cand.get(idx).copied().unwrap_or(0);
        if want != have {
            return want > have;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_dotted_versions() {
        assert!(is_newer("0.7.296", "0.7.297"));
        assert!(is_newer("0.7.296", "v0.8.0"));
        assert!(is_newer("0.7.296", "1.0"));
        assert!(!is_newer("0.7.296", "0.7.296"));
        assert!(!is_newer("0.7.296", "0.7.295"));
        assert!(!is_newer("0.7.296", "v0.7.296"));
        assert!(!is_newer("0.7.296", "garbage"));
        assert!(is_newer("0.7.296", "0.7.297-rc1"));
        assert!(!is_newer("0.7.10", "0.7.9"));
    }

    #[tokio::test]
    async fn record_keeps_only_newer() {
        let c = UpdateCheck::default();
        c.record("v0.0.1", "u".into()).await;
        assert_eq!(c.newer().await, None);
        c.record("v999.0.0", "u".into()).await;
        assert_eq!(c.newer().await.map(|l| l.version), Some("999.0.0".into()));
        // A later, older answer clears it (release rolled back / API glitch).
        c.record("v0.0.1", "u".into()).await;
        assert_eq!(c.newer().await, None);
    }

    fn rel(tag: &str, body: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            html_url: format!("https://example/{tag}"),
            body: Some(body.into()),
            published_at: Some("2026-09-04T00:00:00Z".into()),
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn newer_notes_filters_sorts_and_caps() {
        let mut releases = vec![
            rel("v0.7.300", "old"),
            rel("v0.7.302", "b"),
            rel("v0.7.305", " c \n"),
            rel("v0.7.303", "a"),
            GithubRelease { draft: true, ..rel("v0.7.306", "draft") },
            GithubRelease { prerelease: true, ..rel("v0.7.307", "rc") },
            rel("v0.7.303", "dup"),
        ];
        releases.push(GithubRelease { body: None, ..rel("v0.7.304", "") });
        let notes = newer_notes("0.7.301", releases);
        let versions: Vec<&str> = notes.iter().map(|n| n.version.as_str()).collect();
        assert_eq!(versions, ["0.7.305", "0.7.304", "0.7.303", "0.7.302"]);
        assert_eq!(notes[0].body, "c");
        assert_eq!(notes[1].body, "");

        let many: Vec<GithubRelease> =
            (1..=MAX_NOTES + 5).map(|i| rel(&format!("v1.0.{i}"), "x")).collect();
        assert_eq!(newer_notes("0.0.1", many).len(), MAX_NOTES);
        assert!(newer_notes("999.0.0", vec![rel("v1.0.0", "x")]).is_empty());
    }

    #[tokio::test]
    async fn record_all_sets_latest_from_the_newest_note() {
        let c = UpdateCheck::default();
        c.record_all(vec![rel("v999.0.1", "n1"), rel("v999.0.2", "n2"), rel("v0.0.1", "old")])
            .await;
        assert_eq!(c.newer().await.map(|l| l.version), Some("999.0.2".into()));
        assert_eq!(c.notes().await.len(), 2);
        c.record_all(vec![rel("v0.0.1", "old")]).await;
        assert_eq!(c.newer().await, None);
        assert!(c.notes().await.is_empty());
    }

    #[tokio::test]
    async fn manual_refresh_reuses_a_fresh_answer() {
        let c = UpdateCheck::default();
        // A recorded answer stamps the probe clock, so an immediate on-demand
        // refresh serves it instead of reaching for the network.
        c.record("v999.0.0", "u".into()).await;
        assert_eq!(c.refresh(&reqwest::Client::new()).await, Ok(false));
        assert_eq!(c.newer().await.map(|l| l.version), Some("999.0.0".into()));
    }
}
