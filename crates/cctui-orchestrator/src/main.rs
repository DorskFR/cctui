//! Mutating admission webhook server: injects the secretless-worker envelope
//! into profiled pods. No kube client, no state — it operates purely on the
//! `AdmissionReview` payload.

use axum::{Json, Router, http::StatusCode, routing::get, routing::post};
use cctui_orchestrator::envelope::{default_sidecar_image, mutate_pod};
use k8s_openapi::api::core::v1::Pod;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use std::net::SocketAddr;
use std::sync::Arc;

struct AppState {
    sidecar_image: String,
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn mutate(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(review): Json<AdmissionReview<Pod>>,
) -> Json<AdmissionReview<Pod>> {
    let req: AdmissionRequest<Pod> = match review.try_into() {
        Ok(req) => req,
        Err(err) => {
            return Json(AdmissionResponse::invalid(err.to_string()).into_review_for_pod());
        }
    };

    let mut resp = AdmissionResponse::from(&req);
    if let Some(pod) = &req.object
        && let Some(patch) = mutate_pod(pod, &state.sidecar_image)
    {
        resp = match resp.with_patch(patch) {
            Ok(patched) => patched,
            Err(err) => AdmissionResponse::from(&req).deny(err.to_string()),
        };
    }
    Json(resp.into_review_for_pod())
}

/// `AdmissionResponse::into_review` yields `AdmissionReview<DynamicObject>`; the
/// response carries no request object, so it is sound to re-type the review to
/// `AdmissionReview<Pod>` to match axum's typed handler signature.
trait IntoPodReview {
    fn into_review_for_pod(self) -> AdmissionReview<Pod>;
}

impl IntoPodReview for AdmissionResponse {
    fn into_review_for_pod(self) -> AdmissionReview<Pod> {
        let review = self.into_review();
        AdmissionReview { types: review.types, request: None, response: review.response }
    }
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/mutate", post(mutate))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let sidecar_image =
        std::env::var("CCTUI_ORCH_SIDECAR_IMAGE").unwrap_or_else(|_| default_sidecar_image());
    let state = Arc::new(AppState { sidecar_image });
    let router = app(state);

    let addr: SocketAddr =
        std::env::var("CCTUI_ORCH_LISTEN").unwrap_or_else(|_| "0.0.0.0:8443".to_owned()).parse()?;

    if let (Ok(cert), Ok(key)) =
        (std::env::var("CCTUI_ORCH_TLS_CERT"), std::env::var("CCTUI_ORCH_TLS_KEY"))
    {
        tracing::info!(%addr, "serving mutating webhook over TLS");
        let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
        axum_server::bind_rustls(addr, config).serve(router.into_make_service()).await?;
    } else {
        tracing::warn!(%addr, "TLS cert/key unset; serving plain HTTP (local/testing only)");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use cctui_orchestrator::LABEL_WORKER_PROFILE;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn review_json(labels: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "admission.k8s.io/v1",
            "kind": "AdmissionReview",
            "request": {
                "uid": "test-uid",
                "kind": { "group": "", "version": "v1", "kind": "Pod" },
                "resource": { "group": "", "version": "v1", "resource": "pods" },
                "operation": "CREATE",
                "userInfo": {},
                "object": {
                    "apiVersion": "v1",
                    "kind": "Pod",
                    "metadata": { "name": "p", "labels": labels, "annotations": {} },
                    "spec": {
                        "containers": [
                            { "name": "worker", "image": "registry.example.com/worker:latest" }
                        ]
                    }
                }
            }
        })
    }

    async fn post_review(body: serde_json::Value) -> serde_json::Value {
        let state = Arc::new(AppState { sidecar_image: "registry.example.com/w:test".to_owned() });
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mutate")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn profiled_pod_round_trips_with_patch() {
        let out =
            post_review(review_json(serde_json::json!({ LABEL_WORKER_PROFILE: "lean" }))).await;
        let response = &out["response"];
        assert_eq!(response["uid"], "test-uid");
        assert_eq!(response["allowed"], true);
        assert_eq!(response["patchType"], "JSONPatch");

        let raw: Vec<u8> = response["patch"]
            .as_array()
            .expect("patch present")
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect();
        let patch: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let text = patch.to_string();
        assert!(text.contains("guard-proxy"), "patch injects guard-proxy sidecar");
        assert!(text.contains("envelope-injected"), "patch stamps the marker annotation");
    }

    #[tokio::test]
    async fn unlabeled_pod_allowed_without_patch() {
        let out = post_review(review_json(serde_json::json!({}))).await;
        let response = &out["response"];
        assert_eq!(response["allowed"], true);
        assert!(response.get("patch").is_none() || response["patch"].is_null());
    }

    #[tokio::test]
    async fn healthz_ok() {
        let state = Arc::new(AppState { sidecar_image: "x".to_owned() });
        let resp = app(state)
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
