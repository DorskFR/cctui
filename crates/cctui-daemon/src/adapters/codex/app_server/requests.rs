use serde_json::{Value, json};

use super::rpc::{ID_INITIALIZE, ID_THREAD_START};
use crate::adapters::codex::contract;

/// Thread identity extracted from a `thread/start` response.
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub cwd: Option<String>,
    pub rollout_path: Option<String>,
}

/// Pull thread identity out of a `thread/start` response `result` object.
#[must_use]
pub fn thread_info(result: &Value) -> Option<ThreadInfo> {
    let t = result.get("thread")?;
    let thread_id = t
        .get("sessionId")
        .or_else(|| t.get("id"))
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)?;
    Some(ThreadInfo {
        thread_id,
        cwd: t.get("cwd").and_then(Value::as_str).map(std::string::ToString::to_string),
        rollout_path: t.get("path").and_then(Value::as_str).map(std::string::ToString::to_string),
    })
}

/// Build the documented `initialize` request. Capabilities are
/// declared explicitly rather than left to defaults so a protocol change that
/// flips a default is visible here: cctui speaks the stable (non-experimental)
/// API and does not participate in upstream attestation.
pub(super) fn initialize_req() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_INITIALIZE,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "cctui", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {
                "experimentalApi": false,
                "requestAttestation": false,
            },
        },
    })
}

/// The `initialized` notification that completes the handshake. Codex expects
/// it after the client has processed the `initialize` response; only then is
/// the server fully ready for `thread/*` requests.
pub(in crate::adapters::codex) fn initialized_notification() -> Value {
    json!({"jsonrpc": "2.0", "method": "initialized"})
}

/// Pull the Codex version out of an `initialize` response and log a diagnostic:
/// info when supported, a loud warning when the server is below
/// [`contract::CODEX_MIN_VERSION`] (the protocol shapes cctui relies on are not
/// guaranteed there). The version is returned so it can ride on the
/// [`AdapterEvent::SessionStarted`] meta for downstream diagnose reports.
pub(in crate::adapters::codex) fn record_codex_version(response: &Value) -> Option<String> {
    let user_agent = response.pointer("/result/userAgent").and_then(Value::as_str);
    let version = user_agent.and_then(contract::version_from_user_agent);
    match &version {
        Some(v) if contract::version_supported(v) => {
            tracing::info!(
                codex_version = %v,
                min = contract::CODEX_MIN_VERSION,
                "codex app-server handshake: supported version",
            );
        }
        Some(v) => {
            tracing::warn!(
                codex_version = %v,
                min = contract::CODEX_MIN_VERSION,
                "codex app-server is below the minimum supported version; protocol may drift",
            );
        }
        None => {
            tracing::warn!(
                user_agent = user_agent.unwrap_or("<missing>"),
                "codex app-server initialize response had no parseable version",
            );
        }
    }
    version
}

/// The gateway routing block as a per-thread `ThreadStart`/`ThreadResumeParams`
/// input rather than a process-level `-c` flag, so one app-server can host
/// threads for several accounts at once.
///
/// The bearer is a literal `authorization` header instead of the `env_key`
/// indirection, which resolves against the *process* env and so cannot differ
/// per thread. Codex persists only `model_provider = "cctui"` in the rollout —
/// never the definition or the secret — so this must be re-supplied on every
/// resume or the thread fails config load.
#[must_use]
pub fn gateway_thread_config(
    env: &std::collections::BTreeMap<String, String>,
) -> Option<(String, Value)> {
    let base_url = env.get("OPENAI_BASE_URL")?;
    let mut headers = json!({"x-openai-actor-authorization": "cctui-gateway"});
    let mut provider = json!({
        "name": "cctui-gateway",
        "base_url": base_url,
        "wire_api": "responses",
    });
    match env.get("OPENAI_API_KEY").filter(|k| !k.is_empty()) {
        Some(key) => headers["authorization"] = json!(format!("Bearer {key}")),
        None => provider["env_key"] = json!("OPENAI_API_KEY"),
    }
    provider["http_headers"] = headers;
    Some(("cctui".to_owned(), json!({"model_providers": {"cctui": provider}})))
}

