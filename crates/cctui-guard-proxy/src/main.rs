//! `cctui-guard-proxy`: an egress allow-list proxy for the cctui worker. Two
//! modes share one policy engine and health endpoint:
//!
//! - `transparent` (default): the iptables REDIRECT target. Recovers the
//!   original destination via `SO_ORIGINAL_DST`, enforces policy on the SNI /
//!   Host name, then tunnels.
//! - `forward`: a standard HTTP proxy (CONNECT + absolute-URI) for the
//!   no-NET_ADMIN path, where the worker sets `HTTP_PROXY`/`HTTPS_PROXY`.
//!
//! IPv4 only — IPv6 egress is denied at the iptables layer by the worker
//! entrypoint.

mod denylist;
mod forward;
mod ghapp;
mod health;
mod inject;
mod peek;
mod policy;
mod secrets;
mod transparent;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};

use crate::forward::ForwardListener;
use crate::ghapp::{GhAppConfig, GhAppMinter};
use crate::inject::{InjectionPolicy, Injector, PerPodCa};
use crate::policy::PolicyManager;
use crate::secrets::{
    AwsSmBackend, EnvBackend, SecretBackend, SecretSource, VaultBackend, VaultConfig,
};
use crate::transparent::TransparentListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// iptables REDIRECT target; recover original dst + SNI/Host (default).
    Transparent,
    /// Standard HTTP proxy: CONNECT + absolute-URI.
    Forward,
}

#[derive(Debug, Parser)]
#[command(name = "cctui-guard-proxy", about = "Egress allow-list proxy")]
struct Args {
    /// Proxy operating mode.
    #[arg(long, value_enum, default_value = "transparent", env = "GUARD_PROXY_MODE")]
    mode: Mode,

    /// Address the proxy listens on.
    #[arg(long, default_value = "0.0.0.0:15001", env = "GUARD_PROXY_LISTEN")]
    listen: String,

    /// Address the health endpoint listens on.
    #[arg(long, default_value = "0.0.0.0:15002", env = "GUARD_PROXY_HEALTH_LISTEN")]
    health_listen: String,

    /// Path to the JSON policy file.
    #[arg(long, default_value = "/var/run/guard-proxy/policy.json", env = "GUARD_PROXY_POLICY")]
    policy: String,

    /// Credential backend for injection (CCT-718). `none` keeps the feature
    /// inert: the proxy behaves exactly as before.
    #[arg(long, value_enum, default_value = "none", env = "GUARD_PROXY_SECRET_SOURCE")]
    secret_source: SecretSourceKind,

    /// TTL of the in-memory secret cache; rotation lands within one TTL.
    #[arg(long, default_value_t = 120, env = "GUARD_PROXY_SECRET_TTL_SECS")]
    secret_ttl_secs: u64,

    /// Vault/OpenBao base address, e.g. `http://vault.vault:8200`.
    #[arg(long, env = "GUARD_PROXY_VAULT_ADDR")]
    vault_addr: Option<String>,

    /// Vault Kubernetes auth role.
    #[arg(long, env = "GUARD_PROXY_VAULT_ROLE")]
    vault_role: Option<String>,

    /// Vault KV v2 mount.
    #[arg(long, default_value = "secret", env = "GUARD_PROXY_VAULT_MOUNT")]
    vault_mount: String,

    /// Vault path prefix; secrets are read at `<mount>/data/<prefix>/<identity>/<service>`.
    #[arg(long, default_value = "cctui/workers", env = "GUARD_PROXY_VAULT_PATH_PREFIX")]
    vault_path_prefix: String,

    /// Field read from the KV v2 secret data.
    #[arg(long, default_value = "value", env = "GUARD_PROXY_VAULT_FIELD")]
    vault_field: String,

