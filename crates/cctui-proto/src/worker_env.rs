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

/// Reject a payload whose `env` sets a reserved name.
///
/// # Errors
/// Names the first reserved key found.
pub fn check_payload_env(payload: &serde_json::Value) -> Result<(), String> {
    let Some(env) = payload.get("env").and_then(serde_json::Value::as_object) else {
        return Ok(());
    };
    match env.keys().find(|k| is_reserved_env_key(k)) {
        Some(k) => Err(format!("payload env `{k}` is reserved by the dispatcher and cannot be set")),
        None => Ok(()),
    }
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
}
