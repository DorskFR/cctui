//! Worker env names a dispatch payload may not set: the dispatcher's own
//! contract vars and anything that changes how the worker's shell, loader or
//! network stack behaves.

const RESERVED_EXACT: [&str; 5] = ["SESSION_ID", "REPLY_URL", "BASH_ENV", "ENV", "PATH"];
const RESERVED_PREFIXES: [&str; 4] = ["CCTUI_", "TASK_", "LD_", "DYLD_"];

#[must_use]
pub fn is_reserved_env_key(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    RESERVED_EXACT.contains(&upper.as_str())
        || RESERVED_PREFIXES.iter().any(|p| upper.starts_with(p))
        || upper.ends_with("_PROXY")
}

const PLUGIN_DENIED_EXACT: [&str; 6] =
    ["HOME", "SHELL", "USER", "NODE_OPTIONS", "TMPDIR", "XDG_CONFIG_HOME"];
const PLUGIN_DENIED_PREFIXES: [&str; 5] =
    ["ANTHROPIC_", "CLAUDE_", "OPENAI_", "FIREWORKS_", "CODEX_"];

/// Whether a plugin may declare `name` as the env var carrying one of its
/// user settings: `^[A-Z][A-Z0-9_]{0,63}$`, and none of the names that steer
/// the shell, the loader, the harness or cctui itself.
#[must_use]
pub fn valid_plugin_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let shape = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    shape
        && !is_reserved_env_key(name)
        && !PLUGIN_DENIED_EXACT.contains(&name)
        && !PLUGIN_DENIED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Payload field listing the `env` keys the server minted itself (gateway
/// routing, account env). The server strips any caller-supplied copy.
pub const SERVER_ENV_KEYS_FIELD: &str = "server_env_keys";

const DISPATCHER_OWNED_EXACT: [&str; 4] =
    ["CCTUI_URL", "CCTUI_MACHINE_KEY", "SESSION_ID", "REPLY_URL"];

fn is_dispatcher_owned(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    DISPATCHER_OWNED_EXACT.contains(&upper.as_str()) || upper.starts_with("TASK_")
}

fn first_reserved(payload: &serde_json::Value, exempt: &[&str]) -> Result<(), String> {
    let Some(env) = payload.get("env").and_then(serde_json::Value::as_object) else {
        return Ok(());
    };
    env.keys()
        .find(|k| {
            is_reserved_env_key(k) && (is_dispatcher_owned(k) || !exempt.contains(&k.as_str()))
        })
        .map_or(Ok(()), |k| {
            Err(format!("payload env `{k}` is reserved by the dispatcher and cannot be set"))
        })
}

/// Reject a caller payload whose `env` sets a reserved name.
///
/// # Errors
/// Names the first reserved key found.
pub fn check_caller_payload_env(payload: &serde_json::Value) -> Result<(), String> {
    first_reserved(payload, &[])
}

/// Reject a server-forwarded payload whose `env` sets a reserved name, except
/// server-minted keys named in [`SERVER_ENV_KEYS_FIELD`]. Dispatcher contract
/// vars are never exempt.
///
/// # Errors
/// Names the first reserved key found.
pub fn check_payload_env(payload: &serde_json::Value) -> Result<(), String> {
    let exempt: Vec<&str> = payload
        .get(SERVER_ENV_KEYS_FIELD)
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    first_reserved(payload, &exempt)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reserved_names_are_rejected_case_insensitively() {
        for k in [
            "CCTUI_URL",
            "CCTUI_MACHINE_KEY",
            "SESSION_ID",
            "TASK_PAYLOAD_JSON",
            "REPLY_URL",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "BASH_ENV",
            "ENV",
            "PATH",
            "HTTPS_PROXY",
            "no_proxy",
            "cctui_url",
        ] {
            assert!(is_reserved_env_key(k), "{k} must be reserved");
        }
        for k in ["FEATURE_FLAG", "ANTHROPIC_BASE_URL", "OPENAI_API_KEY", "PATHS", "MY_ENV"] {
            assert!(!is_reserved_env_key(k), "{k} must be allowed");
        }
    }

    #[test]
    fn check_payload_env_names_the_offending_key() {
        let err = check_payload_env(&json!({ "env": { "OK": "1", "CCTUI_URL": "https://x" } }))
            .unwrap_err();
        assert!(err.contains("CCTUI_URL"), "{err}");
        assert!(check_payload_env(&json!({ "env": { "OK": "1" } })).is_ok());
        assert!(check_payload_env(&json!({})).is_ok());
    }

    #[test]
    fn server_minted_keys_are_exempt_but_contract_vars_are_not() {
        let minted = json!({
            "env": { "CCTUI_CODEX_CONFIG_TOML": "x", "PATH": "/bin" },
            SERVER_ENV_KEYS_FIELD: ["CCTUI_CODEX_CONFIG_TOML", "PATH"],
        });
        assert!(check_payload_env(&minted).is_ok());
        assert!(check_caller_payload_env(&minted).is_err());

        let unlisted = json!({
            "env": { "CCTUI_CODEX_CONFIG_TOML": "x", "LD_PRELOAD": "/x.so" },
            SERVER_ENV_KEYS_FIELD: ["CCTUI_CODEX_CONFIG_TOML"],
        });
        assert!(check_payload_env(&unlisted).unwrap_err().contains("LD_PRELOAD"));

        for k in ["CCTUI_URL", "CCTUI_MACHINE_KEY", "SESSION_ID", "REPLY_URL", "TASK_ID"] {
            let p = json!({ "env": { k: "x" }, SERVER_ENV_KEYS_FIELD: [k] });
            assert!(check_payload_env(&p).unwrap_err().contains(k), "{k}");
        }
    }

    #[test]
    fn plugin_env_names_are_shaped_and_not_reserved() {
        for ok in ["YUBI_HOST", "X", "A1_B2", &format!("A{}", "B".repeat(63))] {
            assert!(super::valid_plugin_env_name(ok), "{ok}");
        }
        for bad in [
            "",
            "yubi_host",
            "1ABC",
            "A-B",
            "A B",
            &format!("A{}", "B".repeat(64)),
            "PATH",
            "HOME",
            "SHELL",
            "USER",
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "DYLD_X",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_X",
            "CCTUI_WEB_ORIGIN",
            "OPENAI_API_KEY",
            "FIREWORKS_API_KEY",
            "HTTP_PROXY",
            "REPLY_URL",
            "SESSION_ID",
            "BASH_ENV",
            "ENV",
        ] {
            assert!(!super::valid_plugin_env_name(bad), "{bad}");
        }
    }
}
