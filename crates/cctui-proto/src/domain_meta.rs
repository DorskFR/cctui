//! The static domain tables a client needs to render without encoding any of
//! the rules itself: provider metadata, quota probes, end-reason tones and the
//! permission-mode order. Labels that are translated stay client-side.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::adapter::PermissionMode;
use crate::provider::{ProviderInfo, UsageProbeInfo, provider_kinds};
use crate::session_end::{EndReasonInfo, end_reason_table};

/// Body of `GET /api/v1/meta/domain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DomainMeta {
    pub providers: Vec<ProviderInfo>,
    pub usage_probes: Vec<UsageProbeInfo>,
    pub end_reasons: Vec<EndReasonInfo>,
    pub permission_modes: Vec<PermissionMode>,
}

impl DomainMeta {
    /// `usage_probes` comes from the server's probe registry, which is the only
    /// part this crate cannot enumerate.
    #[must_use]
    pub fn new(usage_probes: Vec<UsageProbeInfo>) -> Self {
        Self {
            providers: provider_kinds(),
            usage_probes,
            end_reasons: end_reason_table(),
            permission_modes: PermissionMode::ALL.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SessionEndReason;

    #[test]
    fn every_end_reason_and_permission_mode_is_listed_in_picker_order() {
        let meta = DomainMeta::new(vec![]);
        assert_eq!(meta.end_reasons.len(), 9);
        assert_eq!(meta.end_reasons[0].reason, SessionEndReason::Completed);
        assert_eq!(
            meta.permission_modes,
            vec![
                PermissionMode::Ask,
                PermissionMode::Auto,
                PermissionMode::Yolo,
                PermissionMode::Whip
            ]
        );
    }
}
