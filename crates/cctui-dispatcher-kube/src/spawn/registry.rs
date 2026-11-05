use std::time::Duration;

use cctui_dispatcher_core::secrets::Secret;
use cctui_proto::dispatch_identity::{REGISTER_PATH, RegisterRequest, hold_path};

const REGISTER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct DispatchRegistry {
    http: reqwest::Client,
    server_url: String,
    dispatcher_key: Secret,
}

impl DispatchRegistry {
    #[must_use]
    pub fn new(server_url: String, dispatcher_key: Secret) -> Self {
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
            .bearer_auth(self.dispatcher_key.get())
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
            .bearer_auth(self.dispatcher_key.get())
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

    fn key(value: &str) -> Secret {
        Secret::fixed("dispatcher_key", value)
    }

    async fn authorization_of_one_request(listener: &tokio::net::TcpListener) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 4096];
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "connection closed before the headers ended");
            seen.extend_from_slice(&buf[..n]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8_lossy(&seen)
            .lines()
            .find_map(|l| l.strip_prefix("authorization: ").map(str::to_owned))
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_rotated_dispatcher_key_is_sent_from_the_next_registration_on() {
        use cctui_dispatcher_core::secrets::{Engines, RefResolver, SecretRegistry};
        let path = std::env::temp_dir().join(format!("cctui-dk-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "dk-v1\n").unwrap();
        let resolver: std::sync::Arc<dyn RefResolver> = std::sync::Arc::new(Engines::new());
        let secrets =
            SecretRegistry::new(resolver).with_ttl_lookup(std::sync::Arc::new(|_: &str| None));
        let key = secrets
            .register("dispatcher_key", &format!("file:{}", path.display()))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let registry =
            DispatchRegistry::new(format!("http://{}", listener.local_addr().unwrap()), key);
        let req = RegisterRequest {
            dispatch_id: uuid::Uuid::nil(),
            session_id: "s".to_owned(),
            namespace: "ns".to_owned(),
            job_name: "job".to_owned(),
            machine_key: "mk".to_owned(),
            env: std::collections::BTreeMap::new(),
        };

        let (sent, ()) = tokio::join!(authorization_of_one_request(&listener), async {
            registry.register(&req).await.unwrap();
        });
        assert_eq!(sent, "Bearer dk-v1");

        std::fs::write(&path, "dk-v2\n").unwrap();
        assert_eq!(secrets.reload().await.changed, 1);
        let (sent, ()) = tokio::join!(authorization_of_one_request(&listener), async {
            registry.register(&req).await.unwrap();
        });
        assert_eq!(sent, "Bearer dk-v2");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn register_url_joins_the_server_and_path() {
        let r = DispatchRegistry::new("https://s.example.test/".to_owned(), key("k"));
        assert_eq!(r.url(), "https://s.example.test/api/v1/dispatch/register");
    }

    #[test]
    fn hold_url_names_the_dispatch() {
        let r = DispatchRegistry::new("https://s.example.test/".to_owned(), key("k"));
        assert_eq!(
            r.hold_url(uuid::Uuid::nil()),
            "https://s.example.test/api/v1/dispatch/00000000-0000-0000-0000-000000000000/hold"
        );
    }
}
