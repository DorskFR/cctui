//! `/api/v1/auth/device` — device-authorization login for headless clients.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// Path of the unauthenticated device-login start endpoint.
///
/// It cannot live in [`crate::api::routes::ROUTES`] — that table is for routes
/// behind the auth layer — so the clients read it from here instead of
/// spelling it.
pub const START_PATH: &str = "/api/v1/auth/device/start";
pub const POLL_PATH: &str = "/api/v1/auth/device/poll";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DeviceAuthStartRequest {
    /// What to show the approving user, e.g. `cctui on thinkpad`.
    pub client_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DeviceAuthStart {
    /// The polling secret. Never displayed; the user types `user_code` instead.
    pub device_code: String,
    /// The short code the user reads out to the browser, `XXXX-XXXX`.
    pub user_code: String,
    pub verification_uri: String,
    /// `verification_uri` with the code pre-filled, for a clickable terminal.
    pub verification_uri_complete: String,
    pub expires_in_secs: u32,
    /// Minimum seconds between polls. Polling faster earns a 429.
    pub interval_secs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DeviceAuthStatus {
    Pending,
    Approved,
    Denied,
    /// Timed out, or already claimed: either way the code is dead.
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DeviceAuthPollRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DeviceAuthPoll {
    pub status: DeviceAuthStatus,
    /// The minted key, present exactly once: on the first poll after approval.
    pub token: Option<String>,
}

/// What the approval page shows before the user decides.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DeviceAuthRequestInfo {
    pub user_code: String,
    pub client_name: Option<String>,
    pub expires_in_secs: u32,
    pub status: DeviceAuthStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DeviceAuthDecision {
    pub approve: bool,
}
