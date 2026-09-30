//! The device-authorization pair.
//!
//! These two endpoints are the crate's only calls that do not come from
//! `ROUTES`: they are unauthenticated, so they are mounted outside the auth
//! layer that the route table describes. Their paths still come from
//! `cctui_proto`, never from a literal here.

use cctui_proto::api::device_auth::{
    DeviceAuthPoll, DeviceAuthPollRequest, DeviceAuthStart, DeviceAuthStartRequest, POLL_PATH,
    START_PATH,
};
use reqwest::StatusCode;

use crate::error::ClientError;

/// Begin a device login. No credential: earning one is the point.
pub async fn start(
    http: &reqwest::Client,
    base_url: &str,
    client_name: Option<String>,
) -> Result<DeviceAuthStart, ClientError> {
    let route = "post_auth_device_start";
    let resp = http
        .post(format!("{}{START_PATH}", base_url.trim_end_matches('/')))
        .json(&DeviceAuthStartRequest { client_name })
        .send()
        .await
        .map_err(|source| ClientError::Transport { route, source })?;
    decode(route, resp).await
}

/// One poll. `Ok(None)` is the server asking the caller to wait — either the
/// request is still pending or the poll came in too fast.
pub async fn poll(
    http: &reqwest::Client,
    base_url: &str,
    device_code: &str,
) -> Result<Option<DeviceAuthPoll>, ClientError> {
    let route = "post_auth_device_poll";
    let body = DeviceAuthPollRequest { device_code: device_code.to_owned() };
    let resp = http
        .post(format!("{}{POLL_PATH}", base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|source| ClientError::Transport { route, source })?;
    if resp.status() == StatusCode::TOO_MANY_REQUESTS {
        return Ok(None);
    }
    decode(route, resp).await.map(Some)
}

async fn decode<R: serde::de::DeserializeOwned>(
    route: &'static str,
    resp: reqwest::Response,
) -> Result<R, ClientError> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ClientError::Status { route, status: status.as_u16(), body });
    }
    let bytes = resp.bytes().await.map_err(|source| ClientError::Transport { route, source })?;
    serde_json::from_slice(&bytes).map_err(|source| ClientError::Decode { route, source })
}

#[cfg(test)]
mod tests {
    use super::{POLL_PATH, START_PATH};

    #[test]
    fn the_paths_are_the_ones_the_server_mounts_outside_the_auth_layer() {
        assert_eq!(START_PATH, "/api/v1/auth/device/start");
        assert_eq!(POLL_PATH, "/api/v1/auth/device/poll");
    }
}
