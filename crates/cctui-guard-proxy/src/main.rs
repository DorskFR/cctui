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

mod aws_creds;
mod denylist;
mod dispatch;
mod forward;
mod gate;
mod ghapp;
mod health;
mod inject;
mod peek;
mod policy;
mod private_allow;
mod secrets;
mod server_inject;
mod shutdown;
mod transparent;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand, ValueEnum};

use crate::dispatch::{DispatchConfig, DispatchIdentity, ServerTarget};
use crate::forward::ForwardListener;
use crate::gate::{Gate, GatePolicy, HumanGate};
use crate::ghapp::{GhAppConfig, GhAppMinter};
use crate::inject::{InjectionPolicy, InjectionRule, Injector, PerPodCa};
use crate::policy::PolicyManager;
use crate::private_allow::PrivateAllow;
use crate::secrets::{
    Engines, K8sClient, RefResolver as _, SecretRef, SecretSource, VaultClient, render_identity,
};
use crate::server_inject::ServerInjector;
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

    /// Extra `host[:port] [label]` lines whose private addresses may be dialed,
    /// seeded by the worker entrypoint. Absent ⇒ none; re-read on change.
    #[arg(
        long,
        default_value = "/var/run/guard-proxy/private-allow",
        env = "GUARD_PROXY_PRIVATE_ALLOW_FILE"
    )]
    private_allow_file: PathBuf,

    /// Append every egress verdict as a JSON line here — the same file the guard
    /// writes its `/check` + `/transition` decisions to, so the end-of-run report
    /// can attribute a denied host to the active step. Unset ⇒ no log.
    #[arg(long = "decision-log", env = "GUARD_DECISION_LOG")]
    decision_log: Option<PathBuf>,

    /// Path to the JSON injection config (an array of `{host, shape, secret,
    /// …}` entries, each naming an explicit `env:`/`vault:`/`aws-sm:`/`k8s:`
    /// secret ref). The file fully defines injection; absent ⇒ pure passthrough.
    /// MUST NOT live in a worker-writable location.
    #[arg(long, default_value = "/etc/guard-proxy/inject.json", env = "GUARD_PROXY_INJECT_CONFIG")]
    inject_config: PathBuf,

    /// TTL of the in-memory secret cache; rotation lands within one TTL, or at
    /// once on SIGHUP. A refresh that hits a backend failure keeps the cached
    /// value.
    #[arg(long, default_value_t = 120, env = "GUARD_PROXY_SECRET_TTL_SECS")]
    secret_ttl_secs: u64,

    /// Vault/OpenBao base address, e.g. `http://vault.vault:8200`. Enables the
    /// `vault:` ref engine.
    #[arg(long, env = "GUARD_PROXY_VAULT_ADDR")]
    vault_addr: Option<String>,

    /// Vault Kubernetes auth role.
    #[arg(long, env = "GUARD_PROXY_VAULT_ROLE")]
    vault_role: Option<String>,

    /// Service account token used for the Vault Kubernetes login.
    #[arg(
        long,
        default_value = "/var/run/secrets/kubernetes.io/serviceaccount/token",
        env = "GUARD_PROXY_VAULT_TOKEN_PATH"
    )]
    vault_token_path: PathBuf,

    /// Task identity, substituted into `${IDENTITY}`/`${identity}` secret-ref
    /// placeholders. Required by refs that use a placeholder.
    #[arg(long, default_value = "", env = "GUARD_PROXY_IDENTITY")]
    identity: String,

    /// Where to write the per-pod CA cert (PEM) for the worker to install. Only
    /// written when injection is active.
    #[arg(long, default_value = "/var/run/guard-proxy-ca/ca.pem", env = "GUARD_PROXY_CA_CERT_OUT")]
    ca_cert_out: PathBuf,

    /// GitHub App id. Set together with `--github-app-installation-id`
    /// to mint short-lived installation tokens for the `github` service instead
    /// of injecting a stored PAT. The App private key is NOT on the CLI: it is
    /// fetched via `--github-app-key-secret`. Unset ⇒ inert (the `github`
    /// service uses the stored credential).
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_ID")]
    github_app_id: Option<String>,

    /// GitHub App installation id the token is minted for.
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_INSTALLATION_ID")]
    github_app_installation_id: Option<String>,

    /// Secret ref of the App private-key PEM (identity placeholders apply).
    /// Required whenever the App ids are set.
    #[arg(long, env = "GUARD_PROXY_GITHUB_APP_KEY_SECRET")]
    github_app_key_secret: Option<String>,

    /// Optional repository names (bare `name`, not `owner/name`) to scope the
    /// minted installation token to. Empty = the installation's default set.
    #[arg(long, value_delimiter = ',', env = "GUARD_PROXY_GITHUB_APP_REPOS")]
    github_app_repos: Vec<String>,

    /// The dispatch this pod redeems its projected `cctui` token for, from the
    /// pod's `cctui.dev/dispatch-id` annotation. Empty ⇒ not a dispatched pod.
    #[arg(long, default_value = "", env = "CCTUI_DISPATCH_ID")]
    dispatch_id: String,

    /// The cctui server the dispatch redeems against and whose host gets the
    /// machine key injected.
    #[arg(long, default_value = "", env = "CCTUI_DISPATCH_SERVER_URL")]
    dispatch_server_url: String,

    /// The projected `cctui`-audience service-account token.
    #[arg(
        long,
        default_value = "/var/run/secrets/kubernetes.io/serviceaccount/cctui-token",
        env = "GUARD_PROXY_DISPATCH_TOKEN_PATH"
    )]
    dispatch_token_path: PathBuf,

    /// GitHub REST base for the token exchange; override only for testing.
    #[arg(
        long,
        default_value = crate::ghapp::DEFAULT_API_BASE,
        env = "GUARD_PROXY_GITHUB_APP_API_BASE"
    )]
    github_app_api_base: String,

    /// The profile's `humanGate`, rendered by the admission webhook. A value
    /// that does not parse refuses every outbound connection.
    #[arg(long, env = "GUARD_PROXY_HUMAN_GATE")]
    human_gate: Option<String>,

    /// Branch names a `draftPrOnly` gate protects on top of `main`, `master`
    /// and `trunk`.
    #[arg(long, value_delimiter = ',', env = "GUARD_PROXY_PROTECTED_BRANCHES")]
    protected_branches: Vec<String>,

    /// On SIGTERM/SIGINT, how long in-flight connections may finish before
    /// they are cut. Keep well inside the pod's terminationGracePeriodSeconds.
    #[arg(long, default_value_t = 10, env = "GUARD_PROXY_SHUTDOWN_DRAIN_SECS")]
    shutdown_drain_secs: u64,
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

