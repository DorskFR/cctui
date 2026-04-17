//! `cctui-admin` — CLI for user/machine provisioning.
//!
//! Two auth modes:
//!   - Admin ops (`user create/list/revoke/rotate`, `machine list/revoke/rotate`)
//!     use `--token` / `CCTUI_ADMIN_TOKEN`.
//!   - `enroll` uses a user token (`--token` / `CCTUI_USER_TOKEN` or
//!     `~/.config/cctui/user.json`) and writes a new `machine.json`.

use anyhow::{Context, Result, bail};
use cctui_proto::identity::{MachineIdentity, UserIdentity, load_user, save_machine, save_user};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "cctui-admin", about = "Provision users and machines for cctui-server", version)]
struct Cli {
    /// Server URL (default: <http://localhost:8700>, or `CCTUI_URL` env).
    #[arg(long, global = true, env = "CCTUI_URL", default_value = "http://localhost:8700")]
    server: String,

    /// Bearer token. For admin ops, an admin token (`CCTUI_ADMIN_TOKEN`).
    /// For `enroll`, a user token (`CCTUI_USER_TOKEN`); if unset, read from
    /// `~/.config/cctui/user.json`.
    #[arg(long, global = true, env = "CCTUI_ADMIN_TOKEN")]
    token: Option<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// User management (admin token required).
    #[command(subcommand)]
    User(UserCmd),
    /// Machine management (admin token required).
    #[command(subcommand)]
    Machine(MachineCmd),
    /// Enroll *this* host: mints a machine key using a user token and
    /// writes `~/.config/cctui/machine.json`.
    Enroll {
        /// Hostname to register (defaults to system hostname).
        #[arg(long)]
        hostname: Option<String>,
        /// User token. Falls back to `CCTUI_USER_TOKEN`, then user.json.
        #[arg(long, env = "CCTUI_USER_TOKEN")]
        user_token: Option<String>,
    },
}

#[derive(Subcommand)]
enum UserCmd {
    /// Create a new user and print (and optionally save) the key.
    Create {
        name: String,
        /// Also write the key to `~/.config/cctui/user.json`.
        #[arg(long)]
        save: bool,
    },
    List,
    Revoke {
        id: Uuid,
    },
    Rotate {
        id: Uuid,
    },
    Machines {
        id: Uuid,
    },
}

#[derive(Subcommand)]
enum MachineCmd {
    Revoke { id: Uuid },
    Rotate { id: Uuid },
}

#[derive(Deserialize, Serialize)]
struct CreateUserResponse {
    id: Uuid,
    name: String,
    key: String,
}

#[derive(Deserialize, Serialize)]
struct UserRow {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize)]
struct MachineRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize)]
struct RotateResponse {
    id: Uuid,
    key: String,
}

#[derive(Deserialize)]
struct EnrollResponse {
    machine_id: Uuid,
    machine_key: String,
    #[allow(dead_code)]
    server_version: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::builder().build()?;

    match cli.cmd {
        Command::User(cmd) => user_cmd(&client, &cli.server, cli.token.as_deref(), cmd).await,
        Command::Machine(cmd) => machine_cmd(&client, &cli.server, cli.token.as_deref(), cmd).await,
        Command::Enroll { hostname, user_token } => {
            enroll_cmd(&client, &cli.server, user_token, hostname).await
        }
    }
}

fn require_token(token: Option<&str>) -> Result<&str> {
    token
        .filter(|t| !t.is_empty())
        .context("admin token required (pass --token or set CCTUI_ADMIN_TOKEN)")
}

async fn user_cmd(client: &Client, server: &str, token: Option<&str>, cmd: UserCmd) -> Result<()> {
    let token = require_token(token)?;
    match cmd {
        UserCmd::Create { name, save } => {
            let url = format!("{server}/api/v1/admin/users");
            let res: CreateUserResponse = post_json(client, &url, token, json!({"name": name}))
                .await
                .context("create user")?;
            println!("id:   {}", res.id);
            println!("name: {}", res.name);
            println!("key:  {}", res.key);
            if save {
                let id = UserIdentity {
                    server_url: server.to_string(),
                    user_key: res.key.clone(),
                    user_id: Some(res.id.to_string()),
                    name: Some(res.name),
                };
                let path = save_user(&id)?;
                println!("saved: {}", path.display());
            } else {
                eprintln!("\n⚠  key shown once — store it now (or rerun with --save).");
            }
        }
        UserCmd::List => {
            let url = format!("{server}/api/v1/admin/users");
            let rows: Vec<UserRow> = get_json(client, &url, token).await?;
            print_users(&rows);
        }
        UserCmd::Revoke { id } => {
            let url = format!("{server}/api/v1/admin/users/{id}");
            delete(client, &url, token).await?;
            println!("revoked user {id}");
        }
        UserCmd::Rotate { id } => {
            let url = format!("{server}/api/v1/admin/users/{id}/rotate");
            let res: RotateResponse = post_json(client, &url, token, json!({})).await?;
            println!("id:  {}", res.id);
            println!("key: {}", res.key);
        }
        UserCmd::Machines { id } => {
            let url = format!("{server}/api/v1/admin/users/{id}/machines");
            let rows: Vec<MachineRow> = get_json(client, &url, token).await?;
            print_machines(&rows);
        }
    }
    Ok(())
}