    /// Service account token used for the Vault Kubernetes login.
    #[arg(
        long,
        default_value = "/var/run/secrets/kubernetes.io/serviceaccount/token",
        env = "GUARD_PROXY_VAULT_TOKEN_PATH"
    )]
    vault_token_path: PathBuf,

    /// AWS Secrets Manager name prefix; secrets are named `<prefix><identity>/<service>`.
    #[arg(long, default_value = "cctui/worker/", env = "GUARD_PROXY_AWS_SM_PREFIX")]
    aws_sm_prefix: String,

    /// Task identity — the first half of every secret lookup key (CCT-718).
    /// Required when `--inject-hosts` is non-empty.
    #[arg(long, default_value = "", env = "GUARD_PROXY_IDENTITY")]
    identity: String,

    /// Hosts to TLS-terminate + credential-inject (CCT-718). Each token is
    /// `host`, `host=service`, or `host=service:<shape>` (bearer|basic|slack).
    /// Empty = passthrough only; injection also needs a real `--secret-source`.
    #[arg(long, value_delimiter = ',', env = "GUARD_PROXY_INJECT_HOSTS")]
    inject_hosts: Vec<String>,

    /// Where to write the per-pod CA cert (PEM) for the worker to install. Only
    /// written when injection is active.
    #[arg(long, default_value = "/var/run/guard-proxy-ca/ca.pem", env = "GUARD_PROXY_CA_CERT_OUT")]
    ca_cert_out: PathBuf,

    /// GitHub App id (CCT-722). Set together with `--github-app-installation-id`
    /// to mint short-lived installation tokens for the `github` service instead
    /// of injecting a stored PAT. The App private key is NOT on the CLI: it is
    /// fetched from the secret store as `(identity, "github-app-key")`. Unset ⇒
    /// inert (the `github` service uses the stored credential as before).
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_ID")]
    github_app_id: Option<String>,

    /// GitHub App installation id the token is minted for (CCT-722).
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_INSTALLATION_ID")]
    github_app_installation_id: Option<String>,

    /// Optional repository names (bare `name`, not `owner/name`) to scope the
    /// minted installation token to. Empty = the installation's default set.
    #[arg(long, value_delimiter = ',', env = "GUARD_PROXY_GITHUB_APP_REPOS")]
    github_app_repos: Vec<String>,

    /// GitHub REST base for the token exchange; override only for testing.
    #[arg(
        long,
        default_value = crate::ghapp::DEFAULT_API_BASE,
        env = "GUARD_PROXY_GITHUB_APP_API_BASE"
    )]
    github_app_api_base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SecretSourceKind {
    /// No secret source (default).
    None,
    /// `CRED_<IDENTITY>_<SERVICE>` from the sidecar's own env.
    Env,
    /// `HashiCorp` Vault / `OpenBao` KV v2 with Kubernetes auth.
    Vault,
    /// AWS Secrets Manager via the SDK default credential chain.
    AwsSm,
}

async fn build_secret_source(args: &Args) -> anyhow::Result<Option<Arc<SecretSource>>> {
    let backend: Box<dyn SecretBackend> = match args.secret_source {
        SecretSourceKind::None => return Ok(None),
        SecretSourceKind::Env => Box::new(EnvBackend::from_process_env()),
        SecretSourceKind::Vault => {
            let addr = args
                .vault_addr
                .clone()
                .context("--vault-addr is required for --secret-source vault")?;
            let role = args
                .vault_role
                .clone()
                .context("--vault-role is required for --secret-source vault")?;
            Box::new(VaultBackend::new(VaultConfig {
                addr,
                role,
                mount: args.vault_mount.clone(),
                path_prefix: args.vault_path_prefix.clone(),
                field: args.vault_field.clone(),
                token_path: args.vault_token_path.clone(),
            })?)
        }
        SecretSourceKind::AwsSm => {
            Box::new(AwsSmBackend::from_default_chain(args.aws_sm_prefix.clone()).await)
        }
    };
    tracing::info!(
        "secret source enabled: {:?} (ttl={}s)",
        args.secret_source,
        args.secret_ttl_secs
    );
    Ok(Some(Arc::new(SecretSource::new(backend, Duration::from_secs(args.secret_ttl_secs)))))
}