/// Builds the engine set every ref resolution goes through. The `vault:` engine
/// needs `--vault-addr` + `--vault-role`; the `k8s:` engine needs the
/// in-cluster environment; `env:`/`aws-sm:` are always available.
fn build_engines(args: &Args) -> anyhow::Result<Engines> {
    crate::inject::install_crypto();
    let vault = match (&args.vault_addr, &args.vault_role) {
        (Some(addr), Some(role)) => {
            Some(VaultClient::new(addr.clone(), role.clone(), args.vault_token_path.clone())?)
        }
        (Some(_), None) => anyhow::bail!("--vault-role is required with --vault-addr"),
        (None, _) => None,
    };
    Ok(Engines::new().with_vault(vault).with_k8s(K8sClient::in_cluster().ok()))
}

/// The injection rule set, fully defined by the JSON inject config. Absent file
/// ⇒ passthrough; malformed ⇒ startup failure, never a partial credential set.
fn build_rules(args: &Args) -> anyhow::Result<Vec<InjectionRule>> {
    if !args.inject_config.is_file() {
        return Ok(Vec::new());
    }
    let json = std::fs::read_to_string(&args.inject_config)
        .with_context(|| format!("reading {}", args.inject_config.display()))?;
    let rules = inject::load_inject_config(&json, &args.identity)
        .with_context(|| format!("loading {}", args.inject_config.display()))?;
    tracing::info!(
        "loaded {} injection rule(s) from {}",
        rules.len(),
        args.inject_config.display()
    );
    Ok(rules)
}

/// The dispatch this pod redeems for, when it is a dispatched pod.
fn dispatch_config(args: &Args) -> anyhow::Result<Option<DispatchConfig>> {
    let dispatch_id = args.dispatch_id.trim();
    if dispatch_id.is_empty() {
        return Ok(None);
    }
    let server = ServerTarget::parse(&args.dispatch_server_url)
        .context("CCTUI_DISPATCH_SERVER_URL (the pod's cctui.dev/server-url annotation)")?;
    Ok(Some(DispatchConfig {
        dispatch_id: dispatch_id.to_owned(),
        server,
        token_path: args.dispatch_token_path.clone(),
    }))
}