/// `"default"` or `"fast"` (codex maps `fast` → request tier `priority`).
/// Anything else resolves to `None`: supplying no tier beats guessing one.
#[must_use]
pub fn normalize_service_tier(raw: Option<&str>) -> Option<String> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "default" => Some("default".to_owned()),
        "fast" => Some("fast".to_owned()),
        _ => None,
    }
}

#[must_use]
pub fn service_tier_from_settings(settings: Option<&Value>) -> Option<String> {
    normalize_service_tier(settings?.get("service_tier").and_then(Value::as_str))
}

/// Everything codex does not persist per thread, so every
/// `thread/{start,resume,fork}` — including a rejoin after a shared-connection
/// drop — must re-supply it. The only place those params are built.
///
/// The tier rides both the native `serviceTier` param and the `config`
/// overlay: codex persists neither, only `model_provider`, so a resume that
/// omits it silently falls back to codex's own `priority` default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadConfig {
    pub env: std::collections::BTreeMap<String, String>,
    pub service_tier: Option<String>,
    /// `-c`-style overrides (TOML literals) that a stdio child would take on
    /// its command line. Empty on stdio; on a shared app-server there is no
    /// per-session process, so they ride the per-thread overlay instead.
    pub overlay: Vec<(String, String)>,
}

impl ThreadConfig {
    #[must_use]
    pub fn new(
        env: &std::collections::BTreeMap<String, String>,
        service_tier: Option<&str>,
    ) -> Self {
        Self {
            env: env.clone(),
            service_tier: normalize_service_tier(service_tier),
            overlay: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_overlay(mut self, overlay: Vec<(String, String)>) -> Self {
        self.overlay = overlay;
        self
    }

    #[must_use]
    pub fn apply(&self, mut params: Value) -> Value {
        let Some(map) = params.as_object_mut() else { return params };
        let mut config = json!({});
        for (key, literal) in &self.overlay {
            if let Some(value) = toml_literal_to_json(literal) {
                config[key.as_str()] = value;
            }
        }
        if let Some((provider, block)) = gateway_thread_config(&self.env) {
            map.insert("modelProvider".to_owned(), json!(provider));
            if let (Some(dst), Some(src)) = (config.as_object_mut(), block.as_object()) {
                dst.extend(src.clone());
            }
        }
        if let Some(tier) = &self.service_tier {
            map.insert("serviceTier".to_owned(), json!(tier));
            config["service_tier"] = json!(tier);
        }
        if config.as_object().is_some_and(|c| !c.is_empty()) {
            map.insert("config".to_owned(), config);
        }
        params
    }

    #[must_use]
    pub fn start_params(&self, cwd: &str) -> Value {
        self.apply(json!({"cwd": cwd}))
    }

    #[must_use]
    pub fn resume_params(&self, thread_id: &str, cwd: &str) -> Value {
        self.apply(json!({"threadId": thread_id, "cwd": cwd}))
    }

    #[must_use]
    pub fn fork_params(&self, parent_thread_id: &str, cwd: &str) -> Value {
        self.apply(json!({"threadId": parent_thread_id, "cwd": cwd}))
    }
}

fn toml_literal_to_json(literal: &str) -> Option<Value> {
    let table: toml::Table = toml::from_str(&format!("v = {literal}")).ok()?;
    serde_json::to_value(table.get("v")?).ok()
}

#[cfg(test)]
fn thread_start_req(
    cwd: &str,
    env: &std::collections::BTreeMap<String, String>,
    service_tier: Option<&str>,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_THREAD_START,
        "method": "thread/start",
        "params": ThreadConfig::new(env, service_tier).start_params(cwd),
    })
}

#[cfg(test)]
fn thread_resume_req(
    thread_id: &str,
    cwd: &str,
    env: &std::collections::BTreeMap<String, String>,
    service_tier: Option<&str>,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_THREAD_START,
        "method": "thread/resume",
        "params": ThreadConfig::new(env, service_tier).resume_params(thread_id, cwd),
    })
}