/// Wires the injection layer: parses the host allowlist, mints the per-pod CA,
/// writes its public cert for the worker to install, and builds the TLS
/// injector. Returns `None` (passthrough) when there is no secret source or no
/// inject hosts configured.
fn build_injection(
    args: &Args,
    secret_source: Option<Arc<SecretSource>>,
) -> anyhow::Result<Option<Arc<Injector>>> {
    let Some(secrets) = secret_source else {
        return Ok(None);
    };
    let policy = InjectionPolicy::from_hosts(args.identity.clone(), &args.inject_hosts);
    if policy.is_empty() {
        tracing::info!("no inject hosts configured — TLS passthrough only");
        return Ok(None);
    }
    anyhow::ensure!(!args.identity.is_empty(), "--identity is required with --inject-hosts");

    let ca = Arc::new(PerPodCa::generate()?);
    write_ca_cert(&args.ca_cert_out, ca.ca_pem())
        .with_context(|| format!("writing CA cert to {}", args.ca_cert_out.display()))?;
    tracing::info!(
        "per-pod CA minted; wrote CA cert to {} (identity={})",
        args.ca_cert_out.display(),
        args.identity
    );

    let ghapp = build_ghapp(args, &secrets)?;
    let injector = Injector::new(ca, secrets, policy, ghapp)?;
    Ok(Some(Arc::new(injector)))
}

/// Builds the GitHub-App installation-token minter (CCT-722) when both an App id
/// and installation id are configured. Returns `None` (stored-PAT/passthrough
/// behavior) otherwise, so the App path is inert until an operator wires it up.
fn build_ghapp(
    args: &Args,
    secrets: &Arc<SecretSource>,
) -> anyhow::Result<Option<Arc<GhAppMinter>>> {
    let (Some(app_id), Some(installation_id)) =
        (args.github_app_id.clone(), args.github_app_installation_id.clone())
    else {
        return Ok(None);
    };
    let mut config =
        GhAppConfig::new(app_id, installation_id, args.github_app_repos.clone());
    config.api_base.clone_from(&args.github_app_api_base);
    tracing::info!(
        "github-app token minting enabled: app_id={} installation_id={} repos={:?}",
        config.app_id,
        config.installation_id,
        config.repositories
    );
    Ok(Some(Arc::new(GhAppMinter::new(secrets.clone(), config)?)))
}

