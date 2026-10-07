//! The ACP v1 requests the adapter sends and the answers it reads, as JSON.
//!
//! Requests go out untyped so an agent still on the legacy `models` /
//! `session/set_model` pair, or one stamping token counts into `_meta`, is
//! read in full rather than trimmed by a typed schema. The SDK's own types
//! stay inside `connection.rs`.

use std::path::Path;

use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u64 = 1;
pub const CLIENT_NAME: &str = "cctui-daemon";

/// JSON-RPC error code an agent answers `session/new` with when it has no
/// credentials.
pub const AUTH_REQUIRED: i64 = -32000;

#[must_use]
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
        },
        "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
    })
}

/// What `initialize` told us, kept raw for the diagnose report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitInfo {
    pub protocol_version: u64,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub image_prompts: bool,
    pub can_close: bool,
    pub can_load: bool,
    pub auth_methods: Vec<String>,
}

#[must_use]
pub fn parse_initialize(resp: &Value) -> InitInfo {
    let caps = resp.get("agentCapabilities").cloned().unwrap_or(Value::Null);
    InitInfo {
        protocol_version: resp.get("protocolVersion").and_then(Value::as_u64).unwrap_or_default(),
        agent_name: resp.pointer("/agentInfo/name").and_then(Value::as_str).map(str::to_owned),
        agent_version: resp
            .pointer("/agentInfo/version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        image_prompts: caps.pointer("/promptCapabilities/image").and_then(Value::as_bool)
            == Some(true),
        can_close: caps.pointer("/sessionCapabilities/close").is_some_and(|v| !v.is_null()),
        can_load: caps.get("loadSession").and_then(Value::as_bool) == Some(true),
        auth_methods: resp
            .get("authMethods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect(),
    }
}

#[must_use]
pub fn new_session_params(cwd: &Path) -> Value {
    json!({ "cwd": cwd, "mcpServers": [] })
}

/// One entry of a mode or model list, from either vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// A list with a current selection: legacy `modes` and `models`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub current: String,
    pub available: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewSession {
    pub session_id: String,
    pub modes: Option<Selection>,
    /// The legacy `models` block agents like gemini still send.
    pub models: Option<Selection>,
    /// The `configOptions` array, raw.
    pub config_options: Vec<Value>,
}

fn selection(v: &Value, current_key: &str, list_key: &str, id_key: &str) -> Option<Selection> {
    let current = v.get(current_key)?.as_str()?.to_owned();
    let available = v
        .get(list_key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(Choice {
                id: c.get(id_key).and_then(Value::as_str)?.to_owned(),
                name: c.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                description: c.get("description").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect();
    Some(Selection { current, available })
}

pub fn parse_new_session(resp: &Value) -> anyhow::Result<NewSession> {
    let session_id = resp
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("session/new answered without a sessionId"))?;
    Ok(NewSession {
        session_id: session_id.to_owned(),
        modes: resp
            .get("modes")
            .and_then(|m| selection(m, "currentModeId", "availableModes", "id")),
        models: resp
            .get("models")
            .and_then(|m| selection(m, "currentModelId", "availableModels", "modelId")),
        config_options: resp
            .get("configOptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

/// A staged image attachment, base64 with its mime type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub data_b64: String,
    pub mime_type: String,
}

#[must_use]
pub fn prompt_params(session_id: &str, text: &str, images: &[Image]) -> Value {
    let mut blocks = vec![json!({ "type": "text", "text": text })];
    for image in images {
        blocks
            .push(json!({ "type": "image", "data": image.data_b64, "mimeType": image.mime_type }));
    }
    json!({ "sessionId": session_id, "prompt": blocks })
}

/// Per-turn token counts, from `PromptResponse.usage` when the agent sends
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    pub cached_write_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOutcome {
    pub stop_reason: String,
    pub usage: Option<TurnUsage>,
}

#[must_use]
pub fn parse_prompt(resp: &Value) -> PromptOutcome {
    let count = |u: &Value, key: &str| u.get(key).and_then(Value::as_u64).unwrap_or_default();
    PromptOutcome {
        stop_reason: resp
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("end_turn")
            .to_owned(),
        usage: resp.get("usage").filter(|u| u.is_object()).map(|u| TurnUsage {
            input_tokens: count(u, "inputTokens"),
            output_tokens: count(u, "outputTokens"),
            cached_read_tokens: count(u, "cachedReadTokens"),
            cached_write_tokens: count(u, "cachedWriteTokens"),
        }),
    }
}

/// Token counts gemini stamps into `_meta.quota` on its updates instead of
/// sending `usage_update`.
#[must_use]
pub fn quota_tokens(meta: &Value) -> Option<(u64, u64)> {
    let quota = meta.get("quota")?;
    let input = quota
        .get("inputTokens")
        .or_else(|| quota.get("input_tokens"))
        .or_else(|| quota.get("promptTokens"))
        .and_then(Value::as_u64)?;
    let output = quota
        .get("outputTokens")
        .or_else(|| quota.get("output_tokens"))
        .or_else(|| quota.get("candidatesTokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    Some((input, output))
}

#[must_use]
pub fn cancel_params(session_id: &str) -> Value {
    json!({ "sessionId": session_id })
}

#[must_use]
pub fn close_params(session_id: &str) -> Value {
    json!({ "sessionId": session_id })
}

#[must_use]
pub fn set_mode_params(session_id: &str, mode_id: &str) -> Value {
    json!({ "sessionId": session_id, "modeId": mode_id })
}

#[must_use]
pub fn set_config_option_params(session_id: &str, config_id: &str, value: &str) -> Value {
    json!({ "sessionId": session_id, "configId": config_id, "value": value })
}

#[must_use]
pub fn set_model_params(session_id: &str, model_id: &str) -> Value {
    json!({ "sessionId": session_id, "modelId": model_id })
}

/// One option of a `session/request_permission`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    /// `allow_once` / `allow_always` / `reject_once` / `reject_always`.
    pub kind: String,
}

#[must_use]
pub fn parse_permission_options(request: &Value) -> Vec<PermissionOption> {
    request
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(PermissionOption {
                id: o.get("optionId").and_then(Value::as_str)?.to_owned(),
                kind: o.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// The `session/request_permission` result for a yes/no answer.
///
/// The once variant of the matching kind, then the always variant, then any
/// option of that polarity; a refusal with nothing to pick is `cancelled`.
#[must_use]
pub fn permission_response(options: &[PermissionOption], allow: bool) -> Value {
    let (first, second) =
        if allow { ("allow_once", "allow_always") } else { ("reject_once", "reject_always") };
    let picked = options
        .iter()
        .find(|o| o.kind == first)
        .or_else(|| options.iter().find(|o| o.kind == second))
        .or_else(|| {
            options.iter().find(|o| o.kind.starts_with(if allow { "allow" } else { "reject" }))
        });
    match picked {
        Some(o) => json!({ "outcome": { "outcome": "selected", "optionId": o.id } }),
        None if allow => options.first().map_or(
            json!({ "outcome": { "outcome": "cancelled" } }),
            |o| json!({ "outcome": { "outcome": "selected", "optionId": o.id } }),
        ),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

/// The `-32000` the agent answers `session/new` with when it is not logged
/// in, worded for the user.
#[must_use]
pub fn auth_required_detail(agent: &str, machine: Option<&str>) -> String {
    match machine {
        Some(m) => format!("{agent} is not logged in: run `{agent}` once on {m} to log in"),
        None => format!("{agent} is not logged in: run `{agent}` once on this machine to log in"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_no_client_fs_or_terminal() {
        let p = initialize_params();
        assert_eq!(p["protocolVersion"], 1);
        assert_eq!(p["clientCapabilities"]["fs"]["readTextFile"], false);
        assert_eq!(p["clientCapabilities"]["fs"]["writeTextFile"], false);
        assert_eq!(p["clientCapabilities"]["terminal"], false);
        assert_eq!(p["clientInfo"]["name"], CLIENT_NAME);
    }

    #[test]
    fn initialize_response_yields_agent_info_and_capabilities() {
        let info = parse_initialize(&json!({
            "protocolVersion": 1,
            "agentInfo": { "name": "gemini-cli", "version": "0.62.0" },
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": { "image": true },
                "sessionCapabilities": { "close": {} },
            },
            "authMethods": [{ "id": "oauth-personal", "name": "Log in with Google" }],
        }));
        assert_eq!(info.agent_name.as_deref(), Some("gemini-cli"));
        assert_eq!(info.agent_version.as_deref(), Some("0.62.0"));
        assert!(info.image_prompts);
        assert!(info.can_close);
        assert!(info.can_load);
        assert_eq!(info.auth_methods, ["oauth-personal"]);
        let bare = parse_initialize(&json!({ "protocolVersion": 1 }));
        assert!(!bare.image_prompts && !bare.can_close && bare.agent_version.is_none());
    }

    #[test]
    fn new_session_reads_modes_legacy_models_and_config_options() {
        let s = parse_new_session(&json!({
            "sessionId": "sess_1",
            "modes": {
                "currentModeId": "default",
                "availableModes": [
                    { "id": "default", "name": "Default" },
                    { "id": "yolo", "name": "YOLO", "description": "no prompts" },
                ],
            },
            "models": {
                "currentModelId": "gemini-2.5-pro",
                "availableModels": [
                    { "modelId": "gemini-2.5-pro", "name": "Gemini 2.5 Pro" },
                    { "modelId": "gemini-2.5-flash", "name": "Gemini 2.5 Flash" },
                ],
            },
            "configOptions": [{ "id": "model", "category": "model", "type": "select" }],
        }))
        .unwrap();
        assert_eq!(s.session_id, "sess_1");
        let modes = s.modes.unwrap();
        assert_eq!(modes.current, "default");
        assert_eq!(modes.available[1].description.as_deref(), Some("no prompts"));
        let models = s.models.unwrap();
        assert_eq!(models.current, "gemini-2.5-pro");
        assert_eq!(models.available[1].id, "gemini-2.5-flash");
        assert_eq!(s.config_options.len(), 1);
        assert!(parse_new_session(&json!({})).is_err());
        let bare = parse_new_session(&json!({ "sessionId": "s" })).unwrap();
        assert!(bare.modes.is_none() && bare.models.is_none() && bare.config_options.is_empty());
    }

    #[test]
    fn a_prompt_carries_text_then_images() {
        let p = prompt_params(
            "s",
            "look",
            &[Image { data_b64: "AAAA".into(), mime_type: "image/png".into() }],
        );
        assert_eq!(p["sessionId"], "s");
        assert_eq!(p["prompt"][0], json!({ "type": "text", "text": "look" }));
        assert_eq!(p["prompt"][1]["type"], "image");
        assert_eq!(p["prompt"][1]["mimeType"], "image/png");
    }

    #[test]
    fn a_prompt_response_yields_the_stop_reason_and_optional_usage() {
        let out = parse_prompt(&json!({
            "stopReason": "end_turn",
            "usage": { "totalTokens": 30, "inputTokens": 20, "outputTokens": 10, "cachedReadTokens": 5 },
        }));
        assert_eq!(out.stop_reason, "end_turn");
        assert_eq!(
            out.usage,
            Some(TurnUsage {
                input_tokens: 20,
                output_tokens: 10,
                cached_read_tokens: 5,
                cached_write_tokens: 0
            })
        );
        let bare = parse_prompt(&json!({ "stopReason": "cancelled" }));
        assert_eq!(bare.stop_reason, "cancelled");
        assert!(bare.usage.is_none());
    }

    #[test]
    fn gemini_quota_meta_yields_token_counts_without_cost() {
        assert_eq!(
            quota_tokens(&json!({ "quota": { "inputTokens": 100, "outputTokens": 7 } })),
            Some((100, 7))
        );
        assert_eq!(quota_tokens(&json!({ "quota": { "promptTokens": 3 } })), Some((3, 0)));
        assert_eq!(quota_tokens(&json!({})), None);
    }

    #[test]
    fn permission_answers_pick_the_once_option_of_the_right_polarity() {
        let options = parse_permission_options(&json!({
            "options": [
                { "optionId": "always", "name": "Always", "kind": "allow_always" },
                { "optionId": "once", "name": "Once", "kind": "allow_once" },
                { "optionId": "no", "name": "No", "kind": "reject_once" },
            ]
        }));
        assert_eq!(permission_response(&options, true)["outcome"]["optionId"], "once");
        assert_eq!(permission_response(&options, false)["outcome"]["optionId"], "no");
        let allow_only = &options[..2];
        assert_eq!(permission_response(allow_only, false)["outcome"]["outcome"], "cancelled");
        assert_eq!(permission_response(&options[..1], true)["outcome"]["optionId"], "always");
        assert_eq!(permission_response(&[], true)["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn the_login_hint_names_the_agent_and_the_machine() {
        assert_eq!(
            auth_required_detail("gemini", Some("workbench")),
            "gemini is not logged in: run `gemini` once on workbench to log in"
        );
        assert!(auth_required_detail("gemini", None).contains("this machine"));
    }

    #[test]
    fn set_requests_carry_their_ids() {
        assert_eq!(set_mode_params("s", "yolo"), json!({ "sessionId": "s", "modeId": "yolo" }));
        assert_eq!(
            set_config_option_params("s", "model", "pro"),
            json!({ "sessionId": "s", "configId": "model", "value": "pro" })
        );
        assert_eq!(set_model_params("s", "pro"), json!({ "sessionId": "s", "modelId": "pro" }));
        assert_eq!(cancel_params("s"), json!({ "sessionId": "s" }));
        assert_eq!(close_params("s"), json!({ "sessionId": "s" }));
    }
}
