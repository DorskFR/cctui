//! Kube dispatch identity: register a Job, redeem the pod's service-account
//! token for its machine key, fetch the payload env.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const AUDIENCE: &str = "cctui";

/// What the worker holds instead of the machine key; the guard-proxy swaps it.
pub const PLACEHOLDER_MACHINE_KEY: &str = "cctui-dispatch-placeholder-key";

pub const REGISTER_PATH: &str = "/api/v1/dispatch/register";
pub const REDEEM_PATH: &str = "/api/v1/dispatch/redeem";

#[must_use]
pub fn env_path(dispatch_id: Uuid) -> String {
    format!("/api/v1/dispatch/{dispatch_id}/env")
}

/// Keeps a queued dispatch's registration from expiring.
#[must_use]
pub fn hold_path(dispatch_id: Uuid) -> String {
    format!("/api/v1/dispatch/{dispatch_id}/hold")
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterRequest {
    pub dispatch_id: Uuid,
    pub session_id: String,
    pub namespace: String,
    pub job_name: String,
    pub machine_key: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedeemRequest {
    pub dispatch_id: Uuid,
    pub token: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedeemResponse {
    pub machine_key: String,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvResponse {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_path_names_the_dispatch() {
        let id = Uuid::nil();
        assert_eq!(env_path(id), "/api/v1/dispatch/00000000-0000-0000-0000-000000000000/env");
    }

    #[test]
    fn hold_path_names_the_dispatch() {
        let id = Uuid::nil();
        assert_eq!(hold_path(id), "/api/v1/dispatch/00000000-0000-0000-0000-000000000000/hold");
    }

    #[test]
    fn register_env_defaults_to_empty() {
        let r: RegisterRequest = serde_json::from_value(serde_json::json!({
            "dispatch_id": Uuid::nil(),
            "session_id": "s",
            "namespace": "ns",
            "job_name": "j",
            "machine_key": "k"
        }))
        .unwrap();
        assert!(r.env.is_empty());
    }
}
