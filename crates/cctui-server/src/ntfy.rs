//! Best-effort ntfy push notifications (CCT-198).
//!
//! Active only when `CCTUI_NTFY_TOKEN` is set in the env (provisioned from
//! vault). When unset every call here is a no-op, so the feature stays off in
//! environments that don't configure it. Notifications are fire-and-forget:
//! we spawn the POST on a detached task and never block (or fail) the request
//! path on a flaky ntfy server.
//!
//! Target topic is `CCTUI_NTFY_URL` (a full topic URL, e.g.
//! `https://ntfy.example.internal/cctui-dispatch`); the token is sent as a bearer.

use crate::config::Config;

/// A single notification to push. `tags` map to ntfy emoji/labels and
/// `priority` is the ntfy 1..=5 scale (3 = default).
pub struct Notification {
    pub title: String,
    pub message: String,
    pub tags: String,
    pub priority: u8,
}

/// Fire-and-forget: push `n` to ntfy if a token is configured, otherwise a
/// no-op. Never blocks the caller and never propagates errors — at most it
/// logs a warning if the POST fails.
pub fn notify(config: &Config, n: Notification) {
    let Some(token) = config.ntfy_token.clone() else {
        return;
    };
    let url = config.ntfy_url.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .bearer_auth(token)
            .header("Title", &n.title)
            .header("Tags", &n.tags)
            .header("Priority", n.priority.to_string())
            .body(n.message)
            .send()
            .await;
        match resp {
            Ok(r) if !r.status().is_success() => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!(%status, %body, "ntfy push returned non-success");
            }
            Err(e) => tracing::warn!("ntfy push failed: {e}"),
            _ => {}
        }
    });
}
