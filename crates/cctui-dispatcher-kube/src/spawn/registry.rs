use std::time::Duration;

use cctui_proto::dispatch_identity::{REGISTER_PATH, RegisterRequest};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_url_joins_the_server_and_path() {
        let r = DispatchRegistry::new("https://s.example.test/".to_owned(), "k".to_owned());
        assert_eq!(r.url(), "https://s.example.test/api/v1/dispatch/register");
    }
}