/// Writes the public CA cert (PEM) world-readable so the worker (a different
/// uid) can install it. The private key never leaves the sidecar process.
fn write_ca_cert(path: &std::path::Path, pem: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pem.tmp");
    std::fs::write(&tmp, pem.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    tracing::info!(
        "starting guard-proxy mode={:?} listen={} health={} policy={}",
        args.mode,
        args.listen,
        args.health_listen,
        args.policy
    );

    let secret_source = build_secret_source(&args).await?;
    let injection = build_injection(&args, secret_source)?;

    let policy = Arc::new(PolicyManager::new(&args.policy));
    if let Err(e) = policy.load() {
        // Fail closed: an unreadable/invalid policy stays deny-all.
        tracing::warn!("failed to load initial policy (deny-all until fixed): {e}");
    }

    // Hot-reload via mtime poll.
    tokio::spawn(policy.clone().watch(Duration::from_secs(1)));

    // Health endpoint.
    {
        let policy = policy.clone();
        let addr = args.health_listen.clone();
        tokio::spawn(async move {
            if let Err(e) = health::serve(&addr, policy).await {
                tracing::error!("health server exited: {e}");
            }
        });
    }

    match args.mode {
        Mode::Transparent => {
            let listener = TransparentListener::bind(&args.listen, policy, injection).await?;
            tracing::info!("transparent proxy listening on {}", args.listen);
            listener.serve().await?;
        }
        Mode::Forward => {
            let listener = ForwardListener::bind(&args.listen, policy, injection).await?;
            tracing::info!("forward proxy listening on {}", args.listen);
            listener.serve().await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("cctui-guard-proxy").chain(argv.iter().copied()))
            .unwrap()
    }

    #[test]
    fn secret_source_defaults_to_none() {
        let args = parse(&[]);
        assert_eq!(args.secret_source, SecretSourceKind::None);
        assert_eq!(args.secret_ttl_secs, 120);
    }

    #[tokio::test]
    async fn secret_source_none_builds_nothing() {
        assert!(build_secret_source(&parse(&[])).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn secret_source_env_builds() {
        let args = parse(&["--secret-source", "env", "--secret-ttl-secs", "30"]);
        assert_eq!(args.secret_source, SecretSourceKind::Env);
        assert_eq!(args.secret_ttl_secs, 30);
        assert!(build_secret_source(&args).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn secret_source_vault_requires_addr_and_role() {
        let args = parse(&["--secret-source", "vault"]);
        let err = build_secret_source(&args).await.err().unwrap().to_string();
        assert!(err.contains("--vault-addr"), "{err}");

        let args = parse(&["--secret-source", "vault", "--vault-addr", "http://vault:8200"]);
        let err = build_secret_source(&args).await.err().unwrap().to_string();
        assert!(err.contains("--vault-role"), "{err}");
    }

    #[tokio::test]
    async fn secret_source_vault_builds_with_full_config() {
        let args = parse(&[
            "--secret-source",
            "vault",
            "--vault-addr",
            "http://vault:8200",
            "--vault-role",
            "cctui-worker",
            "--vault-mount",
            "kvmount",
            "--vault-path-prefix",
            "cctui/workers",
        ]);
        assert_eq!(args.vault_mount, "kvmount");
        assert!(build_secret_source(&args).await.unwrap().is_some());
    }

    #[test]
    fn secret_source_aws_sm_parses_prefix() {
        let args = parse(&["--secret-source", "aws-sm", "--aws-sm-prefix", "prod/cctui"]);
        assert_eq!(args.secret_source, SecretSourceKind::AwsSm);
        assert_eq!(args.aws_sm_prefix, "prod/cctui");
    }

    #[test]
    fn secret_source_rejects_unknown_backend() {
        assert!(Args::try_parse_from(["cctui-guard-proxy", "--secret-source", "bogus"]).is_err());
    }

    #[test]
    fn github_app_flags_default_inert() {
        let args = parse(&[]);
        assert!(args.github_app_id.is_none());
        assert!(args.github_app_installation_id.is_none());
        assert!(args.github_app_repos.is_empty());
        assert_eq!(args.github_app_api_base, crate::ghapp::DEFAULT_API_BASE);
    }

    #[test]
    fn github_app_flags_parse() {
        let args = parse(&[
            "--github-app-id",
            "123",
            "--github-app-installation-id",
            "456",
            "--github-app-repos",
            "cctui,infra",
        ]);
        assert_eq!(args.github_app_id.as_deref(), Some("123"));
        assert_eq!(args.github_app_installation_id.as_deref(), Some("456"));
        assert_eq!(args.github_app_repos, vec!["cctui", "infra"]);
    }

    #[tokio::test]
    async fn ghapp_inert_without_both_ids() {
        let secrets = build_secret_source(&parse(&["--secret-source", "env"])).await.unwrap().unwrap();
        assert!(build_ghapp(&parse(&["--github-app-id", "123"]), &secrets).unwrap().is_none());
        assert!(build_ghapp(&parse(&[]), &secrets).unwrap().is_none());
        assert!(
            build_ghapp(
                &parse(&["--github-app-id", "1", "--github-app-installation-id", "2"]),
                &secrets
            )
            .unwrap()
            .is_some()
        );
    }
}
