//! `cctui login` / `cctui logout`.

use anyhow::{Context, Result, bail};
use cctui_proto::api::device_auth::{
    self, DeviceAuthPoll, DeviceAuthPollRequest, DeviceAuthStart, DeviceAuthStartRequest,
    DeviceAuthStatus,
};
use cctui_proto::api::me::MeResponse;
use cctui_proto::identity::{self, UserIdentity};

use crate::client::{ApiError, ServerClient};

pub const DEFAULT_SERVER_URL: &str = "http://localhost:8700";

/// Where to log in: the flag, else the server already in `user.json`, else
/// `CCTUI_URL`, else the local default. Trailing slashes are dropped so the
/// stored url concatenates cleanly with an api path.
#[must_use]
pub fn resolve_server_url(flag: Option<&str>, stored: Option<&str>, env: Option<&str>) -> String {
    let raw = flag
        .or(stored)
        .or(env)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SERVER_URL);
    raw.trim_end_matches('/').to_owned()
}

#[must_use]
pub fn identity_for(server_url: &str, key: &str, me: &MeResponse) -> UserIdentity {
    UserIdentity {
        server_url: server_url.to_owned(),
        user_key: key.to_owned(),
        user_id: me.user_id.map(|id| id.to_string()),
        name: me.user_name.clone(),
    }
}

fn stored_server_url() -> Option<String> {
    identity::load_user().map(|i| i.server_url)
}

/// Validate `key` against `server_url` via `GET /me`. No credential is written
/// before the server has agreed it is one.
pub async fn validate(server_url: &str, key: &str) -> Result<MeResponse> {
    match ServerClient::new(server_url, key).me().await {
        Ok(me) => Ok(me),
        Err(ApiError::Unauthorized) => bail!("key rejected by {server_url}"),
        Err(e) => bail!("could not reach {server_url}: {e}"),
    }
}

/// Persist a validated key and report who it belongs to.
pub fn persist(server_url: &str, key: &str, me: &MeResponse) -> Result<()> {
    let path = identity::save_user(&identity_for(server_url, key, me))
        .context("write ~/.config/cctui/user.json")?;
    println!(
        "logged in as {} ({}) — {}",
        me.user_name.as_deref().unwrap_or("?"),
        me.role,
        path.display()
    );
    Ok(())
}

/// `cctui login`: the device-authorization flow, unless `--key` asked for the
/// paste-a-key fallback.
pub async fn login(server: Option<String>, key: Option<String>) -> Result<()> {
    match key {
        Some(key) => login_with_key(server, Some(key).filter(|k| !k.is_empty())).await,
        None => device_login(server).await,
    }
}

/// `cctui login`: print a short code, wait for a browser user to approve it,
/// then validate and store the key the server mints.
pub async fn device_login(server: Option<String>) -> Result<()> {
    let server_url = resolve_server_url(
        server.as_deref(),
        stored_server_url().as_deref(),
        std::env::var("CCTUI_URL").ok().as_deref(),
    );
    let http = reqwest::Client::new();
    let client_name = format!("cctui on {}", cctui_proto::util::hostname());
    let start: DeviceAuthStart = http
        .post(format!("{server_url}{}", device_auth::START_PATH))
        .json(&DeviceAuthStartRequest { client_name: Some(client_name) })
        .send()
        .await
        .with_context(|| format!("POST {server_url}{}", device_auth::START_PATH))?
        .error_for_status()
        .context("device login could not be started")?
        .json()
        .await
        .context("deserialize device login")?;

    println!("open {}", start.verification_uri_complete);
    println!("and confirm the code: {}", start.user_code);
    println!("waiting for approval…");

    let interval = std::time::Duration::from_secs(u64::from(start.interval_secs.max(1)));
    let ttl = std::time::Duration::from_secs(u64::from(start.expires_in_secs));
    let deadline = std::time::Instant::now() + ttl;
    let token = loop {
        tokio::time::sleep(interval).await;
        if std::time::Instant::now() >= deadline {
            bail!("the code expired before it was approved; run `cctui login` again");
        }
        if let Some(token) = poll_once(&http, &server_url, &start.device_code).await? {
            break token;
        }
    };

    let me = validate(&server_url, &token).await?;
    persist(&server_url, &token, &me)
}

