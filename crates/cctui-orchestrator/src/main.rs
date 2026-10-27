//! Admission webhook server: the mutating endpoint injects the secretless-worker
//! envelope into profiled pods (reading only the profile's `spec.guard`), the
//! validating endpoint checks them against their `WorkerProfile`.

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use cctui_orchestrator::envelope::{
    MutationOutcome, default_sidecar_image, is_eligible, is_injected, mutate_pod, profile_name,
};
use cctui_orchestrator::validate::{Decision, ProfileSource, validate};
use cctui_orchestrator::{GuardSpec, WorkerProfile, WorkerProfileSpec};
use k8s_openapi::api::core::v1::Pod;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::{Api, Client, Error as KubeError};
use std::net::SocketAddr;
use std::sync::Arc;

struct AppState {
    sidecar_image: String,
    profile_source: Arc<dyn ProfileSource>,
}

/// Resolves `WorkerProfile`s from the cluster for the validating webhook.
struct KubeProfileSource {
    client: Client,
}

#[async_trait::async_trait]
impl ProfileSource for KubeProfileSource {
    async fn get(&self, namespace: &str, name: &str) -> anyhow::Result<Option<WorkerProfileSpec>> {
        let api: Api<WorkerProfile> = Api::namespaced(self.client.clone(), namespace);
        match api.get(name).await {
            Ok(profile) => Ok(Some(profile.spec)),
            Err(KubeError::Api(e)) if e.code == 404 => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

fn request_namespace(req: &AdmissionRequest<Pod>, pod: &Pod) -> String {
    req.namespace
        .clone()
        .or_else(|| pod.metadata.namespace.clone())
        .unwrap_or_else(|| "default".to_owned())
}

/// The source profile's `spec.guard` for a pod about to be injected. A missing
/// profile renders no guard overrides (the validating webhook denies the pod);
/// a lookup failure is an `Err` so the pod is denied fail-closed.
async fn profile_guard(
    state: &AppState,
    req: &AdmissionRequest<Pod>,
    pod: &Pod,
) -> anyhow::Result<Option<GuardSpec>> {
    if !is_eligible(pod) || is_injected(pod) {
        return Ok(None);
    }
    let Some(name) = profile_name(pod) else { return Ok(None) };
    let namespace = request_namespace(req, pod);
    let profile = state.profile_source.get(&namespace, name).await.map_err(|e| {
        anyhow::anyhow!("cannot resolve WorkerProfile `{name}` in `{namespace}`: {e}")
    })?;
    Ok(profile.and_then(|p| p.guard))
}

#[allow(clippy::cognitive_complexity)]
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
    if let Some(pod) = &req.object {
        let outcome = profile_guard(&state, &req, pod)
            .await
            .and_then(|guard| mutate_pod(pod, &state.sidecar_image, guard.as_ref()));
        match outcome {
            Ok(MutationOutcome::Patch(patch)) => {
                tracing::info!(
                    pod = pod.metadata.name.as_deref().unwrap_or("<unnamed>"),
                    "injecting worker sandbox envelope"
                );
                resp = match resp.with_patch(patch) {
                    Ok(patched) => patched,
                    Err(err) => {
                        tracing::error!(%err, "encoding JSONPatch failed; denying pod (fail closed)");
                        AdmissionResponse::from(&req).deny(err.to_string())
                    }
                };
            }
            Ok(MutationOutcome::NotEligible | MutationOutcome::AlreadyInjected) => {}
            Err(err) => {
                tracing::error!(%err, "envelope construction failed; denying pod (fail closed)");
                resp = AdmissionResponse::from(&req).deny(err.to_string());
            }
        }
    }
    Json(resp.into_review_for_pod())
}

async fn validate_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(review): Json<AdmissionReview<Pod>>,
) -> Json<AdmissionReview<Pod>> {
    let req: AdmissionRequest<Pod> = match review.try_into() {
        Ok(req) => req,
        Err(err) => {
            return Json(AdmissionResponse::invalid(err.to_string()).into_review_for_pod());
        }
    };

    let resp = AdmissionResponse::from(&req);
    let resp = if let Some(pod) = &req.object {
        let namespace = request_namespace(&req, pod);
        match validate(pod, &namespace, state.profile_source.as_ref()).await {
            Decision::Allow => resp,
            Decision::Deny(msg) => resp.deny(msg),
        }
    } else {
        resp
    };
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
        .route("/validate", post(validate_handler))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("rustls crypto provider already installed"))?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let sidecar_image =
        std::env::var("CCTUI_ORCH_SIDECAR_IMAGE").unwrap_or_else(|_| default_sidecar_image());
    let client = Client::try_default().await?;
    let profile_source: Arc<dyn ProfileSource> = Arc::new(KubeProfileSource { client });
    let state = Arc::new(AppState { sidecar_image, profile_source });
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
    use cctui_orchestrator::{ANNOTATION_POSTURE, LABEL_WORKER_PROFILE};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct EmptyProfiles;

    #[async_trait::async_trait]
    impl ProfileSource for EmptyProfiles {
        async fn get(
            &self,
            _namespace: &str,
            _name: &str,
        ) -> anyhow::Result<Option<WorkerProfileSpec>> {
            Ok(None)
        }
    }

