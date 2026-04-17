//! Runtime configuration — `~/.config/cctui/machine.json` with env overrides.
//! Mirrors `channel/src/types.ts:loadConfig`.

use cctui_proto::identity;

#[derive(Debug, Clone)]
pub struct Config {
    pub server_url: String,
    pub agent_token: String,
}

#[must_use]
pub fn load() -> Config {
    let identity = identity::load_machine();
    let (default_url, default_token) = identity.map_or_else(
        || ("http://localhost:8700".to_string(), "dev-agent".to_string()),
        |id| (id.server_url, id.machine_key),
    );
    Config {
        server_url: std::env::var("CCTUI_URL").unwrap_or(default_url),
        agent_token: std::env::var("CCTUI_AGENT_TOKEN").unwrap_or(default_token),
    }
}
