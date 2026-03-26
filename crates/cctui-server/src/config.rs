use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub external_url: String,
    #[allow(dead_code)]
    pub heartbeat_timeout_secs: u64,
    #[allow(dead_code)]
    pub terminated_timeout_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env::var("CCTUI_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("CCTUI_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8700),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            external_url: env::var("CCTUI_EXTERNAL_URL")
                .unwrap_or_else(|_| "http://localhost:8700".into()),
            heartbeat_timeout_secs: env::var("CCTUI_HEARTBEAT_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            terminated_timeout_secs: env::var("CCTUI_TERMINATED_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
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
