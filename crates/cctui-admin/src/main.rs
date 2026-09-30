//! `cctui-admin` — CLI for user/machine provisioning.
//!
//! Two auth modes:
//!   - Admin ops (`user create/list/revoke/rotate`, `machine list/revoke/rotate`)
//!     use `--token` / `CCTUI_ADMIN_TOKEN`.
//!   - `enroll` uses a user token (`--token` / `CCTUI_USER_TOKEN` or
//!     `~/.config/cctui/user.json`) and writes a new `machine.json`.

use anyhow::{Context, Result, bail};
use cctui_proto::api::SkillIndexEntry;
use cctui_proto::identity::{
    MachineIdentity, UserIdentity, load_machine, load_user, save_machine, save_user,
};
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
    /// Print this instance's account usage windows (the numbers `/metrics`
    /// exports), for a status bar or a script.
    Usage {
        /// Emit the machine-readable envelope instead of a table. Field names
        /// are stable — third parties pin to them.
        #[arg(long)]
        json: bool,
    },
    /// Sync `~/.claude/skills/<name>/` bundles with the server.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// List skills registered for this user.
    List,
    /// Upload a local skill directory (`~/.claude/skills/<name>/` by default)
    /// to the server as a tar+zstd bundle.
    Push {
        /// Skill name (directory basename).
        name: String,
        /// Root containing `<name>/`. Defaults to `~/.claude/skills`.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
        /// Version label stored with the upload. Defaults server-side to the
        /// upload time in unix milliseconds.
        #[arg(long)]
        version: Option<String>,
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
    Revoke {
        id: Uuid,
    },
    Rotate {
        id: Uuid,
    },
    /// Set a friendly display name for the machine (overrides the hostname
    /// in the admin UI). Pass `--clear` to remove the override.
    Rename {
        id: Uuid,
        #[arg(required_unless_present = "clear")]
        name: Option<String>,
        #[arg(long)]
        clear: bool,
    },
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
    #[serde(default)]
    display_name: Option<String>,
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
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder().build()?;

    match cli.cmd {
        Command::User(cmd) => user_cmd(&client, &cli.server, cli.token.as_deref(), cmd).await,
        Command::Machine(cmd) => machine_cmd(&client, &cli.server, cli.token.as_deref(), cmd).await,
        Command::Enroll { hostname, user_token } => {
            enroll_cmd(&client, &cli.server, user_token, hostname).await
        }
        Command::Usage { json } => {
            usage_cmd(&client, &cli.server, cli.token.as_deref(), json).await
        }
        Command::Skills { cmd } => {
            skills_cmd(&client, &cli.server, cli.token.as_deref(), cmd).await
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
        MachineCmd::Rename { id, name, clear } => {
            let url = format!("{server}/api/v1/admin/machines/{id}");
            let display_name = if clear { None } else { name };
            patch_json(client, &url, token, json!({ "display_name": &display_name })).await?;
            match display_name {
                Some(n) => println!("renamed machine {id} → {n}"),
                None => println!("cleared display name for machine {id}"),
            }
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
    let hostname = hostname.unwrap_or_else(cctui_proto::util::hostname);

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

/// Resolve (`server_url`, `token`) for user read ops.
/// Precedence: CLI `--token` > machine.json > user.json.
fn resolve_read_auth(server_flag: &str, token: Option<&str>) -> Result<(String, String)> {
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        return Ok((server_flag.to_string(), t.to_string()));
    }
    if let Some(m) = load_machine() {
        return Ok((m.server_url, m.machine_key));
    }
    if let Some(u) = load_user() {
        return Ok((u.server_url, u.user_key));
    }
    bail!(
        "no credentials — enrol this host (`cctui-admin enroll`) or pass --token / \
         CCTUI_USER_TOKEN"
    )
}

/// One window as the accounts API reports it. Only the fields the export needs;
/// anything else the server adds is ignored.
#[derive(Debug, Deserialize)]
struct UsageWindowRow {
    key: String,
    label: String,
    #[serde(default)]
    utilization: f64,
    #[serde(default)]
    amount_usd: Option<f64>,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pace: Option<UsagePaceRow>,
}

#[derive(Debug, Deserialize)]
struct UsagePaceRow {
    #[serde(default)]
    ratio: f64,
    #[serde(default)]
    projected_wall_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct UsageEntryRow {
    account_id: Uuid,
    provider: String,
    account_name: String,
    #[serde(default)]
    age_secs: u64,
    #[serde(default)]
    windows: Vec<UsageWindowRow>,
}

/// `cctui-admin usage` — the same numbers `/metrics` exposes, in the shape a
/// status bar wants. The JSON field names are a contract: add, never rename.
async fn usage_cmd(
    client: &Client,
    server: &str,
    token: Option<&str>,
    as_json: bool,
) -> Result<()> {
    let (server, token) = resolve_read_auth(server, token)?;
    let url = format!("{server}/api/v1/accounts/usage");
    let rows: Vec<UsageEntryRow> = get_json(client, &url, &token).await?;
    let now = Utc::now();
    if as_json {
        println!("{}", serde_json::to_string_pretty(&usage_envelope(&rows, now))?);
        return Ok(());
    }
    for r in &rows {
        println!("{} · {}", r.account_name, r.provider);
        for w in &r.windows {
            let value = w
                .amount_usd
                .map_or_else(|| format!("{:.1}%", w.utilization), |usd| format!("${usd:.2}"));
            let reset = w.resets_at.map_or_else(String::new, |at| {
                format!("  resets in {}", human_secs(secs_until(at, now)))
            });
            println!("  {:<24} {value}{reset}", w.label);
        }
        if r.windows.is_empty() {
            println!("  (no usage data)");
        }
    }
    if rows.is_empty() {
        println!("no credentials configured");
    }
    Ok(())
}

/// Seconds until `at`, floored at zero — a window already past its reset is
/// `0`, never negative.
fn secs_until(at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    (at - now).num_seconds().max(0)
}

fn human_secs(secs: i64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

/// The stable JSON envelope. Pure so the field names are pinned by a test
/// rather than by a running server.
fn usage_envelope(rows: &[UsageEntryRow], now: DateTime<Utc>) -> serde_json::Value {
    json!({
        "accounts": rows.iter().map(|r| json!({
            "account": r.account_name,
            "provider": r.provider,
            "credential": r.account_id,
            "usage_known": !r.windows.is_empty(),
            "age_seconds": r.age_secs,
            "windows": r.windows.iter().map(|w| json!({
                "key": w.key,
                "label": w.label,
                "utilization_pct": w.utilization,
                "spend_usd": w.amount_usd,
                "resets_at": w.resets_at,
                "seconds_to_reset": w.resets_at.map(|at| secs_until(at, now)),
                "pace_ratio": w.pace.as_ref().map(|p| p.ratio),
                "projected_wall_at": w.pace.as_ref().and_then(|p| p.projected_wall_at),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

async fn skills_cmd(
    client: &Client,
    server: &str,
    token: Option<&str>,
    cmd: SkillsCmd,
) -> Result<()> {
    let (server_url, tok) = resolve_read_auth(server, token)?;
    match cmd {
        SkillsCmd::List => {
            let url = format!("{server_url}/api/v1/skills/index");
            let rows: Vec<SkillIndexEntry> = get_json(client, &url, &tok).await?;
            print_skills(&rows);
        }
        SkillsCmd::Push { name, root, version } => {
            skills_push(client, &server_url, &tok, &name, root, version.as_deref()).await?;
        }
    }
    Ok(())
}

async fn skills_push(
    client: &Client,
    server_url: &str,
    token: &str,
    name: &str,
    root: Option<std::path::PathBuf>,
    version: Option<&str>,
) -> Result<()> {
    validate_skill_name(name)?;
    let root = if let Some(p) = root {
        p
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        std::path::PathBuf::from(home).join(".claude").join("skills")
    };
    let skill_dir = root.join(name);
    if !skill_dir.is_dir() {
        bail!("{} is not a directory", skill_dir.display());
    }

    let tmp = tempfile_path(name);
    let status = std::process::Command::new("tar")
        .arg("--zstd")
        .arg("-C")
        .arg(&root)
        .arg("-cf")
        .arg(&tmp)
        .arg(name)
        .status()
        .with_context(|| "spawn tar (is tar+zstd available?)")?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("tar failed with status {status}");
    }

    let bytes = std::fs::read(&tmp).with_context(|| format!("read {}", tmp.display()))?;
    let _ = std::fs::remove_file(&tmp);
    let sha = cctui_proto::util::sha256_hex(&bytes);

    let url = format!("{server_url}/api/v1/skills/{name}");
    let mut req = client
        .put(&url)
        .bearer_auth(token)
        .header("X-CCTUI-SHA256", &sha)
        .header("Content-Type", "application/zstd");
    if let Some(v) = version {
        req = req.header("X-CCTUI-Version", v);
    }
    let resp = req.body(bytes.clone()).send().await?;
    let entry: SkillIndexEntry = decode(resp).await?;

    println!("name:       {}", entry.name);
    println!("version:    {}", entry.version);
    println!("sha256:     {}", entry.sha256);
    println!("size:       {} bytes", entry.size_bytes);
    println!("uploaded:   {}", entry.uploaded_at.format("%Y-%m-%d %H:%M:%S"));
    Ok(())
}

fn validate_skill_name(s: &str) -> Result<()> {
    if cctui_proto::util::is_valid_skill_name(s) {
        Ok(())
    } else {
        bail!("invalid skill name: {s}")
    }
}

fn tempfile_path(name: &str) -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("cctui-skill-{name}-{pid}-{nanos}.tar.zst"))
}

fn print_skills(rows: &[SkillIndexEntry]) {
    println!("{:<32}  {:<16}  {:<64}  {:>10}  uploaded", "name", "version", "sha256", "bytes");
    for r in rows {
        println!(
            "{:<32}  {:<16}  {:<64}  {:>10}  {}",
            truncate(&r.name, 32),
            truncate(&r.version, 16),
            r.sha256,
            r.size_bytes,
            r.uploaded_at.format("%Y-%m-%d %H:%M:%S"),
        );
    }
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

async fn patch_json(
    client: &Client,
    url: &str,
    token: &str,
    body: serde_json::Value,
) -> Result<()> {
    let resp = client.patch(url).bearer_auth(token).json(&body).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("{status}: {body}");
    }
    Ok(())
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
    println!(
        "{:<38}  {:<24}  {:<24}  {:<20}  {:<20}  status",
        "id", "hostname", "display_name", "first_seen", "last_seen"
    );
    for r in rows {
        let status = r.revoked_at.map_or("active", |_| "REVOKED");
        println!(
            "{:<38}  {:<24}  {:<24}  {:<20}  {:<20}  {}",
            r.id,
            truncate(&r.name, 24),
            truncate(r.display_name.as_deref().unwrap_or("—"), 24),
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

    fn usage_rows() -> Vec<UsageEntryRow> {
        serde_json::from_value(json!([{
            "account_id": "00000000-0000-0000-0000-000000000001",
            "provider": "anthropic",
            "account_name": "prod",
            "age_secs": 12,
            "windows": [
                {
                    "key": "session",
                    "kind": "session",
                    "label": "5h",
                    "utilization": 42.5,
                    "resets_at": "2026-09-30T14:00:00Z",
                    "pace": {"elapsed_fraction": 0.5, "expected_pct": 50.0, "ratio": 0.85,
                             "projected_wall_at": "2026-09-30T18:00:00Z"}
                },
                {"key": "usd_7d", "kind": "usd", "label": "7d spend", "utilization": 0.0, "amount_usd": 3.25}
            ]
        }]))
        .unwrap()
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_usage_envelope_field_names_are_the_documented_ones() {
        let env = usage_envelope(&usage_rows(), now());
        let a = &env["accounts"][0];
        assert_eq!(a["account"], "prod");
        assert_eq!(a["provider"], "anthropic");
        assert_eq!(a["credential"], "00000000-0000-0000-0000-000000000001");
        assert_eq!(a["usage_known"], true);
        assert_eq!(a["age_seconds"], 12);
        let session = &a["windows"][0];
        assert_eq!(session["key"], "session");
        assert_eq!(session["label"], "5h");
        assert_eq!(session["utilization_pct"], 42.5);
        assert_eq!(session["spend_usd"], serde_json::Value::Null);
        assert_eq!(session["seconds_to_reset"], 7200);
        assert_eq!(session["pace_ratio"], 0.85);
        assert_eq!(session["projected_wall_at"], "2026-09-30T18:00:00Z");
        let spend = &a["windows"][1];
        assert_eq!(spend["spend_usd"], 3.25);
        assert_eq!(spend["seconds_to_reset"], serde_json::Value::Null);
        assert_eq!(spend["pace_ratio"], serde_json::Value::Null);
    }

    #[test]
    fn an_instance_with_no_credentials_still_emits_the_envelope() {
        assert_eq!(usage_envelope(&[], now()), json!({"accounts": []}));
    }

    #[test]
    fn a_window_past_its_reset_reports_zero_not_a_negative_countdown() {
        let past =
            DateTime::parse_from_rfc3339("2026-09-30T11:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(secs_until(past, now()), 0);
        assert_eq!(human_secs(0), "0m");
        assert_eq!(human_secs(7200), "2h 0m");
        assert_eq!(human_secs(300), "5m");
    }

    #[test]
    fn require_token_errors_when_empty() {
        assert!(require_token(None).is_err());
        assert!(require_token(Some("")).is_err());
        assert!(require_token(Some("tok")).is_ok());
    }
}
