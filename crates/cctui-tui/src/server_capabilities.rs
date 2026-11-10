//! Refuse a headless server before opening the interactive terminal.
use anyhow::Result;
use cctui_client::Client;
use cctui_proto::api::capabilities::CapabilitiesResponse;

pub async fn require_workspace(client: &Client) -> Result<()> {
    match client.get_as::<CapabilitiesResponse>("get_capabilities", &[]).await {
        Ok(caps) => anyhow::ensure!(
            caps.workspace,
            "this headless server has no interactive sessions; connect CCTUI_URL to a cctui server"
        ),
        // Offline and authentication failures surface later, with their own handling.
        Err(error) => tracing::debug!(%error, "server capability probe unavailable"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn probe(status: &str, body: &str) -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let count = socket.read(&mut request).await.unwrap();
            assert!(
                std::str::from_utf8(&request[..count])
                    .unwrap()
                    .starts_with("GET /api/v1/capabilities ")
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let client = Client::new(format!("http://{address}"), "fixture-key");
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(2), require_workspace(&client))
                .await
                .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), server).await.unwrap().unwrap();
        result
    }

    #[tokio::test]
    async fn a_headless_server_is_refused_with_one_message() {
        let body = r#"{"langfuse":{"available":false,"host":null,"public_host":null,"project_id":null},"distribution":"core","workspace":false,"features":["dispatch"],"experimental":true}"#;
        let error = probe("200 OK", body).await.unwrap_err().to_string();
        assert!(error.contains("headless server"), "{error}");
        assert!(error.contains("CCTUI_URL"), "{error}");
    }

    #[tokio::test]
    async fn servers_without_a_workspace_flag_remain_supported() {
        probe(
            "200 OK",
            r#"{"langfuse":{"available":false,"host":null,"public_host":null,"project_id":null}}"#,
        )
        .await
        .unwrap();
        probe("404 Not Found", "{}").await.unwrap();
    }

    #[tokio::test]
    async fn a_server_with_every_feature_off_still_opens() {
        let body = serde_json::to_string(&CapabilitiesResponse::full(
            cctui_proto::api::capabilities::LangfuseCapability::default(),
            cctui_proto::features::FeatureSet::none(),
        ))
        .unwrap();
        probe("200 OK", &body).await.unwrap();
    }
}