/// One poll. `Ok(None)` means "still pending, keep waiting"; a refused code
/// ends the flow.
async fn poll_once(
    http: &reqwest::Client,
    server_url: &str,
    device_code: &str,
) -> Result<Option<String>> {
    let resp = http
        .post(format!("{server_url}{}", device_auth::POLL_PATH))
        .json(&DeviceAuthPollRequest { device_code: device_code.to_owned() })
        .send()
        .await
        .context("poll device login")?;
    // The server rate-limits polling; a client that hits it simply waits again.
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Ok(None);
    }
    let poll: DeviceAuthPoll =
        resp.error_for_status().context("device login poll failed")?.json().await?;
    match poll.status {
        DeviceAuthStatus::Pending => Ok(None),
        DeviceAuthStatus::Approved => poll
            .token
            .ok_or_else(|| anyhow::anyhow!("the server approved the login but minted no key"))
            .map(Some),
        DeviceAuthStatus::Denied => bail!("the login was denied"),
        DeviceAuthStatus::Expired => {
            bail!("the code expired before it was approved; run `cctui login` again")
        }
    }
}

/// `cctui login --key`: take the key from the flag or from stdin.
pub async fn login_with_key(server: Option<String>, key: Option<String>) -> Result<()> {
    let server_url = resolve_server_url(
        server.as_deref(),
        stored_server_url().as_deref(),
        std::env::var("CCTUI_URL").ok().as_deref(),
    );
    let key = match key {
        Some(k) => k,
        None => read_key_from_stdin()?,
    };
    let key = key.trim().to_owned();
    if key.is_empty() {
        bail!("no key given");
    }
    let me = validate(&server_url, &key).await?;
    persist(&server_url, &key, &me)
}

/// The key is read from stdin rather than taken as an argument by default: an
/// argument lands in the shell history and in `ps`.
fn read_key_from_stdin() -> Result<String> {
    use std::io::{BufRead, Write};
    print!("paste your cctui key: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).context("read key from stdin")?;
    Ok(line)
}

/// `cctui logout [--revoke]`. Deleting the local file is the part that always
/// happens; a failed revoke is reported but does not keep the credential on disk.
pub async fn logout(revoke: bool) -> Result<()> {
    let Some(id) = identity::load_user() else {
        println!("not logged in");
        return Ok(());
    };
    if revoke {
        match ServerClient::new(&id.server_url, &id.user_key).revoke_current_key().await {
            Ok(()) => println!("key revoked on {}", id.server_url),
            Err(ApiError::Unauthorized) => println!("key was already invalid on {}", id.server_url),
            Err(e) => eprintln!("could not revoke the key on {}: {e}", id.server_url),
        }
    }
    let path = identity::user_path().context("could not resolve config dir (HOME unset?)")?;
    match std::fs::remove_file(&path) {
        Ok(()) => println!("removed {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("not logged in"),
        Err(e) => return Err(e).context("remove user.json"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SERVER_URL, identity_for, resolve_server_url};
    use cctui_proto::api::me::MeResponse;

    fn me() -> MeResponse {
        MeResponse {
            role: "user".to_owned(),
            user_id: Some(uuid::Uuid::nil()),
            user_name: Some("dorsk".to_owned()),
            machine_id: None,
            scopes: vec!["read".to_owned()],
            token_preview: "cctui_u_ab12…ef34".to_owned(),
        }
    }

    #[test]
    fn the_flag_wins_then_the_stored_url_then_the_env() {
        assert_eq!(resolve_server_url(Some("https://a"), Some("https://b"), None), "https://a");
        assert_eq!(resolve_server_url(None, Some("https://b"), Some("https://c")), "https://b");
        assert_eq!(resolve_server_url(None, None, Some("https://c")), "https://c");
        assert_eq!(resolve_server_url(None, None, None), DEFAULT_SERVER_URL);
    }

    #[test]
    fn a_blank_url_falls_through_and_trailing_slashes_go() {
        assert_eq!(resolve_server_url(Some("  "), Some("https://b/"), None), "https://b");
    }

    #[test]
    fn the_identity_carries_the_validated_answer() {
        let id = identity_for("https://a", "cctui_u_k", &me());
        assert_eq!(id.server_url, "https://a");
        assert_eq!(id.user_key, "cctui_u_k");
        assert_eq!(id.name.as_deref(), Some("dorsk"));
        assert_eq!(id.user_id.as_deref(), Some(uuid::Uuid::nil().to_string().as_str()));
    }
}
