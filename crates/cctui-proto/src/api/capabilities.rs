//! `GET /api/v1/capabilities` — which optional integrations this server has.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// The capability envelope. One field per optional integration.
///
/// Self-hosted models are a per-account property surfaced by `GET /accounts`,
/// not a server-global list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CapabilitiesResponse {
    pub langfuse: LangfuseCapability,
}

/// The Langfuse read integration's capability, as seen by the webui.
/// `available` gates every Langfuse UI element; `host` + `project_id` build the
/// `<host>/project/<id>/sessions/<uuid>` deep link. All `None` when the sink is
/// unconfigured; `project_id` alone `None` when the id could not be resolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct LangfuseCapability {
    pub available: bool,
    pub host: Option<String>,
    pub public_host: Option<String>,
    pub project_id: Option<String>,
}