async fn machine_cmd(
    client: &Client,
    server: &str,
    token: Option<&str>,
    cmd: MachineCmd,
) -> Result<()> {
    let token = require_token(token)?;
    match cmd {
        MachineCmd::Revoke { id } => {
            let url = format!("{server}/api/v1/admin/machines/{id}");
            delete(client, &url, token).await?;
            println!("revoked machine {id}");
        }
        MachineCmd::Rotate { id } => {
            let url = format!("{server}/api/v1/admin/machines/{id}/rotate");
            let res: RotateResponse = post_json(client, &url, token, json!({})).await?;
            println!("id:  {}", res.id);
            println!("key: {}", res.key);
        }
    }
    Ok(())
}

async fn enroll_cmd(
    client: &Client,
    server: &str,
    user_token: Option<String>,
    hostname: Option<String>,
) -> Result<()> {
    let (server_url, token) = resolve_user_auth(server, user_token)?;
    let hostname = hostname.unwrap_or_else(system_hostname);

    let url = format!("{server_url}/api/v1/enroll");
    let res: EnrollResponse = post_json(
        client,
        &url,
        &token,
        json!({"hostname": hostname, "os": std::env::consts::OS, "arch": std::env::consts::ARCH}),
    )
    .await
    .context("enroll")?;

    let id = MachineIdentity {
        server_url: server_url.clone(),
        machine_key: res.machine_key.clone(),
        machine_id: Some(res.machine_id.to_string()),
        hostname: Some(hostname.clone()),
    };
    let path = save_machine(&id)?;
    println!("machine_id: {}", res.machine_id);
    println!("hostname:   {hostname}");
    println!("saved:      {}", path.display());
    Ok(())
}

/// Resolve (`server_url`, `user_token`) for enrol. Precedence:
///  1. CLI flag / `CCTUI_USER_TOKEN`
///  2. user.json (takes its `server_url` too)
fn resolve_user_auth(server_flag: &str, token: Option<String>) -> Result<(String, String)> {
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        return Ok((server_flag.to_string(), t));
    }
    if let Some(u) = load_user() {
        return Ok((u.server_url, u.user_key));
    }
    bail!(
        "user token required — pass --user-token, set CCTUI_USER_TOKEN, or create \
         ~/.config/cctui/user.json via `cctui-admin user create --save`"
    )
}

fn system_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

async fn post_json<T: for<'de> Deserialize<'de>>(
    client: &Client,
    url: &str,
    token: &str,
    body: serde_json::Value,
) -> Result<T> {
    let resp = client.post(url).bearer_auth(token).json(&body).send().await?;
    decode(resp).await
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    client: &Client,
    url: &str,
    token: &str,
) -> Result<T> {
    let resp = client.get(url).bearer_auth(token).send().await?;
    decode(resp).await
}

async fn delete(client: &Client, url: &str, token: &str) -> Result<()> {
    let resp = client.delete(url).bearer_auth(token).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("{status}: {body}");
    }
    Ok(())
}

async fn decode<T: for<'de> Deserialize<'de>>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            bail!("401 unauthorized — token rejected by server");
        }
        bail!("{status}: {body}");
    }
    resp.json::<T>().await.context("decode response")
}

fn print_users(rows: &[UserRow]) {
    println!("{:<38}  {:<24}  {:<20}  status", "id", "name", "created_at");
    for r in rows {
        let status = r.revoked_at.map_or("active", |_| "REVOKED");
        println!(
            "{:<38}  {:<24}  {:<20}  {}",
            r.id,
            truncate(&r.name, 24),
            r.created_at.format("%Y-%m-%d %H:%M:%S"),
            status
        );
    }
}

fn print_machines(rows: &[MachineRow]) {
    println!("{:<38}  {:<24}  {:<20}  {:<20}  status", "id", "hostname", "first_seen", "last_seen");
    for r in rows {
        let status = r.revoked_at.map_or("active", |_| "REVOKED");
        println!(
            "{:<38}  {:<24}  {:<20}  {:<20}  {}",
            r.id,
            truncate(&r.name, 24),
            r.first_seen_at.format("%Y-%m-%d %H:%M:%S"),
            r.last_seen_at.format("%Y-%m-%d %H:%M:%S"),
            status
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}…", &s[..max - 1]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses() {
        Cli::command().debug_assert();
    }

    #[test]
    fn require_token_errors_when_empty() {
        assert!(require_token(None).is_err());
        assert!(require_token(Some("")).is_err());
        assert!(require_token(Some("tok")).is_ok());
    }
}
