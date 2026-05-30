use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub external_url: String,
    /// How long a session may sit without activity before the reaper
    /// demotes it from `Active` to `Inactive`. The old
    /// `CCTUI_HEARTBEAT_TIMEOUT` env var is still accepted for
    /// back-compat with existing deployments.
    pub inactive_after_secs: u64,
    /// How long a session may sit without a heartbeat before the reaper
    /// auto-archives it (hides it from the default list). Configured in
    /// hours via `CCTUI_SESSION_ARCHIVE_TTL_HOURS`; stored as seconds.
    /// `0` disables auto-archiving.
    pub archive_after_secs: u64,
    /// Optional GitHub PAT (read access to the releases repo). When set, the
    /// daemon-binary manifest points clients at this server's proxy endpoint
    /// and the server streams the release asset itself (so a private releases
    /// repo stays private and clients never need a token). When unset, the
    /// manifest falls back to the raw GitHub release URLs and the proxy
    /// 302-redirects there — letting selfupdate degrade gracefully (it fails
    /// for a private repo, which is the intended no-op until a token is set).
    pub github_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env::var("CCTUI_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("CCTUI_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8700),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            external_url: env::var("CCTUI_EXTERNAL_URL")
                .unwrap_or_else(|_| "http://localhost:8700".into()),
            inactive_after_secs: env::var("CCTUI_INACTIVE_AFTER")
                .or_else(|_| env::var("CCTUI_HEARTBEAT_TIMEOUT"))
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            archive_after_secs: env::var("CCTUI_SESSION_ARCHIVE_TTL_HOURS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(24 * 60 * 60, |hours| hours * 60 * 60),
            github_token: env::var("CCTUI_GITHUB_TOKEN")
                .or_else(|_| env::var("GH_TOKEN"))
                .ok()
                .filter(|s| !s.trim().is_empty()),
        }
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn agent_tokens() -> Vec<String> {
        env::var("CCTUI_AGENT_TOKENS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect()
    }

    pub fn admin_tokens() -> Vec<String> {
        env::var("CCTUI_ADMIN_TOKENS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect()
    }
}
