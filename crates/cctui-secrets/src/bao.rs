use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::{Credential, SecretError, SecretRef};

pub const DEFAULT_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/// `HashiCorp` Vault / `OpenBao` KV v2 (Kubernetes auth via the pod SA token).
pub struct VaultClient {
    http: reqwest::Client,
    addr: String,
    role: String,
    token_path: PathBuf,
}

impl VaultClient {
    pub fn new(addr: String, role: String, token_path: PathBuf) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        Ok(Self { http, addr, role, token_path })
    }

    fn backend_err(context: &str, e: impl fmt::Display) -> SecretError {
        SecretError::Backend(anyhow::anyhow!("{context}: {e}"))
    }

    async fn login(&self) -> Result<String, SecretError> {
        let jwt = tokio::fs::read_to_string(&self.token_path)
            .await
            .map_err(|e| Self::backend_err("reading service account token", e))?;
        let url = format!("{}/v1/auth/kubernetes/login", self.addr.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "jwt": jwt.trim(), "role": self.role }))
            .send()
            .await
            .map_err(|e| Self::backend_err("vault login request", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!("vault login failed: HTTP {status}")));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| Self::backend_err("vault login response", e))?;
        body.pointer("/auth/client_token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                SecretError::Backend(anyhow::anyhow!(
                    "vault login response missing auth.client_token"
                ))
            })
    }

    pub async fn read_kv2(
        &self,
        r: &SecretRef,
        mount: &str,
        path: &str,
        field: &str,
    ) -> Result<Credential, SecretError> {
        let token = self.login().await?;
        let url = format!("{}/v1/{mount}/data/{path}", self.addr.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .header("X-Vault-Token", &token)
            .send()
            .await
            .map_err(|e| Self::backend_err("vault read request", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::not_found(r));
        }
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!("vault read failed: HTTP {status}")));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| Self::backend_err("vault read response", e))?;
        match body.pointer(&format!("/data/data/{field}")).and_then(serde_json::Value::as_str) {
            Some(value) if !value.is_empty() => Ok(Credential::new(value.to_owned())),
            _ => Err(SecretError::not_found(r)),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::*;
    use crate::{Engines, RefResolver as _, resolve_value};

    async fn spawn_vault_mock() -> String {
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::{get, post};
        use axum::{Json, Router};

        let app = Router::new()
            .route(
                "/v1/auth/kubernetes/login",
                post(|Json(body): Json<serde_json::Value>| async move {
                    if body["jwt"] == "test-jwt" && body["role"] == "cctui-worker" {
                        Json(serde_json::json!({"auth": {"client_token": "tok-123"}}))
                            .into_response()
                    } else {
                        (StatusCode::FORBIDDEN, "bad login").into_response()
                    }
                }),
            )
            .route(
                "/v1/kvmount/data/dev/cctui-worker",
                get(|headers: HeaderMap| async move {
                    if headers.get("X-Vault-Token").and_then(|v| v.to_str().ok()) != Some("tok-123")
                    {
                        return (StatusCode::FORBIDDEN, "bad token").into_response();
                    }
                    Json(serde_json::json!({"data": {"data": {
                        "GITHUB_TOKEN_ACME": "hunter2",
                        "EMPTY": "",
                    }}}))
                    .into_response()
                }),
            )
            .route(
                "/v1/kvmount/data/boom",
                get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "vault sad").into_response() }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn write_token(dir: &tempfile::TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("token");
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn vault_engines(addr: String, token_path: PathBuf) -> Engines {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = VaultClient::new(addr, "cctui-worker".to_owned(), token_path).unwrap();
        Engines::new().with_vault(Some(client))
    }

    #[tokio::test]
    async fn vault_engine_logs_in_and_reads_kv2_field() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr, write_token(&dir, "test-jwt\n"));
        let r = SecretRef::parse("vault:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert_eq!(engines.resolve(&r).await.unwrap().expose(), "hunter2");
        let r = SecretRef::parse("bao:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert_eq!(engines.resolve(&r).await.unwrap().expose(), "hunter2");
    }

    #[tokio::test]
    async fn vault_engine_missing_path_field_or_empty_is_not_found() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr, write_token(&dir, "test-jwt"));
        for r in [
            "vault:kvmount/data/nope#GITHUB_TOKEN_ACME",
            "vault:kvmount/data/dev/cctui-worker#MISSING_FIELD",
            "vault:kvmount/data/dev/cctui-worker#EMPTY",
        ] {
            let r = SecretRef::parse(r).unwrap();
            assert!(
                matches!(engines.resolve(&r).await, Err(SecretError::NotFound { .. })),
                "{r} must be NotFound"
            );
        }
    }

    #[tokio::test]
    async fn vault_engine_server_error_and_bad_login_fail_closed() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr.clone(), write_token(&dir, "test-jwt"));
        let r = SecretRef::parse("vault:kvmount/data/boom#f").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));

        let engines = vault_engines(addr, write_token(&dir, "wrong-jwt"));
        let r = SecretRef::parse("vault:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn vault_engine_unreadable_token_fails_closed() {
        let addr = spawn_vault_mock().await;
        let engines = vault_engines(addr, PathBuf::from("/nonexistent/token"));
        let r = SecretRef::parse("vault:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn bao_value_error_names_the_ref_and_hides_sibling_secrets() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr, write_token(&dir, "test-jwt"));
        let raw = "bao:kvmount/data/dev/cctui-worker#MISSING_FIELD";
        let msg = resolve_value(&engines, raw).await.unwrap_err().to_string();
        assert!(msg.contains("vault:kvmount/data/dev/cctui-worker#MISSING_FIELD"), "{msg}");
        assert!(!msg.contains("hunter2"), "{msg}");
        assert!(!msg.contains("tok-123"), "{msg}");
    }
}
