use cctui_dispatcher_core::cli::{Backend, Enrolled};
use clap::Args;
use tokio_util::sync::CancellationToken;

use crate::cli::RealCli;
use crate::config::Config;
use crate::spawn::Spawner;

pub struct Apple;

#[derive(Args, Debug)]
pub struct EnrollArgs {
    /// Worker OCI image to boot on dispatch.
    #[arg(long)]
    pub image: String,
    /// Container network to attach spawned micro-VMs to.
    #[arg(long)]
    pub network: Option<String>,
    /// Path to the `container` binary (defaults to `container` on `PATH`).
    #[arg(long)]
    pub container_bin: Option<String>,
    /// Extra bind mount(s) for spawned containers (`host:guest[:ro]`).
    #[arg(long)]
    pub mount: Vec<String>,
    /// Optional repo mount (`host:guest`) — shallow-pulled at boot.
    #[arg(long)]
    pub repo_mount: Option<String>,
}

impl Backend for Apple {
    const NAME: &'static str = "cctui-dispatcher-apple";
    const KIND: &'static str = "apple";
    const ABOUT: &'static str = "Standalone Apple `container` dispatcher: enrolls to an account and boots worker micro-VMs on dispatch";

    type Config = Config;
    type EnrollArgs = EnrollArgs;
    type Spawner = Spawner<RealCli>;

    fn config(args: EnrollArgs, enrolled: Enrolled) -> Config {
        Config {
            server_url: enrolled.server_url,
            dispatcher_key: enrolled.dispatcher_key,
            dispatcher_id: Some(enrolled.dispatcher_id),
            image: args.image,
            worker_cctui_url: enrolled.worker_cctui_url,
            network: args.network,
            container_bin: args.container_bin,
            mounts: args.mount,
            repo_mount: args.repo_mount,
            secret_mount_path: None,
            secret_dir: None,
            secret_via_env: false,
        }
    }

    async fn spawner(
        cfg: &Config,
        _shutdown: &CancellationToken,
    ) -> anyhow::Result<Spawner<RealCli>> {
        use cctui_dispatcher_core::DispatcherConfig;
        Ok(Spawner::new(
            RealCli::new(cfg.container_bin().to_owned()),
            cfg.image.clone(),
            cfg.network.clone(),
            cfg.worker_url().to_owned(),
            cfg.mounts.clone(),
            cfg.repo_mount.clone(),
            cfg.secret_mount_path().to_owned(),
            cfg.secret_dir(),
            cfg.secret_via_env,
        ))
    }

    fn status_lines(cfg: &Config) -> Vec<(&'static str, String)> {
        vec![("image", cfg.image.clone()), ("container binary", cfg.container_bin().to_owned())]
    }
}

#[cfg(test)]
mod tests {
    use cctui_dispatcher_core::cli::{Cmd, try_parse_from};

    use super::*;

    #[test]
    fn enroll_flags_parse() {
        let cli = try_parse_from::<Apple, _, _>([
            "cctui-dispatcher-apple",
            "enroll",
            "--server-url",
            "https://s.example.test",
            "--token",
            "tok",
            "--name",
            "mac",
            "--image",
            "img:1",
            "--worker-cctui-url",
            "http://w.example.test",
            "--network",
            "net",
            "--container-bin",
            "/opt/apple/bin/container",
            "--mount",
            "/host/cache:/cache:ro",
            "--repo-mount",
            "/host/repo:/workspace/repo",
            "--account",
            "acct",
            "--provider",
            "anthropic",
        ])
        .unwrap();
        let Cmd::Enroll { common, backend } = cli.cmd else { panic!("expected enroll") };
        assert_eq!(common.server_url, "https://s.example.test");
        assert_eq!(common.token, "tok");
        assert_eq!(common.name, "mac");
        assert_eq!(common.worker_cctui_url.as_deref(), Some("http://w.example.test"));
        assert_eq!(common.account.as_deref(), Some("acct"));
        assert_eq!(common.provider.as_deref(), Some("anthropic"));
        assert_eq!(backend.image, "img:1");
        assert_eq!(backend.network.as_deref(), Some("net"));
        assert_eq!(backend.container_bin.as_deref(), Some("/opt/apple/bin/container"));
        assert_eq!(backend.mount, vec!["/host/cache:/cache:ro".to_owned()]);
        assert_eq!(backend.repo_mount.as_deref(), Some("/host/repo:/workspace/repo"));
    }

    #[test]
    fn enroll_builds_config_with_secret_defaults() {
        let cli = try_parse_from::<Apple, _, _>([
            "cctui-dispatcher-apple",
            "enroll",
            "--server-url",
            "u",
            "--token",
            "t",
            "--name",
            "n",
            "--image",
            "i",
        ])
        .unwrap();
        let Cmd::Enroll { common, backend } = cli.cmd else { panic!("expected enroll") };
        let cfg = Apple::config(
            backend,
            Enrolled {
                server_url: common.server_url,
                dispatcher_key: "k".to_owned(),
                dispatcher_id: uuid::Uuid::nil(),
                worker_cctui_url: common.worker_cctui_url,
            },
        );
        assert_eq!(cfg.image, "i");
        assert_eq!(cfg.dispatcher_id, Some(uuid::Uuid::nil()));
        assert!(!cfg.secret_via_env);
        assert!(cfg.secret_dir.is_none());
    }
}
