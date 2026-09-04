//! Background "is there a newer cctui?" probe.
//!
//! Polls the GitHub releases API for the upstream repository at a slow cadence
//! and remembers the newest published tag. `/api/v1/version` surfaces it as
//! `latest_version` / `latest_url` **only when it is strictly newer** than the
//! running build, so the webui can show an update hint next to the server
//! version without any client-side network access.
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

const RELEASES_URL: &str = "https://api.github.com/repos/DorskFR/cctui/releases/latest";
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

#[derive(Default)]
pub struct UpdateCheck {
    latest: RwLock<Option<LatestRelease>>,
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

    /// Record a candidate; kept only when strictly newer than the running build.
    pub async fn record(&self, tag: &str, url: String) {
        let version = tag.trim().trim_start_matches('v').to_owned();
        let newer = is_newer(CURRENT, &version);
        {
            let mut slot = self.latest.write().await;
            *slot = if newer { Some(LatestRelease { version, url }) } else { None };
        }
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
        let rel = fetch_latest(http).await?;
        self.record(&rel.tag_name, rel.html_url).await;
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

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

/// Long-running probe; spawn once per process.
pub async fn task(check: Arc<UpdateCheck>, http: reqwest::Client) {
    tokio::time::sleep(INITIAL_DELAY).await;
    loop {
        match fetch_latest(&http).await {
            Ok(rel) => {
                check.record(&rel.tag_name, rel.html_url).await;
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

async fn fetch_latest(http: &reqwest::Client) -> Result<GithubRelease, String> {
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
    resp.json::<GithubRelease>().await.map_err(|e| e.to_string())
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
