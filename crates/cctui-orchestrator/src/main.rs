//! Admission webhook server: the mutating endpoint injects the secretless-worker
//! envelope into profiled pods (reading only the profile's [`ProfileFlags`] and
//! `spec.guard`), the validating endpoint checks them against their
//! `WorkerProfile`.

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use cctui_orchestrator::envelope::{
    DEFAULT_DNS_RESOLVER, MutationOutcome, ProfileFlags, default_sidecar_image, is_eligible,
    is_injected, mutate_pod, profile_name,
};
use cctui_orchestrator::validate::{Decision, GateAdmission, ProfileSource, Verdict};
use cctui_orchestrator::{GuardSpec, WorkerProfile, WorkerProfileSpec};
use k8s_openapi::api::core::v1::Pod;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::{Api, Client, Error as KubeError};
use std::net::SocketAddr;
use std::sync::Arc;

struct AppState {
    sidecar_image: String,
    dns_resolver: String,
    profile_source: Arc<dyn ProfileSource>,
    gates: GateAdmission,
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

/// The source profile's `spec.guard` and envelope flags for a pod about to be
/// injected. A missing profile renders neither (the validating
/// webhook denies the pod); a lookup failure is an `Err` so the pod is denied
/// fail-closed.
async fn profile_guard(
    state: &AppState,
    req: &AdmissionRequest<Pod>,
    pod: &Pod,
) -> anyhow::Result<(Option<GuardSpec>, ProfileFlags)> {
    if !is_eligible(pod) || is_injected(pod) {
        return Ok((None, ProfileFlags::default()));
    }
    let Some(name) = profile_name(pod) else { return Ok((None, ProfileFlags::default())) };
    let namespace = request_namespace(req, pod);
    let profile = state.profile_source.get(&namespace, name).await.map_err(|e| {
        anyhow::anyhow!("cannot resolve WorkerProfile `{name}` in `{namespace}`: {e}")
    })?;
    Ok(profile.map_or_else(
        || (None, ProfileFlags::default()),
        |p| {
            let flags = ProfileFlags {
                exec_interception: p.exec_interception(),
                human_gate: p.human_gate,
                dns_resolver: p.dns_resolver().map(ToOwned::to_owned),
                sidecar_resources: p.sidecar_resources,
            };
            (p.guard, flags)
        },
    ))
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
        let image = &state.sidecar_image;
        let outcome = profile_guard(&state, &req, pod).await.and_then(|(guard, mut flags)| {
            if flags.dns_resolver.is_none() {
                flags.dns_resolver = Some(state.dns_resolver.clone());
            }
            mutate_pod(pod, image, guard.as_ref(), flags)
        });
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
        let source = state.profile_source.as_ref();
        let verdict =
            cctui_orchestrator::validate::review_gated(pod, &namespace, source, &state.gates).await;
        respond(resp, verdict)
    } else {
        resp
    };
    Json(resp.into_review_for_pod())
}

/// Audit annotation key the validating webhook sets on an admitted pod whose
/// profile declares no exposure; the API server prefixes it with the webhook name.
const AUDIT_EXPOSURE: &str = "exposure";