    fn test_state() -> Arc<AppState> {
        Arc::new(AppState {
            sidecar_image: "registry.example.com/w:test".to_owned(),
            profile_source: Arc::new(EmptyProfiles),
        })
    }
    #[allow(clippy::needless_pass_by_value)]
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

    struct GuardedProfiles;

    #[async_trait::async_trait]
    impl ProfileSource for GuardedProfiles {
        async fn get(
            &self,
            _namespace: &str,
            _name: &str,
        ) -> anyhow::Result<Option<WorkerProfileSpec>> {
            Ok(Some(serde_json::from_value(serde_json::json!({
                "image": "registry.example.com/worker:latest",
                "guard": { "envFrom": [{ "configMapRef": { "name": "guard-proxy-env-canary" } }] }
            }))?))
        }
    }

    struct FailingProfiles;

    #[async_trait::async_trait]
    impl ProfileSource for FailingProfiles {
        async fn get(
            &self,
            _namespace: &str,
            _name: &str,
        ) -> anyhow::Result<Option<WorkerProfileSpec>> {
            anyhow::bail!("apiserver unreachable")
        }
    }

    fn state_with(profile_source: Arc<dyn ProfileSource>) -> Arc<AppState> {
        Arc::new(AppState {
            sidecar_image: "registry.example.com/w:test".to_owned(),
            profile_source,
        })
    }

    fn decode_patch(response: &serde_json::Value) -> String {
        let raw: Vec<u8> = response["patch"]
            .as_array()
            .expect("patch present")
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect();
        serde_json::from_slice::<serde_json::Value>(&raw).unwrap().to_string()
    }

    async fn post_review(body: serde_json::Value) -> serde_json::Value {
        post_review_to(test_state(), body).await
    }

    async fn post_review_to(state: Arc<AppState>, body: serde_json::Value) -> serde_json::Value {
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
    async fn profile_guard_env_from_reaches_the_patch() {
        let review = review_json(serde_json::json!({ LABEL_WORKER_PROFILE: "lean" }));
        let out = post_review_to(state_with(Arc::new(GuardedProfiles)), review).await;
        assert_eq!(out["response"]["allowed"], true);
        let text = decode_patch(&out["response"]);
        assert!(text.contains("guard-proxy-env-canary"), "{text}");
        assert!(text.contains("\"guard-proxy-env\""), "global ConfigMap still rendered: {text}");
    }

    #[tokio::test]
    async fn profile_lookup_failure_denies_the_pod() {
        let review = review_json(serde_json::json!({ LABEL_WORKER_PROFILE: "lean" }));
        let out = post_review_to(state_with(Arc::new(FailingProfiles)), review).await;
        let response = &out["response"];
        assert_eq!(response["allowed"], false);
        let message = response["status"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("apiserver unreachable"), "{message}");
    }

    fn review_with_posture(posture: &str) -> serde_json::Value {
        let mut review = review_json(serde_json::json!({ LABEL_WORKER_PROFILE: "lean" }));
        review["request"]["object"]["metadata"]["annotations"] =
            serde_json::json!({ ANNOTATION_POSTURE: posture });
        review
    }

    #[tokio::test]
    async fn nonroot_posture_pod_gets_the_restricted_envelope() {
        let out = post_review(review_with_posture("nonroot")).await;
        assert_eq!(out["response"]["allowed"], true);
        let text = decode_patch(&out["response"]);
        for want in ["runAsNonRoot\":true", "RuntimeDefault", "runAsGroup\":1000", "NET_ADMIN"] {
            assert!(text.contains(want), "missing {want}: {text}");
        }
        for absent in ["SYS_ADMIN", "Unconfined", "\"overlay\"", "/overlay"] {
            assert!(!text.contains(absent), "unexpected {absent}: {text}");
        }
    }

    #[tokio::test]
    async fn root_posture_pod_keeps_the_root_envelope() {
        let text = decode_patch(&post_review(review_with_posture("root")).await["response"]);
        for want in ["SYS_ADMIN", "Unconfined", "\"overlay\""] {
            assert!(text.contains(want), "missing {want}: {text}");
        }
        assert!(!text.contains("runAsNonRoot"), "{text}");
    }

    #[tokio::test]
    async fn unknown_posture_annotation_denies_the_pod() {
        let out = post_review(review_with_posture("restricted")).await;
        let response = &out["response"];
        assert_eq!(response["allowed"], false);
        let message = response["status"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("names no posture"), "{message}");
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
        let resp = app(test_state())
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    async fn post_validate(body: serde_json::Value) -> serde_json::Value {
        let resp = app(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/validate")
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
    async fn validate_allows_unlabeled_pod() {
        let out = post_validate(review_json(serde_json::json!({}))).await;
        assert_eq!(out["response"]["uid"], "test-uid");
        assert_eq!(out["response"]["allowed"], true);
    }

    #[tokio::test]
    async fn validate_denies_profiled_pod_with_readable_message() {
        let out =
            post_validate(review_json(serde_json::json!({ LABEL_WORKER_PROFILE: "lean" }))).await;
        let response = &out["response"];
        assert_eq!(response["uid"], "test-uid");
        assert_eq!(response["allowed"], false);
        let message = response["status"]["message"].as_str().unwrap_or_default();
        assert!(!message.is_empty(), "denial carries a human-readable message");
    }
}
