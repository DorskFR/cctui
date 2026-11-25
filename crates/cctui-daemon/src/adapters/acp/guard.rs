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
    /// `Err` holds the rejection every request gets.
    client: Result<reqwest::Client, String>,
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
        let client = reqwest::Client::builder().no_proxy().timeout(TIMEOUT).build();
        Some(Self::with_client(url, client.map_err(|err| err.to_string())))
    }

    fn with_client(url: String, client: Result<reqwest::Client, String>) -> Self {
        let client = client.map_err(|err| {
            tracing::error!(%err, %url, "acp guard http client: rejecting every permission request");
            format!("cctui-guard client for {url} could not be built ({err}) {FAIL}")
        });
        Self { url, client }
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn check(&self, tool_call: &Value) -> Verdict {
        let client = match &self.client {
            Ok(client) => client,
            Err(reason) => return Verdict::Reject(reason.clone()),
        };
        let body = check_body(tool_call);
        let sent = client
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

/// The guard's rules are written in Claude tool names. A shell call goes out
/// as `Bash` only with its command: the guard allows an empty `Bash` command,
/// so one it cannot read goes out under another name.
#[must_use]
pub fn check_body(tool_call: &Value) -> Value {
    let raw = tool_call.get("rawInput").filter(|v| v.is_object()).cloned().unwrap_or(json!({}));
    let kind = tool_call.get("kind").and_then(Value::as_str).unwrap_or_default();
    let named = agent_tool_name(tool_call);
    if kind == "execute" || named.as_deref().is_some_and(is_bash) {
        let command = command_of(&raw).or_else(|| title_command(tool_call, &raw));
        if let Some(command) = command {
            let mut input = raw;
            input["command"] = Value::String(command);
            return json!({ "tool_name": "Bash", "tool_input": input });
        }
        let name = named.filter(|n| !is_bash(n)).unwrap_or_else(|| "Shell".to_owned());
        return json!({ "tool_name": name, "tool_input": tool_input(raw) });
    }
    let name = match kind {
        "edit" | "delete" | "move" => "Edit".to_owned(),
        "read" => "Read".to_owned(),
        "search" => "Grep".to_owned(),
        "fetch" => "WebFetch".to_owned(),
        _ => named
            .or_else(|| text(tool_call, "/title"))
            .filter(|n| !is_bash(n))
            .unwrap_or_else(|| "tool".to_owned()),
    };
    json!({ "tool_name": name, "tool_input": tool_input(raw) })
}

const fn is_bash(name: &str) -> bool {
    name.eq_ignore_ascii_case("bash")
}

fn text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// qwen-code names the tool in `_meta.toolName`; some agents use `name`.
fn agent_tool_name(tool_call: &Value) -> Option<String> {
    text(tool_call, "/_meta/toolName").or_else(|| text(tool_call, "/name"))
}

const COMMAND_KEYS: [&str; 4] = ["command", "cmd", "commandLine", "command_line"];

fn command_of(raw: &Value) -> Option<String> {
    COMMAND_KEYS.iter().find_map(|key| match raw.get(*key)? {
        Value::String(cmd) => Some(cmd.trim().to_owned()).filter(|c| !c.is_empty()),
        Value::Array(argv) => {
            let argv: Option<Vec<&str>> = argv.iter().map(Value::as_str).collect();
            argv.and_then(|argv| argv_command(&argv))
        }
        _ => None,
    })
}

/// `["bash", "-lc", "git push"]` is the script itself; any other argv is
/// joined as a shell would read it.
fn argv_command(argv: &[&str]) -> Option<String> {
    if let [shell, flag, script] = argv
        && matches!(shell.rsplit('/').next(), Some("sh" | "bash" | "zsh" | "dash"))
        && matches!(*flag, "-c" | "-lc" | "-ic")
    {
        return Some(script.trim().to_owned()).filter(|s| !s.is_empty());
    }
    let joined = argv.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ");
    Some(joined).filter(|j| !j.trim().is_empty())
}

fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c));
    if plain { arg.to_owned() } else { format!("'{}'", arg.replace('\'', "'\\''")) }
}