fn respond(mut resp: AdmissionResponse, verdict: Verdict) -> AdmissionResponse {
    match verdict.decision {
        Decision::Deny(msg) => resp.deny(msg),
        Decision::Allow => {
            if verdict.exposure_undeclared {
                tracing::warn!(
                    warnings = ?verdict.warnings,
                    "admitting a worker pod whose profile declares no exposure"
                );
                resp.audit_annotations.insert(AUDIT_EXPOSURE.to_owned(), "undeclared".to_owned());
            }
            if !verdict.warnings.is_empty() {
                resp.warnings = Some(verdict.warnings);
            }
            resp
        }
    }
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

/// `require` is `CCTUI_ORCH_REQUIRE_HUMAN_GATE`, `rules` the path of the
/// guard-proxy gate rules file in `CCTUI_ORCH_GATE_RULES`.
fn gate_admission(require: Option<&str>, rules: Option<&str>) -> anyhow::Result<GateAdmission> {
    let require_gate = match require.map(str::trim) {
        None | Some("" | "false") => false,
        Some("true") => true,
        Some(other) => anyhow::bail!("CCTUI_ORCH_REQUIRE_HUMAN_GATE={other:?}: want true or false"),
    };
    let gates = GateAdmission { require_gate, ..GateAdmission::default() };
    let Some(path) = rules.map(str::trim).filter(|p| !p.is_empty()) else { return Ok(gates) };
    let json = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading gate rules {path}: {e}"))?;
    gates.with_rules(&json).map_err(|e| anyhow::anyhow!("{path}: {e}"))
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
    let dns_resolver = std::env::var("CCTUI_ORCH_DNS_RESOLVER")
        .ok()
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| DEFAULT_DNS_RESOLVER.to_owned());
    let gates = gate_admission(
        std::env::var("CCTUI_ORCH_REQUIRE_HUMAN_GATE").ok().as_deref(),
        std::env::var("CCTUI_ORCH_GATE_RULES").ok().as_deref(),
    )?;
    let client = Client::try_default().await?;
    let profile_source: Arc<dyn ProfileSource> = Arc::new(KubeProfileSource { client });
    let state = Arc::new(AppState { sidecar_image, dns_resolver, profile_source, gates });
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
        state_with(Arc::new(EmptyProfiles))
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
            dns_resolver: DEFAULT_DNS_RESOLVER.to_owned(),
            profile_source,
            gates: GateAdmission::default(),
        })
    }

    #[test]
    fn gate_admission_is_off_unless_required_and_reads_the_rules_file() {
        let off = gate_admission(None, None).unwrap();
        assert!(!off.require_gate && off.rule_sets.is_empty());
        assert!(!gate_admission(Some("false"), Some(" ")).unwrap().require_gate);
        assert!(gate_admission(Some(" true "), None).unwrap().require_gate);
        assert!(gate_admission(Some("yes"), None).is_err());
        let name = format!("cctui-orch-gates-{}.json", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, r#"{"gates": {"reviewer": {"extends": "draftPrOnly"}}}"#).unwrap();
        let gates = gate_admission(None, path.to_str());
        std::fs::write(&path, "{").unwrap();
        let broken = gate_admission(None, path.to_str());
        std::fs::remove_file(&path).unwrap();
        assert!(gates.unwrap().rule_sets.contains("reviewer"));
        assert!(broken.is_err());
        assert!(gate_admission(None, path.to_str()).is_err(), "a missing file is refused");
    }

    fn patch_value(response: &serde_json::Value) -> serde_json::Value {
        let raw: Vec<u8> = response["patch"]
            .as_array()
            .expect("patch present")
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect();
        serde_json::from_slice(&raw).unwrap()
    }

    fn decode_patch(response: &serde_json::Value) -> String {
        patch_value(response).to_string()
    }

    struct ResolverProfiles;

    #[async_trait::async_trait]
    impl ProfileSource for ResolverProfiles {
        async fn get(
            &self,
            _namespace: &str,
            _name: &str,
        ) -> anyhow::Result<Option<WorkerProfileSpec>> {
            Ok(Some(serde_json::from_value(serde_json::json!({
                "image": "registry.example.com/worker:latest",
                "dnsResolver": "10.43.0.53",
                "env": [{ "name": "WORKER_DNS_RESOLVER", "value": "9.9.9.9" }]
            }))?))
        }
    }

    fn net_init_env(review: &serde_json::Value, out: &serde_json::Value) -> serde_json::Value {
        let patch = patch_value(&out["response"]);
        let patch: json_patch::Patch = serde_json::from_value(patch).unwrap();
        let mut pod = review["request"]["object"].clone();
        json_patch::patch(&mut pod, &patch.0).unwrap();
        let inits = pod["spec"]["initContainers"].as_array().unwrap();
        inits.iter().find(|c| c["name"] == "net-init").unwrap()["env"].clone()
    }

    fn resolver_env(value: &str) -> serde_json::Value {
        serde_json::json!([{ "name": "WORKER_DNS_RESOLVER", "value": value }])
    }

    #[tokio::test]
    async fn net_init_resolver_comes_from_the_profile_else_the_orchestrator_config() {
        let mut review = review_json(serde_json::json!({ LABEL_WORKER_PROFILE: "lean" }));
        review["request"]["object"]["spec"]["containers"][0]["env"] = resolver_env("8.8.8.8");

        let configured = Arc::new(AppState {
            sidecar_image: "registry.example.com/w:test".to_owned(),
            dns_resolver: "resolver.config".to_owned(),
            profile_source: Arc::new(EmptyProfiles),
            gates: GateAdmission::default(),
        });
        let out = post_review_to(configured, review.clone()).await;
        assert_eq!(out["response"]["allowed"], true);
        assert_eq!(net_init_env(&review, &out), resolver_env("resolver.config"));

        let out = post_review_to(state_with(Arc::new(ResolverProfiles)), review.clone()).await;
        assert_eq!(out["response"]["allowed"], true);
        assert_eq!(net_init_env(&review, &out), resolver_env("10.43.0.53"));

        let out = post_review(review.clone()).await;
        assert_eq!(net_init_env(&review, &out), resolver_env(DEFAULT_DNS_RESOLVER));
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

    fn admission_request() -> AdmissionRequest<Pod> {
        let review: AdmissionReview<Pod> =
            serde_json::from_value(review_json(serde_json::json!({}))).unwrap();
        review.try_into().unwrap()
    }

    #[test]
    fn an_undeclared_exposure_admits_with_a_warning_and_an_audit_annotation() {
        let req = admission_request();
        let warning = "WorkerProfile `lean`: exposure undeclared, set spec.exposure".to_owned();
        let resp = respond(
            AdmissionResponse::from(&req),
            Verdict {
                decision: Decision::Allow,
                warnings: vec![warning.clone()],
                exposure_undeclared: true,
            },
        );
        assert!(resp.allowed);
        assert_eq!(resp.warnings, Some(vec![warning]));
        assert_eq!(
            resp.audit_annotations.get(AUDIT_EXPOSURE).map(String::as_str),
            Some("undeclared")
        );

        let resp = respond(
            AdmissionResponse::from(&req),
            Verdict { decision: Decision::Allow, warnings: Vec::new(), exposure_undeclared: false },
        );
        assert!(resp.allowed && resp.warnings.is_none() && resp.audit_annotations.is_empty());

        let resp = respond(
            AdmissionResponse::from(&req),
            Verdict {
                decision: Decision::Deny("spec.exposure: no gate".into()),
                warnings: Vec::new(),
                exposure_undeclared: false,
            },
        );
        assert!(!resp.allowed);
        assert_eq!(resp.result.message, "spec.exposure: no gate");
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