#[cfg(test)]
/// Fork an existing thread into a brand-new one seeded from its history.
/// The app-server returns a fresh `thread` (its own id) just like
/// `thread/start`, so the response is parsed through the same `ID_THREAD_START`
/// path. Model/effort overrides ride on the subprocess `-c` flags (set in the
/// command pump), mirroring the spawn path, so they apply to the forked thread.
fn thread_fork_req(
    parent_thread_id: &str,
    cwd: &str,
    env: &std::collections::BTreeMap<String, String>,
    service_tier: Option<&str>,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_THREAD_START,
        "method": "thread/fork",
        "params": ThreadConfig::new(env, service_tier).fork_params(parent_thread_id, cwd),
    })
}

pub(super) fn thread_name_set_req(id: i64, thread_id: &str, name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "thread/name/set",
        "params": {"threadId": thread_id, "name": name},
    })
}

/// Build the `input` array for a turn. Staged image attachments ride
/// as native `localImage` items so codex feeds the picture to the model; every
/// other staged file keeps the path/text semantics — its absolute path is
/// listed in the text item, matching the adapter-neutral mid-chat injection.
/// The array is never empty: a turn with only images still carries a text item
/// so an image-only prompt is valid.
fn turn_input_items(text: &str, attachments: &[String]) -> Vec<Value> {
    use std::fmt::Write as _;

    let mut body = text.to_owned();
    let non_images: Vec<&str> = attachments
        .iter()
        .map(String::as_str)
        .filter(|p| !crate::adapters::uploads::is_image_path(p))
        .collect();
    if !non_images.is_empty() {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str("Attached files:");
        for p in non_images {
            let _ = write!(body, "\n  - {p}");
        }
    }

    let mut items = Vec::new();
    if !body.is_empty() {
        items.push(json!({"type": "text", "text": body}));
    }
    for p in attachments.iter().filter(|p| crate::adapters::uploads::is_image_path(p)) {
        items.push(json!({"type": "localImage", "path": p}));
    }
    if items.is_empty() {
        items.push(json!({"type": "text", "text": ""}));
    }
    items
}

/// Build a `turn/start`. An in-place model/effort change rides here
/// as a per-turn override that codex promotes to the later default — the stable
/// alternative to the `experimentalApi`-gated `thread/settings/update`. Only
/// set fields are sent so an unchanged setting keeps codex's own default.
/// Staged attachments become native image / path-in-text inputs.
pub(super) fn turn_start_req(
    id: i64,
    thread_id: &str,
    text: &str,
    attachments: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_owned(), json!(thread_id));
    params.insert("input".to_owned(), json!(turn_input_items(text, attachments)));
    if let Some(model) = model {
        params.insert("model".to_owned(), json!(model));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_owned(), json!(effort));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "turn/start", "params": params})
}

/// Steer a user message into the currently active turn. Unlike
/// `turn/start` — which codex rejects while a turn is in flight — `turn/steer`
/// appends the input to the running turn. `expectedTurnId` is a precondition:
/// the request fails if it no longer matches the active turn (it just ended),
/// which the driver recovers from by falling back to `turn/start`. Attachments
/// build the same native image / path-in-text inputs as a start.
pub(super) fn turn_steer_req(
    id: i64,
    thread_id: &str,
    expected_turn_id: &str,
    text: &str,
    attachments: &[String],
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "turn/steer",
        "params": {
            "threadId": thread_id,
            "expectedTurnId": expected_turn_id,
            "input": turn_input_items(text, attachments),
        },
    })
}

/// Interrupt the active turn. `TurnInterruptParams` requires both
/// `threadId` and `turnId`; codex rejects the request with `-32602` otherwise.
pub(super) fn turn_interrupt_req(id: i64, thread_id: &str, turn_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "turn/interrupt",
        "params": {"threadId": thread_id, "turnId": turn_id},
    })
}

#[cfg(test)]
mod tests {
    use super::super::lifecycle::{LifecycleOp, thread_lifecycle_req};
    use super::*;

