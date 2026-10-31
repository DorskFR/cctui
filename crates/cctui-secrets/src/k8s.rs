use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;

use crate::{Credential, SecretError, SecretRef};

/// Kubernetes Secrets via the pod's own `ServiceAccount` (RBAC decides what it
/// may read).
pub struct K8sClient {
    http: reqwest::Client,
    api_base: String,
    token_path: PathBuf,
    default_namespace: String,
}

const K8S_SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

impl K8sClient {
    /// Builds from the in-cluster environment (`KUBERNETES_SERVICE_HOST` + the
    /// projected SA dir). Errors off-cluster.
    pub fn in_cluster() -> anyhow::Result<Self> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .map_err(|_| anyhow::anyhow!("KUBERNETES_SERVICE_HOST unset (not in-cluster)"))?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_owned());
        let ca = std::fs::read(format!("{K8S_SA_DIR}/ca.crt"))?;
        let namespace = std::fs::read_to_string(format!("{K8S_SA_DIR}/namespace"))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .add_root_certificate(reqwest::Certificate::from_pem(&ca)?)
            .build()?;
        Ok(Self {
            http,
            api_base: format!("https://{host}:{port}"),
            token_path: PathBuf::from(format!("{K8S_SA_DIR}/token")),
            default_namespace: namespace.trim().to_owned(),
        })
    }

    /// Off-cluster constructor with an explicit API base + token path.
    pub fn with_base(
        api_base: String,
        token_path: PathBuf,
        default_namespace: String,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        Ok(Self { http, api_base, token_path, default_namespace })
    }

    pub async fn read(
        &self,
        r: &SecretRef,
        namespace: Option<&str>,
        name: &str,
        key: &str,
    ) -> Result<Credential, SecretError> {
        let err = |context: &str, e: &dyn fmt::Display| {
            SecretError::Backend(anyhow::anyhow!("k8s secret read: {context}: {e}"))
        };
        let token = tokio::fs::read_to_string(&self.token_path)
            .await
            .map_err(|e| err("reading service account token", &e))?;
        let ns = namespace.unwrap_or(&self.default_namespace);
        let url = format!(
            "{}/api/v1/namespaces/{ns}/secrets/{name}",
            self.api_base.trim_end_matches('/')
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(token.trim())
            .send()
            .await
            .map_err(|e| err("request", &e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::not_found(r));
        }
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!(
                "k8s secret read failed: HTTP {status}"
            )));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| err("response", &e))?;
        let Some(b64) = body.pointer(&format!("/data/{key}")).and_then(serde_json::Value::as_str)
        else {
            return Err(SecretError::not_found(r));
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| err("base64 decode", &e))?;
        match String::from_utf8(bytes) {
            Ok(value) if !value.is_empty() => Ok(Credential::new(value)),
            Ok(_) => Err(SecretError::not_found(r)),
            Err(e) => Err(err("utf-8 decode", &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::*;
    use crate::{Engines, RefResolver as _, resolve_value};

    async fn spawn_k8s_mock() -> String {
        use axum::extract::Path;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::get;
        use axum::{Json, Router};

        let app = Router::new().route(
            "/api/v1/namespaces/{ns}/secrets/{name}",
            get(|Path((ns, name)): Path<(String, String)>, headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok())
                    != Some("Bearer sa-token")
                {
                    return (StatusCode::UNAUTHORIZED, "bad token").into_response();
                }
                match (ns.as_str(), name.as_str()) {
                    ("dev", "worker-creds") => Json(serde_json::json!({"data": {
                        "token": base64::engine::general_purpose::STANDARD.encode("k8s-tok"),
                    }}))
                    .into_response(),
                    _ => (StatusCode::NOT_FOUND, "no secret").into_response(),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn k8s_engines(base: String, dir: &tempfile::TempDir, token: &str) -> Engines {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let path = dir.path().join("token");
        std::fs::write(&path, token).unwrap();
        Engines::new().with_k8s(Some(K8sClient::with_base(base, path, "dev".to_owned()).unwrap()))
    }

    #[tokio::test]
    async fn k8s_engine_reads_secret_key_with_ns_default_and_override() {
        let base = spawn_k8s_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = k8s_engines(base, &dir, "sa-token\n");

        let by_default_ns = SecretRef::parse("k8s:worker-creds#token").unwrap();
        assert_eq!(engines.resolve(&by_default_ns).await.unwrap().expose(), "k8s-tok");
        let by_explicit_ns = SecretRef::parse("k8s:dev/worker-creds#token").unwrap();
        assert_eq!(engines.resolve(&by_explicit_ns).await.unwrap().expose(), "k8s-tok");

        let missing_secret = SecretRef::parse("k8s:nope#token").unwrap();
        assert!(matches!(
            engines.resolve(&missing_secret).await,
            Err(SecretError::NotFound { .. })
        ));
        let missing_key = SecretRef::parse("k8s:worker-creds#missing").unwrap();
        assert!(matches!(engines.resolve(&missing_key).await, Err(SecretError::NotFound { .. })));
        let wrong_ns = SecretRef::parse("k8s:prod/worker-creds#token").unwrap();
        assert!(matches!(engines.resolve(&wrong_ns).await, Err(SecretError::NotFound { .. })));
    }

    #[tokio::test]
    async fn k8s_engine_bad_sa_token_fails_closed() {
        let base = spawn_k8s_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = k8s_engines(base, &dir, "wrong");
        let r = SecretRef::parse("k8s:worker-creds#token").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn k8s_value_resolves_and_missing_key_error_names_the_ref() {
        let base = spawn_k8s_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = k8s_engines(base, &dir, "sa-token");
        assert_eq!(resolve_value(&engines, "k8s:dev/worker-creds#token").await.unwrap(), "k8s-tok");
        let msg =
            resolve_value(&engines, "k8s:dev/worker-creds#other").await.unwrap_err().to_string();
        assert!(msg.contains("k8s:dev/worker-creds#other"), "{msg}");
        assert!(!msg.contains("k8s-tok"), "{msg}");
    }
}
