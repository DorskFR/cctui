//! `/api/v1/sessions/{id}/guard` — the mirrored workflow-guard step.

/// Body of `POST /sessions/{id}/guard/transition`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GuardTransitionRequest {
    /// A step number or `exit`.
    pub step: String,
}
