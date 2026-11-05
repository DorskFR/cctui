use std::time::Duration;

use cctui_proto::dispatch_identity::{REGISTER_PATH, RegisterRequest, hold_path};

const REGISTER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct DispatchRegistry {
    http: reqwest::Client,
    server_url: String,
    dispatcher_key: String,
}

impl DispatchRegistry {
    #[must_use]
    pub fn new(server_url: String, dispatcher_key: String) -> Self {
        cctui_dispatcher_core::install_crypto_provider();
        Self { http: reqwest::Client::new(), server_url, dispatcher_key }
    }

    fn url(&self) -> String {
        format!("{}{REGISTER_PATH}", self.server_url.trim_end_matches('/'))
    }

    pub async fn register(&self, req: &RegisterRequest) -> anyhow::Result<()> {
        let resp = self
            .http
            .post(self.url())
            .bearer_auth(&self.dispatcher_key)
            .timeout(REGISTER_TIMEOUT)
            .json(req)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("registering dispatch {}: {e}", req.dispatch_id))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "the server refused to register dispatch {} ({status}): {body}",
            req.dispatch_id
        )
    }

    fn hold_url(&self, dispatch_id: uuid::Uuid) -> String {
        format!("{}{}", self.server_url.trim_end_matches('/'), hold_path(dispatch_id))
    }

    /// Keep a queued Job's registration from expiring while it waits.
    pub async fn hold(&self, dispatch_id: uuid::Uuid) -> anyhow::Result<()> {
        let resp = self
            .http
            .post(self.hold_url(dispatch_id))
            .bearer_auth(&self.dispatcher_key)
            .timeout(REGISTER_TIMEOUT)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("holding dispatch {dispatch_id}: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("the server refused to hold dispatch {dispatch_id} ({status}): {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_url_joins_the_server_and_path() {
        let r = DispatchRegistry::new("https://s.example.test/".to_owned(), "k".to_owned());
        assert_eq!(r.url(), "https://s.example.test/api/v1/dispatch/register");
    }

    #[test]
    fn hold_url_names_the_dispatch() {
        let r = DispatchRegistry::new("https://s.example.test/".to_owned(), "k".to_owned());
        assert_eq!(
            r.hold_url(uuid::Uuid::nil()),
            "https://s.example.test/api/v1/dispatch/00000000-0000-0000-0000-000000000000/hold"
        );
    }
}