/// Mints the per-pod CA and writes its public cert for the worker to install.
fn build_ca(args: &Args) -> anyhow::Result<Arc<PerPodCa>> {
    let ca = Arc::new(PerPodCa::generate()?);
    write_ca_cert(&args.ca_cert_out, ca.ca_pem())
        .with_context(|| format!("writing CA cert to {}", args.ca_cert_out.display()))?;
    tracing::info!("per-pod CA minted; wrote CA cert to {}", args.ca_cert_out.display());
    Ok(ca)
}

fn build_gate(args: &Args) -> Arc<GatePolicy> {
    let policy = GatePolicy::new(Gate::parse(args.human_gate.as_deref()), &args.protected_branches);
    match policy.gate() {
        Gate::Open => {}
        Gate::Unparseable => tracing::error!(
            "human gate {:?} does not parse: every outbound connection is refused",
            args.human_gate.as_deref().unwrap_or_default()
        ),
        Gate::Declared(HumanGate::OutwardHeldForApproval) => tracing::warn!(
            "human gate outwardHeldForApproval is declared but the guard has no approval flow \
             to hold outward writes for: not enforced here"
        ),
        Gate::Declared(declared) => tracing::info!("human gate {} enforced", declared.as_str()),
    }
    Arc::new(policy)
}

/// Builds the TLS injector over the rule set. Returns `None` (passthrough)
/// when no rules are configured.
fn build_injection(
    args: &Args,
    rules: Vec<InjectionRule>,
    ca: Option<Arc<PerPodCa>>,
    gate: Arc<GatePolicy>,
) -> anyhow::Result<Option<Arc<Injector>>> {
    if rules.is_empty() {
        tracing::info!("no injection rules configured — TLS passthrough only");
        return Ok(None);
    }
    let ca = ca.ok_or_else(|| anyhow::anyhow!("injection rules need the per-pod CA"))?;
    let secrets = Arc::new(SecretSource::new(
        Box::new(build_engines(args)?),
        Duration::from_secs(args.secret_ttl_secs),
    ));
    secrets.spawn_sighup_refresh();
    tracing::info!("{} injection rule(s), secret ttl={}s", rules.len(), args.secret_ttl_secs);

    let ghapp = build_ghapp(args, &secrets)?;
    let injector = Injector::new(ca, secrets, InjectionPolicy::new(rules), ghapp)?.with_gate(gate);
    Ok(Some(Arc::new(injector)))
}

/// Builds the GitHub-App installation-token minter when both an App id
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
    let Some(template) = &args.github_app_key_secret else {
        anyhow::bail!("--github-app-key-secret is required with --github-app-id");
    };
    let key_ref = SecretRef::parse(&render_identity(template, &args.identity)?)?;
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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let rt = tokio::runtime::Runtime::new()?;
    let res = rt.block_on(async {
        if let Some(Cmd::FetchSecret { secret_ref }) = &args.command {
            return fetch_secret(&args, secret_ref).await;
        }
        run(args).await
    });
    rt.shutdown_timeout(Duration::from_secs(1));
    res
}

