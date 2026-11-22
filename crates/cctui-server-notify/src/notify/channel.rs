use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use cctui_server_kernel::config::Config;

pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub update: bool,
    pub retract: bool,
    pub buttons: u8,
    pub needs_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("{0}")]
    Retry(String),
    #[error("{0}")]
    Permanent(String),
    #[error("endpoint gone")]
    Gone,
}

#[derive(Debug, Clone)]
pub struct Rendered {
    pub delivery_id: Uuid,
    pub title: String,
    pub body: Option<String>,
    pub priority: u8,
    pub tags: Option<String>,
    pub link: Option<String>,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub url: String,
    pub press: Press,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Opens the URL; the human logs in there.
    Open,
    /// POSTs to the URL, which carries a single-use action token.
    Post,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRef(pub serde_json::Value);

#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn call_timeout(&self) -> Duration {
        DEFAULT_CALL_TIMEOUT
    }
    async fn send(&self, n: &Rendered, to: &Endpoint) -> Result<ProviderRef, SendError>;
    async fn update(&self, n: &Rendered, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError>;
    async fn retract(&self, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError>;
}

#[derive(Clone, Default)]
pub struct Channels {
    by_id: HashMap<&'static str, Arc<dyn Channel>>,
    config: Option<Arc<Config>>,
}

impl Channels {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut channels = Self { config: Some(Arc::new(config.clone())), ..Self::default() };
        channels.add(Arc::new(super::in_app::InApp));
        channels.add(Arc::new(super::web_push::WebPush::from_config(config)));
        channels.add(Arc::new(super::ntfy_channel::NtfyChannel::from_config(config)));
        if let Some(email) = super::email::EmailChannel::from_config(config) {
            channels.add(Arc::new(email));
        }
        channels
    }

    pub fn add(&mut self, channel: Arc<dyn Channel>) {
        self.by_id.insert(channel.id(), channel);
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Channel>> {
        self.by_id.get(id)
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_config(mut self, config: &Config) -> Self {
        self.config = Some(Arc::new(config.clone()));
        self
    }

    #[must_use]
    pub fn config(&self) -> Option<&Config> {
        self.config.as_deref()
    }
}