/// gemini-cli sends no `rawInput`; its shell tool's title is the command.
fn title_command(tool_call: &Value, raw: &Value) -> Option<String> {
    if raw.as_object().is_some_and(|m| !m.is_empty()) {
        return None;
    }
    text(tool_call, "/title")
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

    fn bash(command: &str) -> Value {
        json!({ "tool_name": "Bash", "tool_input": { "command": command } })
    }

    #[test]
    fn gemini_shell_calls_carry_the_command_in_the_title() {
        let gemini = json!({
            "toolCallId": "run_shell_command-1", "status": "pending",
            "title": "git push origin main", "content": [], "locations": [], "kind": "execute",
        });
        assert_eq!(check_body(&gemini), bash("git push origin main"));
        let read_outside = json!({
            "toolCallId": "x", "status": "pending", "kind": "read", "locations": [],
            "title": "Allow access to absolute path: /etc/passwd",
        });
        assert_eq!(check_body(&read_outside)["tool_name"], "Read");
    }

    #[test]
    fn qwen_calls_are_named_from_meta_and_read_from_raw_input() {
        let shell = json!({
            "toolCallId": "c1", "status": "pending", "kind": "execute",
            "title": "Push the branch", "locations": [],
            "rawInput": { "command": "git push", "is_background": false, "description": "Push" },
            "_meta": { "toolName": "run_shell_command" },
        });
        let body = check_body(&shell);
        assert_eq!(body["tool_name"], "Bash");
        assert_eq!(body["tool_input"]["command"], "git push");
        let mcp = json!({
            "toolCallId": "c2", "kind": "other", "title": "Create issue",
            "rawInput": { "title": "x" }, "_meta": { "toolName": "mcp__tracker__create" },
        });
        assert_eq!(check_body(&mcp)["tool_name"], "mcp__tracker__create");
    }

    #[test]
    fn a_command_under_another_key_or_as_argv_still_reaches_the_guard_as_bash() {
        for (raw, command) in [
            (json!({ "cmd": "rm -rf /w" }), "rm -rf /w"),
            (json!({ "commandLine": "git push" }), "git push"),
            (json!({ "command_line": "git push" }), "git push"),
            (json!({ "command": ["bash", "-lc", "git push --force"] }), "git push --force"),
            (json!({ "command": ["/bin/sh", "-c", "git push"] }), "git push"),
            (
                json!({ "command": ["git", "commit", "-m", "it's done"] }),
                "git commit -m 'it'\\''s done'",
            ),
        ] {
            let call =
                json!({ "toolCallId": "c", "kind": "execute", "title": "Shell", "rawInput": raw });
            assert_eq!(check_body(&call)["tool_input"]["command"], command, "{raw}");
            assert_eq!(check_body(&call)["tool_name"], "Bash", "{raw}");
        }
        let schema_shape = json!({
            "toolCallId": "c", "kind": "execute", "title": "developer__shell",
            "rawInput": { "command": "git push" },
        });
        assert_eq!(check_body(&schema_shape), bash("git push"));
    }

    #[test]
    fn a_shell_call_without_a_readable_command_never_goes_out_as_bash() {
        for call in [
            json!({ "kind": "execute", "title": "Run it", "rawInput": { "script": 3 } }),
            json!({ "kind": "execute", "rawInput": { "command": ["git", 1] } }),
            json!({ "kind": "execute", "rawInput": { "command": "  " } }),
            json!({ "kind": "execute" }),
            json!({ "kind": "other", "name": "Bash", "rawInput": { "description": "x" } }),
            json!({ "title": "bash" }),
        ] {
            let body = check_body(&call);
            assert_ne!(body["tool_name"], "Bash", "{call} -> {body}");
            assert!(body["tool_input"].get("command").is_none(), "{call} -> {body}");
        }
        let named = json!({ "kind": "execute", "_meta": { "toolName": "run_shell_command" } });
        assert_eq!(check_body(&named)["tool_name"], "run_shell_command");
        assert_eq!(check_body(&json!({ "kind": "execute" }))["tool_name"], "Shell");
    }

    #[test]
    fn a_shell_named_bash_with_a_command_is_checked_as_bash() {
        let call =
            json!({ "kind": "other", "name": "Bash", "rawInput": { "command": "git push" } });
        assert_eq!(check_body(&call), bash("git push"));
    }

    #[tokio::test]
    async fn a_guard_whose_client_cannot_be_built_rejects_everything() {
        let guard =
            Guard::with_client("http://127.0.0.1:9".to_owned(), Err("no TLS provider".to_owned()));
        for call in [json!({ "kind": "read", "rawInput": { "path": "." } }), Value::Null] {
            let Verdict::Reject(reason) = guard.check(&call).await else {
                panic!("a broken guard client must not allow {call}");
            };
            assert!(reason.contains("no TLS provider"), "{reason}");
            assert!(reason.contains("http://127.0.0.1:9"), "{reason}");
            assert!(reason.contains("failing closed"), "{reason}");
        }
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
