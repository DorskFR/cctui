use std::env;

/// One external dispatcher registration, parsed from `CCTUI_HTTP_DISPATCHERS`
/// (a JSON array of these). `token`, when set, is sent as a bearer secret.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HttpDispatcherConfig {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
}

/// One dispatcher registration, parsed from `CCTUI_DISPATCHERS` — a JSON array
/// of `{ "kind": "http"|"kube"|"docker", ... }` (CCT-234). Supersedes the
/// http-only `CCTUI_HTTP_DISPATCHERS` (still parsed for back-compat); both lists
/// are merged at startup. `kind` is the discriminant.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DispatcherConfig {
    Http(HttpDispatcherConfig),
    Kube(crate::dispatchers::kube::KubeDispatcherConfig),
    Docker(crate::dispatchers::docker::DockerDispatcherConfig),
}

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
    /// External dispatcher registrations, parsed from `CCTUI_HTTP_DISPATCHERS`.
    pub http_dispatchers: Vec<HttpDispatcherConfig>,
    /// Native + http dispatcher registrations, parsed from `CCTUI_DISPATCHERS`
    /// (CCT-234). Merged with `http_dispatchers` at startup.
    pub dispatchers: Vec<DispatcherConfig>,
    /// How long an `ephemeral` (dispatch/worker) machine may go without being
    /// seen before the reaper soft-deletes it — covers pods that die before
    /// self-deenroll (CCT-183). Configured in hours via
    /// `CCTUI_EPHEMERAL_MACHINE_TTL_HOURS`; stored as seconds. `0` disables the
    /// sweep. Persistent machines are never reaped.
    pub ephemeral_machine_ttl_secs: u64,
    /// ntfy access token (`CCTUI_NTFY_TOKEN`, provisioned from vault). Its
    /// presence is the on/off switch for dispatch push notifications: when
    /// unset, `ntfy::notify` is a no-op (CCT-198).
    pub ntfy_token: Option<String>,
    /// ntfy topic URL to POST notifications to (`CCTUI_NTFY_URL`). Defaults to
    /// `https://ntfy.example.internal/cctui-dispatch`.
    pub ntfy_url: String,
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
            http_dispatchers: env::var("CCTUI_HTTP_DISPATCHERS")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    serde_json::from_str(&s).expect("CCTUI_HTTP_DISPATCHERS must be a JSON array")
                })
                .unwrap_or_default(),
            dispatchers: env::var("CCTUI_DISPATCHERS")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| serde_json::from_str(&s).expect("CCTUI_DISPATCHERS must be a JSON array"))
                .unwrap_or_default(),
            ephemeral_machine_ttl_secs: env::var("CCTUI_EPHEMERAL_MACHINE_TTL_HOURS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(2 * 60 * 60, |hours| hours * 60 * 60),
            ntfy_token: env::var("CCTUI_NTFY_TOKEN").ok().filter(|s| !s.trim().is_empty()),
            ntfy_url: env::var("CCTUI_NTFY_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "https://ntfy.example.internal/cctui-dispatch".into()),
        }
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
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