async fn run(args: Args) -> anyhow::Result<()> {
    let shutdown = shutdown::termination()?;
    let drain = Duration::from_secs(args.shutdown_drain_secs);

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
    let dispatch = if let Some(config) = dispatch_config(&args)? {
        Some(Arc::new(DispatchIdentity::redeem(config).await))
    } else {
        None
    };
    let https_server = dispatch.as_ref().is_some_and(|d| d.config.server.https);
    let ca = (!rules.is_empty() || https_server).then(|| build_ca(&args)).transpose()?;
    let server = dispatch
        .as_ref()
        .and_then(|d| d.machine_key().map(|key| (d.config.server.clone(), key.to_owned())))
        .map(|(target, key)| ServerInjector::new(target, key, ca.clone()))
        .transpose()?
        .map(Arc::new);
    let inject_hosts: Vec<String> = rules.iter().map(|r| r.host.clone()).collect();
    let gate = build_gate(&args);
    let injection = build_injection(&args, rules, ca, gate.clone())?;
    let private =
        Arc::new(PrivateAllow::from_env(Some(args.private_allow_file.clone()), &inject_hosts));

    let decision_log = Arc::new(cctui_guard::decision_log::DecisionLog::new(args.decision_log));
    let policy = Arc::new(PolicyManager::new(&args.policy).with_decision_log(Some(decision_log)));
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
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            if let Err(e) = health::serve(&addr, policy, dispatch).await {
                tracing::error!("health server exited: {e}");
            }
        });
    }

    match args.mode {
        Mode::Transparent => {
            let listener = TransparentListener::bind(&args.listen, policy, injection, private)
                .await?
                .with_server(server)
                .with_gate(gate);
            tracing::info!("transparent proxy listening on {}", args.listen);
            listener.serve_until(shutdown, drain).await?;
        }
        Mode::Forward => {
            if server.is_some() {
                tracing::warn!("forward mode does not inject the dispatch machine key");
            }
            let listener = ForwardListener::bind(&args.listen, policy, injection, private)
                .await?
                .with_gate(gate);
            tracing::info!("forward proxy listening on {}", args.listen);
            listener.serve_until(shutdown, drain).await?;
        }
    }

    tracing::info!("guard-proxy stopped");
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
    fn injection_defaults() {
        let args = parse(&[]);
        assert_eq!(args.private_allow_file, PathBuf::from("/var/run/guard-proxy/private-allow"));
        assert_eq!(args.secret_ttl_secs, 120);
        assert_eq!(args.inject_config, PathBuf::from("/etc/guard-proxy/inject.json"));
    }

    #[test]
    fn absent_inject_config_yields_no_rules() {
        let mut args = parse(&["--identity", "acme"]);
        args.inject_config = PathBuf::from("/nonexistent/inject.json");
        assert!(build_rules(&args).unwrap().is_empty());
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
        let mut args = parse(&["--identity", "acme"]);
        args.inject_config = path;
        let rules = build_rules(&args).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].secret,
            Some(SecretRef::parse("vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME").unwrap())
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
    fn a_pod_without_a_dispatch_id_redeems_nothing() {
        assert!(dispatch_config(&parse(&[])).unwrap().is_none());
        let args = parse(&["--dispatch-id", "d-1", "--dispatch-server-url", "http://cctui:8700"]);
        let config = dispatch_config(&args).unwrap().unwrap();
        assert_eq!(config.dispatch_id, "d-1");
        assert_eq!(config.server.port, 8700);
        assert_eq!(
            config.token_path,
            PathBuf::from("/var/run/secrets/kubernetes.io/serviceaccount/cctui-token")
        );
        let err = dispatch_config(&parse(&["--dispatch-id", "d-1"])).unwrap_err();
        assert!(format!("{err:#}").contains("CCTUI_DISPATCH_SERVER_URL"), "{err:#}");
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

    #[test]
    fn human_gate_flags_parse_and_default_open() {
        let args = parse(&[]);
        assert_eq!(build_gate(&args).gate(), Gate::Open);
        let args = parse(&["--human-gate", "draftPrOnly", "--protected-branches", "release,dev"]);
        assert_eq!(build_gate(&args).gate(), Gate::Declared(HumanGate::DraftPrOnly));
        assert_eq!(args.protected_branches, vec!["release", "dev"]);
        assert_eq!(build_gate(&parse(&["--human-gate", "bogus"])).gate(), Gate::Unparseable);
    }

    fn test_secrets() -> Arc<SecretSource> {
        Arc::new(SecretSource::new(Box::new(Engines::new()), Duration::from_mins(2)))
    }

    #[test]
    fn ghapp_inert_without_both_ids() {
        let secrets = test_secrets();
        assert!(build_ghapp(&parse(&["--github-app-id", "123"]), &secrets).unwrap().is_none());
        assert!(build_ghapp(&parse(&[]), &secrets).unwrap().is_none());
    }

    #[test]
    fn ghapp_key_ref_required_when_ids_set() {
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

        let missing = parse(&["--github-app-id", "1", "--github-app-installation-id", "2"]);
        let err = build_ghapp(&missing, &secrets).unwrap_err().to_string();
        assert!(err.contains("--github-app-key-secret"), "{err}");
    }
}
