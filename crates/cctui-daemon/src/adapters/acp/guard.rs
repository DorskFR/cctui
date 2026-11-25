//! `cctui-guard` steering for ACP permission requests. Anything but an
//! explicit allow rejects, unreachable guard included, like the other
//! harnesses' shims.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};

use crate::guardwatch::URL_ENV;

const TIMEOUT: Duration = Duration::from_secs(10);
const FAIL: &str = "— failing closed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Reject(String),
}

#[derive(Debug, Clone)]
pub struct Guard {
    url: String,
    client: reqwest::Client,
}

impl Guard {
    /// The daemon's own guard wins over one named in the spawn env.
    #[must_use]
    pub fn resolve(env: &BTreeMap<String, String>) -> Option<Self> {
        let url = crate::guardwatch::url().or_else(|| {
            env.get(URL_ENV)
                .map(|u| u.trim().trim_end_matches('/').to_owned())
                .filter(|u| !u.is_empty())
        })?;
        crate::install_crypto_provider();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(TIMEOUT)
            .build()
            .map_err(|err| tracing::warn!(%err, "acp guard http client"))
            .ok()?;
        Some(Self { url, client })
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn check(&self, tool_call: &Value) -> Verdict {
        let body = check_body(tool_call);
        let sent = self
            .client
            .post(format!("{}/check?dialect=acp", self.url))
            .json(&body)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        let response = match sent {
            Ok(response) => response,
            Err(err) => {
                return Verdict::Reject(format!(
                    "cctui-guard unreachable or failing at {} ({err}) {FAIL}",
                    self.url
                ));
            }
        };
        response.json::<Value>().await.map_or_else(
            |_| Verdict::Reject(format!("cctui-guard: invalid guard response {FAIL}")),
            |verdict| read_verdict(&verdict),
        )
    }
}

/// The guard's rules are written in Claude tool names.
#[must_use]
pub fn check_body(tool_call: &Value) -> Value {
    let raw = tool_call.get("rawInput").filter(|v| v.is_object()).cloned().unwrap_or(json!({}));
    json!({ "tool_name": tool_name(tool_call), "tool_input": tool_input(raw) })
}

fn tool_name(tool_call: &Value) -> String {
    let kind = tool_call.get("kind").and_then(Value::as_str).unwrap_or_default();
    let mapped = match kind {
        "execute" => Some("Bash"),
        "edit" | "delete" | "move" => Some("Edit"),
        "read" => Some("Read"),
        "search" => Some("Grep"),
        "fetch" => Some("WebFetch"),
        _ => None,
    };
    mapped
        .map(str::to_owned)
        .or_else(|| {
            tool_call
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| tool_call.get("title").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "tool".to_owned())
}

fn tool_input(mut raw: Value) -> Value {
    if let Some(map) = raw.as_object_mut()
        && !map.contains_key("file_path")
        && let Some(path) = ["filePath", "path", "absolute_path"]
            .iter()
            .find_map(|k| map.get(*k).and_then(Value::as_str))
            .map(str::to_owned)
    {
        map.insert("file_path".to_owned(), Value::String(path));
    }
    raw
}

#[must_use]
pub fn read_verdict(verdict: &Value) -> Verdict {
    let outcome = verdict.pointer("/outcome/outcome").and_then(Value::as_str);
    let option = verdict.pointer("/outcome/optionId").and_then(Value::as_str);
    let reason = verdict
        .pointer("/_meta/reason")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
        .map(str::to_owned);
    match (outcome, option) {
        (Some("selected"), Some("allow_once")) => Verdict::Allow,
        (Some("selected"), Some("reject_once")) | (Some("cancelled"), _) => {
            Verdict::Reject(reason.unwrap_or_else(|| "cctui-guard: blocked".to_owned()))
        }
        _ => Verdict::Reject(format!("cctui-guard: invalid guard response {FAIL}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_allow_lets_the_request_through() {
        assert_eq!(
            read_verdict(
                &json!({ "outcome": { "outcome": "selected", "optionId": "allow_once" } })
            ),
            Verdict::Allow
        );
        assert_eq!(
            read_verdict(&json!({
                "outcome": { "outcome": "selected", "optionId": "reject_once" },
                "_meta": { "reason": "[Step 2] no" }
            })),
            Verdict::Reject("[Step 2] no".to_owned())
        );
        assert_eq!(
            read_verdict(&json!({
                "outcome": { "outcome": "cancelled" },
                "_meta": { "reason": "ask a human" }
            })),
            Verdict::Reject("ask a human".to_owned())
        );
        assert_eq!(
            read_verdict(
                &json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })
            ),
            Verdict::Reject("cctui-guard: blocked".to_owned())
        );
        for garbage in [json!({}), json!({ "allow": true }), json!({ "decision": "allow" })] {
            let Verdict::Reject(reason) = read_verdict(&garbage) else {
                panic!("{garbage} must not allow");
            };
            assert!(reason.contains("failing closed"), "{reason}");
        }
    }

    #[test]
    fn the_check_body_speaks_the_guard_tool_vocabulary() {
        let shell = json!({
            "toolCallId": "c1", "kind": "execute", "name": "run_shell_command",
            "rawInput": { "command": "git push" },
        });
        assert_eq!(
            check_body(&shell),
            json!({ "tool_name": "Bash", "tool_input": { "command": "git push" } })
        );
        let edit = json!({ "kind": "edit", "rawInput": { "filePath": "/w/a.rs" } });
        assert_eq!(check_body(&edit)["tool_name"], "Edit");
        assert_eq!(check_body(&edit)["tool_input"]["file_path"], "/w/a.rs");
        let read = json!({ "kind": "read", "rawInput": { "path": "." } });
        assert_eq!(
            check_body(&read),
            json!({ "tool_name": "Read", "tool_input": { "path": ".", "file_path": "." } })
        );
        let mcp = json!({ "kind": "other", "name": "mcp__x__y", "rawInput": null });
        assert_eq!(check_body(&mcp), json!({ "tool_name": "mcp__x__y", "tool_input": {} }));
    }

    #[test]
    fn a_spawn_env_guard_url_is_used_without_a_pod_guard() {
        if crate::guardwatch::url().is_some() {
            return;
        }
        assert!(Guard::resolve(&BTreeMap::new()).is_none());
        let env = BTreeMap::from([(URL_ENV.to_owned(), " http://127.0.0.1:9999/ ".to_owned())]);
        assert_eq!(Guard::resolve(&env).map(|g| g.url).as_deref(), Some("http://127.0.0.1:9999"));
    }
}
