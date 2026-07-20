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
use clap::{Parser, Subcommand, ValueEnum};

use crate::forward::ForwardListener;
use crate::ghapp::{GhAppConfig, GhAppMinter};
use crate::inject::{InjectionPolicy, InjectionRule, Injector, PerPodCa};
use crate::policy::PolicyManager;
use crate::secrets::{
    Engines, K8sClient, RefResolver as _, SecretRef, SecretSource, VaultClient, render_identity,
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
    #[command(subcommand)]
    command: Option<Cmd>,

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

    /// Path to the JSON injection config (an array of `{host, shape, secret,
    /// …}` entries, each naming an explicit `env:`/`vault:`/`aws-sm:`/`k8s:`
    /// secret ref). When the file exists it fully defines injection and the
    /// legacy `--secret-source`/`--inject-hosts` pair is ignored. MUST NOT live
    /// in a worker-writable location.
    #[arg(long, default_value = "/etc/guard-proxy/inject.json", env = "GUARD_PROXY_INJECT_CONFIG")]
    inject_config: PathBuf,

    /// Legacy credential backend for `--inject-hosts` (CCT-718). `none` keeps
    /// the feature inert. Ignored when the inject config file exists.
    #[arg(long, value_enum, default_value = "none", env = "GUARD_PROXY_SECRET_SOURCE")]
    secret_source: SecretSourceKind,

    /// TTL of the in-memory secret cache; rotation lands within one TTL.
    #[arg(long, default_value_t = 120, env = "GUARD_PROXY_SECRET_TTL_SECS")]
    secret_ttl_secs: u64,

    /// Vault/OpenBao base address, e.g. `http://vault.vault:8200`. Enables the
    /// `vault:` ref engine.
    #[arg(long, env = "GUARD_PROXY_VAULT_ADDR")]
    vault_addr: Option<String>,

    /// Vault Kubernetes auth role.
    #[arg(long, env = "GUARD_PROXY_VAULT_ROLE")]
    vault_role: Option<String>,

    /// Vault KV v2 mount (legacy `--inject-hosts` ref derivation only).
    #[arg(long, default_value = "secret", env = "GUARD_PROXY_VAULT_MOUNT")]
    vault_mount: String,

    /// Vault path prefix (legacy derivation:
    /// `<mount>/data/<prefix>/<identity>/<service>`).
    #[arg(long, default_value = "cctui/workers", env = "GUARD_PROXY_VAULT_PATH_PREFIX")]
    vault_path_prefix: String,

    /// Field read from the KV v2 secret data (legacy derivation).
    #[arg(long, default_value = "value", env = "GUARD_PROXY_VAULT_FIELD")]
    vault_field: String,

    /// Service account token used for the Vault Kubernetes login.
    #[arg(
        long,
        default_value = "/var/run/secrets/kubernetes.io/serviceaccount/token",
        env = "GUARD_PROXY_VAULT_TOKEN_PATH"
    )]
    vault_token_path: PathBuf,

    /// AWS Secrets Manager name prefix (legacy derivation:
    /// `<prefix><identity>/<service>`).
    #[arg(long, default_value = "cctui/worker/", env = "GUARD_PROXY_AWS_SM_PREFIX")]
    aws_sm_prefix: String,

    /// Task identity, substituted into `${IDENTITY}`/`${identity}` secret-ref
    /// placeholders. Required by refs that use a placeholder and by the legacy
    /// `--inject-hosts` derivation.
    #[arg(long, default_value = "", env = "GUARD_PROXY_IDENTITY")]
    identity: String,

    /// Legacy host list to TLS-terminate + credential-inject (CCT-718). Each
    /// token is `host`, `host=service`, or `host=service:<shape>`
    /// (bearer|basic|cookie). Ignored when the inject config file exists;
    /// otherwise also needs a real `--secret-source`.
    #[arg(long, value_delimiter = ',', env = "GUARD_PROXY_INJECT_HOSTS")]
    inject_hosts: Vec<String>,

    /// Where to write the per-pod CA cert (PEM) for the worker to install. Only
    /// written when injection is active.
    #[arg(long, default_value = "/var/run/guard-proxy-ca/ca.pem", env = "GUARD_PROXY_CA_CERT_OUT")]
    ca_cert_out: PathBuf,

    /// GitHub App id (CCT-722). Set together with `--github-app-installation-id`
    /// to mint short-lived installation tokens for the `github` service instead
    /// of injecting a stored PAT. The App private key is NOT on the CLI: it is
    /// fetched via `--github-app-key-secret`. Unset ⇒ inert (the `github`
    /// service uses the stored credential as before).
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_ID")]
    github_app_id: Option<String>,

    /// GitHub App installation id the token is minted for (CCT-722).
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_INSTALLATION_ID")]
    github_app_installation_id: Option<String>,

    /// Secret ref of the App private-key PEM (identity placeholders apply).
    /// Falls back to the legacy `(identity, "github-app-key")` derivation when
    /// a legacy `--secret-source` is configured.
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_KEY_SECRET")]
    github_app_key_secret: Option<String>,

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

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Resolve one secret ref (with identity templating) and print its value to
    /// stdout — used by the sidecar entrypoint to fetch the GPG signing key.
    FetchSecret {
        /// The ref, e.g. `vault:<mount>/data/<path>#FIELD_${IDENTITY}`.
        secret_ref: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SecretSourceKind {
    /// No legacy secret source (default).
    None,
    /// `CRED_<IDENTITY>_<SERVICE>` from the sidecar's own env.
    Env,
    /// `HashiCorp` Vault / `OpenBao` KV v2 with Kubernetes auth.
    Vault,
    /// AWS Secrets Manager via the SDK default credential chain.
    AwsSm,
}

/// Builds the engine set every ref resolution goes through. The `vault:` engine
/// needs `--vault-addr` + `--vault-role`; the `k8s:` engine needs the
/// in-cluster environment; `env:`/`aws-sm:` are always available.
fn build_engines(args: &Args) -> anyhow::Result<Engines> {
    let vault = match (&args.vault_addr, &args.vault_role) {
        (Some(addr), Some(role)) => {
            Some(VaultClient::new(addr.clone(), role.clone(), args.vault_token_path.clone())?)
        }
        (Some(_), None) => anyhow::bail!("--vault-role is required with --vault-addr"),
        (None, _) => None,
    };
    Ok(Engines::new(vault, K8sClient::in_cluster().ok()))
}

/// The injection rule set: the JSON inject config when present (authoritative),
/// else the legacy `--secret-source` + `--inject-hosts` pair with refs derived
/// from the backend's naming convention. Empty ⇒ injection inert.
fn build_rules(args: &Args) -> anyhow::Result<Vec<InjectionRule>> {
    if args.inject_config.is_file() {
        let json = std::fs::read_to_string(&args.inject_config)
            .with_context(|| format!("reading {}", args.inject_config.display()))?;
        let rules = inject::load_inject_config(&json, &args.identity)
            .with_context(|| format!("loading {}", args.inject_config.display()))?;
        tracing::info!(
            "loaded {} injection rule(s) from {}",
            rules.len(),
            args.inject_config.display()
        );
        return Ok(rules);
    }

    if args.secret_source == SecretSourceKind::None {
        return Ok(Vec::new());
    }
    let specs: Vec<_> =
        args.inject_hosts.iter().filter_map(|h| inject::parse_inject_host(h)).collect();
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(!args.identity.is_empty(), "--identity is required with --inject-hosts");
    if args.secret_source == SecretSourceKind::Vault {
        anyhow::ensure!(
            args.vault_addr.is_some() && args.vault_role.is_some(),
            "--vault-addr and --vault-role are required for --secret-source vault"
        );
    }

    let legacy_ref = |service: &str| match args.secret_source {
        SecretSourceKind::Env => SecretRef::legacy_env(&args.identity, service),
        SecretSourceKind::Vault => SecretRef::legacy_vault(
            &args.vault_mount,
            &args.vault_path_prefix,
            &args.vault_field,
            &args.identity,
            service,
        ),
        SecretSourceKind::AwsSm => {
            SecretRef::legacy_aws_sm(&args.aws_sm_prefix, &args.identity, service)
        }
        SecretSourceKind::None => unreachable!("gated above"),
    };
    Ok(specs
        .into_iter()
        .map(|spec| {
            let secret = legacy_ref(&spec.service);
            let cookie_secret = matches!(spec.shape, inject::AuthShape::BearerCookie { .. })
                .then(|| legacy_ref(&format!("{}-cookie", spec.service)));
            InjectionRule {
                host: spec.host,
                path_prefix: None,
                service: spec.service,
                shape: spec.shape,
                secret,
                cookie_secret,
            }
        })
        .collect())
}

/// Wires the injection layer: mints the per-pod CA, writes its public cert for
/// the worker to install, and builds the TLS injector over the rule set.
/// Returns `None` (passthrough) when no rules are configured.
fn build_injection(
    args: &Args,
    rules: Vec<InjectionRule>,
) -> anyhow::Result<Option<Arc<Injector>>> {
    if rules.is_empty() {
        tracing::info!("no injection rules configured — TLS passthrough only");
        return Ok(None);
    }
    let secrets = Arc::new(SecretSource::new(
        Box::new(build_engines(args)?),
        Duration::from_secs(args.secret_ttl_secs),
    ));

    let ca = Arc::new(PerPodCa::generate()?);
    write_ca_cert(&args.ca_cert_out, ca.ca_pem())
        .with_context(|| format!("writing CA cert to {}", args.ca_cert_out.display()))?;
    tracing::info!(
        "per-pod CA minted; wrote CA cert to {} ({} rule(s), ttl={}s)",
        args.ca_cert_out.display(),
        rules.len(),
        args.secret_ttl_secs
    );

    let ghapp = build_ghapp(args, &secrets)?;
    let injector = Injector::new(ca, secrets, InjectionPolicy::new(rules), ghapp)?;
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
    let key_ref = match &args.github_app_key_secret {
        Some(template) => SecretRef::parse(&render_identity(template, &args.identity)?)?,
        None if args.secret_source != SecretSourceKind::None => {
            let legacy = build_rules_legacy_key(args);
            tracing::info!("github-app key ref derived from legacy secret source: {legacy}");
            legacy
        }
        None => anyhow::bail!(
            "--github-app-key-secret is required with --github-app-id when no legacy \
             --secret-source is configured"
        ),
    };
    let mut config = GhAppConfig::new(app_id, installation_id, args.github_app_repos.clone());
    config.api_base.clone_from(&args.github_app_api_base);
    tracing::info!(
        "github-app token minting enabled: app_id={} installation_id={} repos={:?}",
        config.app_id,
        config.installation_id,
        config.repositories
    );
    Ok(Some(Arc::new(GhAppMinter::new(secrets.clone(), config, key_ref)?)))
}

fn build_rules_legacy_key(args: &Args) -> SecretRef {
    match args.secret_source {
        SecretSourceKind::Vault => SecretRef::legacy_vault(
            &args.vault_mount,
            &args.vault_path_prefix,
            &args.vault_field,
            &args.identity,
            "github-app-key",
        ),
        SecretSourceKind::AwsSm => {
            SecretRef::legacy_aws_sm(&args.aws_sm_prefix, &args.identity, "github-app-key")
        }
        SecretSourceKind::Env | SecretSourceKind::None => {
            SecretRef::legacy_env(&args.identity, "github-app-key")
        }
    }
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

/// `fetch-secret`: resolve one ref and print the raw value to stdout. Runs
/// before tracing init so nothing but the secret reaches stdout.
async fn fetch_secret(args: &Args, template: &str) -> anyhow::Result<()> {
    use std::io::Write as _;

    let rendered = render_identity(template, &args.identity)?;
    let r = SecretRef::parse(&rendered)?;
    let cred = build_engines(args)?.resolve(&r).await?;
    let mut out = std::io::stdout();
    out.write_all(cred.expose().as_bytes())?;
    out.flush()?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if let Some(Cmd::FetchSecret { secret_ref }) = &args.command {
        return fetch_secret(&args, secret_ref).await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!(
        "starting guard-proxy mode={:?} listen={} health={} policy={}",
        args.mode,
        args.listen,
        args.health_listen,
        args.policy
    );

    let rules = build_rules(&args)?;
    let injection = build_injection(&args, rules)?;

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
        assert_eq!(args.inject_config, PathBuf::from("/etc/guard-proxy/inject.json"));
    }

    #[test]
    fn no_source_and_no_config_yields_no_rules() {
        let mut args = parse(&["--inject-hosts", "api.github.com"]);
        args.inject_config = PathBuf::from("/nonexistent/inject.json");
        assert!(build_rules(&args).unwrap().is_empty());
    }

    #[test]
    fn legacy_env_rules_derive_cred_refs() {
        let mut args = parse(&[
            "--secret-source",
            "env",
            "--identity",
            "acme",
            "--inject-hosts",
            "api.github.com,github.com,slack.com",
        ]);
        args.inject_config = PathBuf::from("/nonexistent/inject.json");
        let rules = build_rules(&args).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].secret, SecretRef::parse("env:CRED_ACME_GITHUB").unwrap());
        assert!(rules[0].cookie_secret.is_none());
        assert_eq!(
            rules[2].cookie_secret,
            Some(SecretRef::parse("env:CRED_ACME_SLACK_COOKIE").unwrap())
        );
    }

    #[test]
    fn legacy_vault_rules_derive_path_refs() {
        let mut args = parse(&[
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
            "--identity",
            "acme",
            "--inject-hosts",
            "api.github.com",
        ]);
        args.inject_config = PathBuf::from("/nonexistent/inject.json");
        let rules = build_rules(&args).unwrap();
        assert_eq!(
            rules[0].secret,
            SecretRef::parse("vault:kvmount/data/cctui/workers/acme/github#value").unwrap()
        );
    }

    #[test]
    fn legacy_rules_require_identity_and_vault_config() {
        let mut args = parse(&["--secret-source", "env", "--inject-hosts", "api.github.com"]);
        args.inject_config = PathBuf::from("/nonexistent/inject.json");
        assert!(build_rules(&args).unwrap_err().to_string().contains("--identity"));

        let mut args = parse(&[
            "--secret-source",
            "vault",
            "--identity",
            "acme",
            "--inject-hosts",
            "api.github.com",
        ]);
        args.inject_config = PathBuf::from("/nonexistent/inject.json");
        assert!(build_rules(&args).unwrap_err().to_string().contains("--vault-addr"));
    }

    #[test]
    fn inject_config_file_is_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inject.json");
        std::fs::write(
            &path,
            r#"[{"host": "api.github.com", "service": "github",
                 "secret": "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_${IDENTITY}"}]"#,
        )
        .unwrap();
        // secret-source stays none — the file alone activates injection.
        let mut args = parse(&["--identity", "acme"]);
        args.inject_config = path;
        let rules = build_rules(&args).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].secret,
            SecretRef::parse("vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME").unwrap()
        );
    }

    #[test]
    fn inject_config_file_errors_fail_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inject.json");
        std::fs::write(&path, r#"[{"host": "a.com", "secret": "bogus"}]"#).unwrap();
        let mut args = parse(&[]);
        args.inject_config = path;
        assert!(build_rules(&args).is_err());
    }

    #[test]
    fn engines_require_role_with_addr() {
        let args = parse(&["--vault-addr", "http://vault:8200"]);
        assert!(build_engines(&args).unwrap_err().to_string().contains("--vault-role"));
        let args = parse(&["--vault-addr", "http://vault:8200", "--vault-role", "r"]);
        assert!(build_engines(&args).is_ok());
        assert!(build_engines(&parse(&[])).is_ok());
    }

    #[test]
    fn secret_source_rejects_unknown_backend() {
        assert!(Args::try_parse_from(["cctui-guard-proxy", "--secret-source", "bogus"]).is_err());
    }

    #[test]
    fn fetch_secret_subcommand_parses() {
        let args = parse(&["fetch-secret", "env:GPG_KEY"]);
        assert!(
            matches!(args.command, Some(Cmd::FetchSecret { ref secret_ref }) if secret_ref == "env:GPG_KEY")
        );
        assert!(parse(&[]).command.is_none());
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
            "--github-app-key-secret",
            "vault:kvmount/data/cctui/workers#APP_KEY_${IDENTITY}",
        ]);
        assert_eq!(args.github_app_id.as_deref(), Some("123"));
        assert_eq!(args.github_app_installation_id.as_deref(), Some("456"));
        assert_eq!(args.github_app_repos, vec!["cctui", "infra"]);
    }

    fn test_secrets() -> Arc<SecretSource> {
        Arc::new(SecretSource::new(Box::new(Engines::new(None, None)), Duration::from_secs(120)))
    }

    #[test]
    fn ghapp_inert_without_both_ids() {
        let secrets = test_secrets();
        assert!(build_ghapp(&parse(&["--github-app-id", "123"]), &secrets).unwrap().is_none());
        assert!(build_ghapp(&parse(&[]), &secrets).unwrap().is_none());
    }

    #[test]
    fn ghapp_key_ref_explicit_legacy_or_error() {
        let secrets = test_secrets();
        let with_ref = parse(&[
            "--github-app-id",
            "1",
            "--github-app-installation-id",
            "2",
            "--github-app-key-secret",
            "env:APP_KEY_${IDENTITY}",
            "--identity",
            "acme",
        ]);
        assert!(build_ghapp(&with_ref, &secrets).unwrap().is_some());

        let legacy = parse(&[
            "--github-app-id",
            "1",
            "--github-app-installation-id",
            "2",
            "--secret-source",
            "env",
            "--identity",
            "acme",
        ]);
        assert!(build_ghapp(&legacy, &secrets).unwrap().is_some());

        let missing = parse(&["--github-app-id", "1", "--github-app-installation-id", "2"]);
        assert!(build_ghapp(&missing, &secrets).is_err());
    }
}