    #[test]
    fn thread_info_reads_session_id_and_path() {
        let result = json!({"thread": {
            "id": "019e6628-af3f-7131",
            "sessionId": "019e6628-af3f-7131",
            "cwd": "/tmp",
            "path": "/home/u/.codex/sessions/2026/05/27/rollout-x-019e6628.jsonl",
        }});
        let info = thread_info(&result).expect("thread info");
        assert_eq!(info.thread_id, "019e6628-af3f-7131");
        assert_eq!(info.cwd.as_deref(), Some("/tmp"));
        assert!(info.rollout_path.unwrap().ends_with("019e6628.jsonl"));
    }

    #[test]
    fn initialize_declares_capabilities_and_handshake() {
        let init = initialize_req();
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["capabilities"]["experimentalApi"], false);
        assert_eq!(init["params"]["capabilities"]["requestAttestation"], false);
        assert_eq!(init["params"]["clientInfo"]["name"], "cctui");

        let done = initialized_notification();
        assert_eq!(done["method"], "initialized");
        assert!(done.get("id").is_none(), "initialized is a notification, not a request");
    }

    #[test]
    fn record_codex_version_extracts_from_user_agent() {
        let resp = json!({
            "id": 1,
            "result": {
                "userAgent": "cctui/0.144.1 (Ubuntu 24.4.0; x86_64) xterm-256color (cctui; 0.0.0)",
                "platformOs": "linux",
            },
        });
        assert_eq!(record_codex_version(&resp).as_deref(), Some("0.144.1"));

        let missing = json!({"id": 1, "result": {"platformOs": "linux"}});
        assert_eq!(record_codex_version(&missing), None);
    }

    #[test]
    fn request_builders_shape() {
        assert_eq!(initialize_req()["method"], "initialize");
        assert_eq!(
            thread_start_req("/tmp", &std::collections::BTreeMap::default(), None)["params"]["cwd"],
            "/tmp"
        );
        let resume =
            thread_resume_req("tid", "/repo", &std::collections::BTreeMap::default(), None);
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "tid");
        assert_eq!(resume["params"]["cwd"], "/repo");
        let fork =
            thread_fork_req("parent-tid", "/repo", &std::collections::BTreeMap::default(), None);
        assert_eq!(fork["method"], "thread/fork");
        assert_eq!(fork["params"]["threadId"], "parent-tid");
        assert_eq!(fork["params"]["cwd"], "/repo");
        let rename = thread_name_set_req(101, "tid", "build fix");
        assert_eq!(rename["method"], "thread/name/set");
        assert_eq!(rename["id"], 101);
        assert_eq!(rename["params"]["threadId"], "tid");
        assert_eq!(rename["params"]["name"], "build fix");
        let turn = turn_start_req(100, "tid", "hello", &[], None, None);
        assert_eq!(turn["params"]["threadId"], "tid");
        assert_eq!(turn["params"]["input"][0]["text"], "hello");
    }

    #[test]
    fn turn_start_req_carries_only_provided_overrides() {
        // No override — model/effort keys absent so codex keeps its defaults.
        let plain = turn_start_req(100, "tid", "hi", &[], None, None);
        assert!(plain["params"].get("model").is_none());
        assert!(plain["params"].get("effort").is_none());
        // Both overrides ride the turn (per-turn model change).
        let both = turn_start_req(101, "tid", "hi", &[], Some("gpt-5-codex"), Some("high"));
        assert_eq!(both["method"], "turn/start");
        assert_eq!(both["params"]["model"], "gpt-5-codex");
        assert_eq!(both["params"]["effort"], "high");
        // Model only — effort key must be absent.
        let model_only = turn_start_req(102, "tid", "hi", &[], Some("gpt-5-codex"), None);
        assert_eq!(model_only["params"]["model"], "gpt-5-codex");
        assert!(model_only["params"].get("effort").is_none());
    }

    #[test]
    fn turn_input_items_sends_images_native_and_files_in_text() {
        let attachments = vec![
            "/tmp/cctui-uploads/s/diagram.png".to_owned(),
            "/tmp/cctui-uploads/s/report.pdf".to_owned(),
            "/tmp/cctui-uploads/s/photo.JPEG".to_owned(),
        ];
        let items = turn_input_items("look at these", &attachments);
        // Text item first: prompt plus a listing of the non-image file only.
        assert_eq!(items[0]["type"], "text");
        let text = items[0]["text"].as_str().unwrap();
        assert!(text.contains("look at these"));
        assert!(text.contains("report.pdf"));
        assert!(!text.contains("diagram.png"), "images are native, not text paths");
        // Both images become localImage inputs carrying their local paths.
        let images: Vec<&Value> = items.iter().filter(|i| i["type"] == "localImage").collect();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0]["path"], "/tmp/cctui-uploads/s/diagram.png");
        assert_eq!(images[1]["path"], "/tmp/cctui-uploads/s/photo.JPEG");
    }

    #[test]
    fn turn_input_items_image_only_prompt_has_no_empty_text_gap() {
        // An image-only spawn (no prompt text) still yields a valid input array:
        // just the localImage item, no stray empty text item.
        let items = turn_input_items("", &["/tmp/s/shot.png".to_owned()]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "localImage");
        // No attachments and no text falls back to a single empty text item.
        let empty = turn_input_items("", &[]);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0]["type"], "text");
    }

    #[test]
    fn turn_steer_req_shape() {
        let req = turn_steer_req(100, "tid", "turn-1", "keep going", &[]);
        assert_eq!(req["method"], "turn/steer");
        assert_eq!(req["id"], 100);
        assert_eq!(req["params"]["threadId"], "tid");
        assert_eq!(req["params"]["expectedTurnId"], "turn-1");
        assert_eq!(req["params"]["input"][0]["type"], "text");
        assert_eq!(req["params"]["input"][0]["text"], "keep going");
    }

    #[test]
    fn turn_interrupt_req_shape() {
        let req = turn_interrupt_req(7, "tid", "turn-1");
        assert_eq!(req["method"], "turn/interrupt");
        assert_eq!(req["id"], 7);
        assert_eq!(req["params"]["threadId"], "tid");
        assert_eq!(req["params"]["turnId"], "turn-1");
    }

    // --- outbound builders vs retained schema `required` -----------

    /// `required` param keys for `method` from the retained
    /// `ClientRequest` schema, following the `params.$ref` one level.
    fn schema_required_params(schema: &Value, method: &str) -> Vec<String> {
        let variants = schema["definitions"]["ClientRequest"]["oneOf"]
            .as_array()
            .expect("ClientRequest.oneOf");
        let variant = variants
            .iter()
            .find(|v| v["properties"]["method"]["enum"][0] == method)
            .unwrap_or_else(|| panic!("schema has no ClientRequest variant for `{method}`"));
        let params = &variant["properties"]["params"];
        let resolved = params.get("$ref").and_then(Value::as_str).map_or(params, |r| {
            schema
                .pointer(r.trim_start_matches('#'))
                .unwrap_or_else(|| panic!("unresolvable $ref {r} for `{method}`"))
        });
        resolved["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Every outbound request builder must carry all `required` params of
    /// its method in the retained schema, so a protocol bump that adds a
    /// required field fails here instead of as `-32602` at runtime.
    #[test]
    fn outbound_request_builders_satisfy_schema_required_params() {
        let schema: Value =
            serde_json::from_str(include_str!("../schema/codex_app_server_protocol.schemas.json"))
                .expect("retained schema bundle is valid JSON");
        let reqs = [
            initialize_req(),
            thread_start_req("/cwd", &std::collections::BTreeMap::default(), None),
            thread_resume_req("tid", "/cwd", &std::collections::BTreeMap::default(), None),
            thread_fork_req("tid", "/cwd", &std::collections::BTreeMap::default(), None),
            thread_name_set_req(1, "tid", "name"),
            thread_lifecycle_req(2, LifecycleOp::Archive, "tid"),
            thread_lifecycle_req(3, LifecycleOp::Unarchive, "tid"),
            thread_lifecycle_req(4, LifecycleOp::Delete, "tid"),
            turn_start_req(5, "tid", "hi", &[], None, None),
            turn_steer_req(6, "tid", "turn-1", "hi", &[]),
            turn_interrupt_req(7, "tid", "turn-1"),
        ];
        for req in reqs {
            let method = req["method"].as_str().expect("method");
            let params = req["params"].as_object().expect("params object");
            for key in schema_required_params(&schema, method) {
                assert!(
                    params.contains_key(&key),
                    "`{method}` builder is missing required param `{key}`"
                );
            }
        }
    }

    #[test]
    fn per_thread_config_carries_a_literal_bearer_not_an_env_key() {
        let env: std::collections::BTreeMap<String, String> = [
            ("OPENAI_BASE_URL".to_owned(), "https://gw.example/v1".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "SECRET-A".to_owned()),
        ]
        .into_iter()
        .collect();
        let (provider, config) = gateway_thread_config(&env).expect("gateway-bound session");
        assert_eq!(provider, "cctui");
        let p = &config["model_providers"]["cctui"];
        assert_eq!(p["base_url"], "https://gw.example/v1");
        assert_eq!(p["wire_api"], "responses");
        assert_eq!(p["http_headers"]["authorization"], "Bearer SECRET-A");
        assert_eq!(p["http_headers"]["x-openai-actor-authorization"], "cctui-gateway");
        assert!(p.get("env_key").is_none(), "a literal bearer must not also indirect via env");
    }

    /// Without a credential the definition must still be emitted, or codex
    /// fails config load on a rollout that persisted `model_provider`.
    #[test]
    fn per_thread_config_falls_back_to_env_key_without_a_credential() {
        let env: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_BASE_URL".to_owned(), "https://gw.example/v1".to_owned()))
                .collect();
        let (_, config) = gateway_thread_config(&env).expect("gateway-bound session");
        let p = &config["model_providers"]["cctui"];
        assert_eq!(p["env_key"], "OPENAI_API_KEY");
        assert!(p["http_headers"].get("authorization").is_none());
    }

    #[test]
    fn an_unbound_session_keeps_the_default_provider() {
        assert!(gateway_thread_config(&std::collections::BTreeMap::new()).is_none());
        let req = thread_start_req("/tmp", &std::collections::BTreeMap::new(), None);
        assert!(req["params"].get("config").is_none());
        assert!(req["params"].get("modelProvider").is_none());
    }

    #[test]
    fn start_resume_and_fork_all_carry_the_per_thread_credential() {
        let env: std::collections::BTreeMap<String, String> = [
            ("OPENAI_BASE_URL".to_owned(), "https://gw.example/v1".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "SECRET-B".to_owned()),
        ]
        .into_iter()
        .collect();
        for req in [
            thread_start_req("/repo", &env, None),
            thread_resume_req("tid", "/repo", &env, None),
            thread_fork_req("tid", "/repo", &env, None),
        ] {
            let params = &req["params"];
            assert_eq!(params["modelProvider"], "cctui", "{}", req["method"]);
            assert_eq!(
                params["config"]["model_providers"]["cctui"]["http_headers"]["authorization"],
                "Bearer SECRET-B",
                "{}",
                req["method"]
            );
        }
    }

    #[test]
    fn service_tier_normalization_accepts_only_the_two_codex_tiers() {
        assert_eq!(normalize_service_tier(Some("fast")).as_deref(), Some("fast"));
        assert_eq!(normalize_service_tier(Some(" FAST ")).as_deref(), Some("fast"));
        assert_eq!(normalize_service_tier(Some("default")).as_deref(), Some("default"));
        assert_eq!(normalize_service_tier(Some("priority")), None);
        assert_eq!(normalize_service_tier(Some("")), None);
        assert_eq!(normalize_service_tier(None), None);
    }

    #[test]
    fn service_tier_reads_out_of_the_served_gateway_settings() {
        assert_eq!(
            service_tier_from_settings(Some(&json!({"service_tier": "fast"}))).as_deref(),
            Some("fast")
        );
        assert_eq!(
            service_tier_from_settings(Some(&json!({"service_tier": "default"}))).as_deref(),
            Some("default")
        );
        assert_eq!(service_tier_from_settings(Some(&json!({}))), None);
        assert_eq!(service_tier_from_settings(None), None);
    }

    #[test]
    fn start_resume_and_fork_all_carry_the_per_session_service_tier() {
        let env = std::collections::BTreeMap::default();
        for tier in ["fast", "default"] {
            for req in [
                thread_start_req("/repo", &env, Some(tier)),
                thread_resume_req("tid", "/repo", &env, Some(tier)),
                thread_fork_req("tid", "/repo", &env, Some(tier)),
            ] {
                let params = &req["params"];
                assert_eq!(params["config"]["service_tier"], tier, "{}", req["method"]);
                assert_eq!(params["serviceTier"], tier, "{}", req["method"]);
            }
        }
    }

    #[test]
    fn a_session_with_no_tier_supplies_none_on_any_thread_op() {
        let env = std::collections::BTreeMap::default();
        for req in [
            thread_start_req("/repo", &env, None),
            thread_resume_req("tid", "/repo", &env, None),
            thread_fork_req("tid", "/repo", &env, None),
        ] {
            let params = &req["params"];
            assert!(params.get("serviceTier").is_none(), "{}", req["method"]);
            assert!(
                params.get("config").and_then(|c| c.get("service_tier")).is_none(),
                "{}",
                req["method"]
            );
        }
    }

    #[test]
    fn the_gateway_config_block_and_the_tier_coexist() {
        let env: std::collections::BTreeMap<String, String> = [
            ("OPENAI_BASE_URL".to_owned(), "https://gw.example/v1".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "SECRET-C".to_owned()),
        ]
        .into_iter()
        .collect();
        let params = &thread_resume_req("tid", "/repo", &env, Some("fast"))["params"];
        assert_eq!(params["config"]["service_tier"], "fast");
        assert_eq!(
            params["config"]["model_providers"]["cctui"]["http_headers"]["authorization"],
            "Bearer SECRET-C"
        );
    }

    #[test]
    fn one_thread_config_resupplies_the_same_block_on_start_resume_and_fork() {
        let env: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_BASE_URL".to_owned(), "https://gw.example/v1".to_owned()))
                .collect();
        let tc = ThreadConfig::new(&env, Some("fast"));
        let start = tc.start_params("/repo");
        let resume = tc.resume_params("tid", "/repo");
        let fork = tc.fork_params("tid", "/repo");
        assert_eq!(start["config"], resume["config"]);
        assert_eq!(resume["config"], fork["config"]);
        assert_eq!(resume["serviceTier"], "fast");
        assert_eq!(resume["modelProvider"], "cctui");
        assert_eq!(resume["threadId"], "tid");
    }

    #[test]
    fn thread_config_drops_an_unknown_tier() {
        let tc = ThreadConfig::new(&std::collections::BTreeMap::new(), Some("priority"));
        assert_eq!(tc.service_tier, None);
        assert!(tc.resume_params("tid", "/repo").get("serviceTier").is_none());
    }

    #[test]
    fn the_overlay_carries_process_knobs_as_typed_config_values() {
        let tc = ThreadConfig::new(&std::collections::BTreeMap::new(), None).with_overlay(vec![
            ("approval_policy".to_owned(), "\"never\"".to_owned()),
            ("mcp_servers.cctui.args".to_owned(), "[\"a\", \"b\"]".to_owned()),
            ("broken".to_owned(), "not toml [".to_owned()),
        ]);
        let params = tc.resume_params("tid", "/repo");
        assert_eq!(params["config"]["approval_policy"], "never");
        assert_eq!(params["config"]["mcp_servers.cctui.args"], json!(["a", "b"]));
        assert!(params["config"].get("broken").is_none());
    }

    #[test]
    fn the_gateway_block_wins_over_an_overlay_key_of_the_same_name() {
        let env: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_BASE_URL".to_owned(), "https://gw.example/v1".to_owned()))
                .collect();
        let tc = ThreadConfig::new(&env, None)
            .with_overlay(vec![("model_providers".to_owned(), "\"hijack\"".to_owned())]);
        let params = tc.start_params("/repo");
        assert_eq!(params["config"]["model_providers"]["cctui"]["name"], "cctui-gateway");
    }
}
