//! Background "is there a newer cctui?" probe.
//!
//! Polls the GitHub releases API for the upstream repository at a slow cadence
//! and remembers the newest published tag. `/api/v1/version` surfaces it as
//! `latest_version` / `latest_url` **only when it is strictly newer** than the
//! running build, so the webui can show an update hint next to the server
//! version without any client-side network access.
//!
//! Opt-out: `CCTUI_UPDATE_CHECK=0` (or `false`/`off`) disables the probe; an
//! air-gapped deployment then simply never reports a newer version.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::RwLock;

const RELEASES_URL: &str = "https://api.github.com/repos/DorskFR/cctui/releases/latest";
const INITIAL_DELAY: Duration = Duration::from_secs(15);
const INTERVAL: Duration = Duration::from_hours(6);

/// The newest release seen upstream, only kept when newer than [`CURRENT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatestRelease {
    pub version: String,
    pub url: String,
}

#[derive(Default)]
pub struct UpdateCheck {
    latest: RwLock<Option<LatestRelease>>,
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
        let mut slot = self.latest.write().await;
        *slot = if newer { Some(LatestRelease { version, url }) } else { None };
    }
}

/// `CCTUI_UPDATE_CHECK` — enabled unless explicitly turned off.
#[must_use]
pub fn enabled_from_env() -> bool {
    !matches!(
        std::env::var("CCTUI_UPDATE_CHECK").ok().as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref(),
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
}
