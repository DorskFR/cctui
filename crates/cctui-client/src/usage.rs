//! Deserialize views of the usage payloads.
//!
//! `cctui_proto::api` declares these Serialize-only (the server writes them), so
//! a client needs its own mirrors. Every numeric is optional: a credential whose
//! fetch failed reports a window without a reading rather than a zero, and the
//! panel has to tell those apart.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UsagePace {
    #[serde(default)]
    pub elapsed_fraction: f64,
    #[serde(default)]
    pub expected_pct: f64,
    #[serde(default)]
    pub ratio: f64,
    #[serde(default)]
    pub projected_wall_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub slope_hours: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct UsageWindowView {
    pub key: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub utilization: Option<f64>,
    #[serde(default)]
    pub amount_usd: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub model_display_name: Option<String>,
    #[serde(default)]
    pub pace: Option<UsagePace>,
}

/// One row of `GET /accounts/usage`. `account_id` is the provider-credential id;
/// `account` is the identity it hangs off.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AccountUsageEntry {
    pub account_id: Uuid,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub windows: Vec<UsageWindowView>,
    #[serde(default)]
    pub age_secs: u64,
    pub account: Uuid,
    #[serde(default)]
    pub account_name: String,
    #[serde(default)]
    pub account_emoji: Option<String>,
    #[serde(default)]
    pub header_pin: bool,
    #[serde(default)]
    pub provider_status: Option<ProviderStatusView>,
    /// The reset a claim would spend, so a confirm can name the credit instead
    /// of leaving the server to pick. `None` when the payload offers none.
    #[serde(default)]
    pub limit_reset: Option<LimitResetStatusView>,
}

/// The claimable reset of one credential, as much of it as a client needs to
/// name what a claim spends.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LimitResetStatusView {
    /// `codex` or `claude`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub credit_id: Option<String>,
    #[serde(default)]
    pub ineligible_reason: Option<String>,
    #[serde(default)]
    pub next_available_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderStatusView {
    #[serde(default)]
    pub indicator: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PoolUsageMember {
    pub account_id: Uuid,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub weight: f64,
    #[serde(default)]
    pub usage_known: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PoolProjection {
    #[serde(default)]
    pub wall_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub first_member_wall_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub demand_pct_per_hour: f64,
    #[serde(default)]
    pub slope_hours: f64,
    #[serde(default)]
    pub min_margin_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PoolUsageWindow {
    pub key: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub model_display_name: Option<String>,
    #[serde(default)]
    pub level_pct: Option<f64>,
    #[serde(default)]
    pub expected_pct: f64,
    #[serde(default)]
    pub ratio: Option<f64>,
    #[serde(default)]
    pub next_reset_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub projection: Option<PoolProjection>,
    #[serde(default)]
    pub projection_unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PoolFamilyUsage {
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub members: Vec<PoolUsageMember>,
    #[serde(default)]
    pub windows: Vec<PoolUsageWindow>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PoolUsageView {
    pub pool_id: Uuid,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub failover: bool,
    #[serde(default)]
    pub families: Vec<PoolFamilyUsage>,
}

#[cfg(test)]
mod tests {
    use super::{AccountUsageEntry, PoolUsageView};

    /// The server flattens `UsageWindow` into `UsageWindowView` and omits a
    /// `None` percent entirely, so the absent case must survive the round trip.
    #[test]
    fn a_window_without_a_reading_deserializes_as_unreported() {
        let rows: Vec<AccountUsageEntry> = serde_json::from_str(
            r#"[{"account_id":"11111111-1111-4111-8111-111111111111","provider":"anthropic",
                 "account":"22222222-2222-4222-8222-222222222222","account_name":"alice",
                 "header_pin":true,"age_secs":12,
                 "windows":[{"key":"session","kind":"session","label":"5h","utilization":91.2,
                             "resets_at":"2026-10-01T12:00:00Z",
                             "pace":{"elapsed_fraction":0.5,"expected_pct":50,"ratio":1.8}},
                            {"key":"weekly_all","kind":"weekly_all","label":"7d"},
                            {"key":"usd_5h","kind":"usd","label":"$","amount_usd":12.4}]}]"#,
        )
        .expect("parses");
        let windows = &rows[0].windows;
        assert_eq!(rows[0].account_name, "alice");
        assert_eq!(windows[0].utilization, Some(91.2));
        assert!(windows[0].resets_at.is_some());
        assert!((windows[0].pace.as_ref().expect("a pace").ratio - 1.8).abs() < f64::EPSILON);
        assert_eq!(windows[1].utilization, None, "an omitted percent is not zero");
        assert!(windows[1].pace.is_none());
        assert_eq!(windows[2].amount_usd, Some(12.4));
        assert!(rows[0].limit_reset.is_none(), "a payload offering no reset says so");
    }

    #[test]
    fn a_claimable_reset_survives_the_decode_so_a_confirm_can_name_it() {
        let rows: Vec<AccountUsageEntry> = serde_json::from_str(
            r#"[{"account_id":"11111111-1111-4111-8111-111111111111","provider":"openai",
                 "account":"22222222-2222-4222-8222-222222222222","account_name":"bob",
                 "windows":[],
                 "limit_reset":{"kind":"codex","available":true,
                                "title":"Full reset (Weekly + 5 hr)","credit_id":"cr_42"}}]"#,
        )
        .expect("parses");
        let reset = rows[0].limit_reset.as_ref().expect("a reset");
        assert_eq!(reset.kind, "codex");
        assert!(reset.available);
        assert_eq!(reset.title.as_deref(), Some("Full reset (Weekly + 5 hr)"));
        assert_eq!(reset.credit_id.as_deref(), Some("cr_42"));
        assert_eq!(reset.ineligible_reason, None);
    }

    #[test]
    fn a_pool_with_no_families_is_a_pool_not_an_error() {
        let pools: Vec<PoolUsageView> = serde_json::from_str(
            r#"[{"pool_id":"33333333-3333-4333-8333-333333333333","name":"default",
                 "strategy":"headroom","failover":true,"families":[]}]"#,
        )
        .expect("parses");
        assert_eq!(pools[0].name, "default");
        assert!(pools[0].families.is_empty());
    }
}
