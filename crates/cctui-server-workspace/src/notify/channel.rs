use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::config::Config;

#[cfg_attr(not(test), allow(dead_code))]
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
    async fn send(&self, n: &Rendered, to: &Endpoint) -> Result<ProviderRef, SendError>;
    async fn update(&self, n: &Rendered, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError>;
    async fn retract(&self, to: &Endpoint, r: &ProviderRef) -> Result<(), SendError>;
}

#[derive(Clone, Default)]
pub struct Channels {
    by_id: HashMap<&'static str, Arc<dyn Channel>>,
}

impl Channels {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut channels = Self::default();
        if let Some(ntfy) = super::ntfy_channel::NtfyChannel::from_config(config) {
            channels.add(Arc::new(ntfy));
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
}
