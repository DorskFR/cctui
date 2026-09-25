use cctui_dispatcher_core::cli::{Backend, Enrolled};
use clap::Args;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::spawn::Spawner;

pub struct Kube;

#[derive(Args, Debug)]
pub struct EnrollArgs {
    /// Namespace the worker Job + its `WorkerProfile` resources live in.
    #[arg(long)]
    pub namespace: String,
    /// `WorkerProfile` instantiated when a dispatch selects none by name.
    #[arg(long)]
    pub default_profile: String,
}

impl Backend for Kube {
    const NAME: &'static str = "cctui-dispatcher-kube";
    const KIND: &'static str = "kubernetes";
    const ABOUT: &'static str = "Standalone kubernetes dispatcher: enrolls to an account and spawns worker Jobs in-cluster on dispatch";

    type Config = Config;
    type EnrollArgs = EnrollArgs;
    type Spawner = Spawner;

    fn config(args: EnrollArgs, enrolled: Enrolled) -> Config {
        Config {
            server_url: enrolled.server_url,
            dispatcher_key: enrolled.dispatcher_key,
            dispatcher_id: Some(enrolled.dispatcher_id),
            namespace: args.namespace,
            default_profile: args.default_profile,
            worker_cctui_url: enrolled.worker_cctui_url,
        }
    }

    async fn spawner(cfg: &Config, shutdown: &CancellationToken) -> anyhow::Result<Spawner> {
        use cctui_dispatcher_core::DispatcherConfig;
        let spawner = Spawner::connect(
            cfg.namespace.clone(),
            cfg.default_profile.clone(),
            cfg.worker_url().to_owned(),
        )
        .await?;
        tokio::spawn(spawner.clone().run_queue_reconciler(shutdown.clone()));
        Ok(spawner)
    }

    fn status_lines(cfg: &Config) -> Vec<(&'static str, String)> {
        vec![("namespace", cfg.namespace.clone()), ("default_profile", cfg.default_profile.clone())]
    }
}

#[cfg(test)]
mod tests {
    use cctui_dispatcher_core::cli::{Cmd, try_parse_from};

    use super::*;

    #[test]
    fn enroll_flags_parse() {
        let cli = try_parse_from::<Kube, _, _>([
            "cctui-dispatcher-kube",
            "enroll",
            "--server-url",
            "https://s.example.test",
            "--token",
            "tok",
            "--name",
            "cluster",
            "--namespace",
            "cctui",
            "--default-profile",
            "default",
            "--worker-cctui-url",
            "http://cctui.cctui.svc:8700",
            "--account",
            "acct",
            "--provider",
            "openai",
        ])
        .unwrap();
        let Cmd::Enroll { common, backend } = cli.cmd else { panic!("expected enroll") };
        assert_eq!(common.server_url, "https://s.example.test");
        assert_eq!(common.token, "tok");
        assert_eq!(common.name, "cluster");
        assert_eq!(common.worker_cctui_url.as_deref(), Some("http://cctui.cctui.svc:8700"));
        assert_eq!(common.account.as_deref(), Some("acct"));
        assert_eq!(common.provider.as_deref(), Some("openai"));
        assert_eq!(backend.namespace, "cctui");
        assert_eq!(backend.default_profile, "default");
    }

    #[test]
    fn run_accepts_config_flag() {
        let cli = try_parse_from::<Kube, _, _>([
            "cctui-dispatcher-kube",
            "--config",
            "/config/dispatcher.toml",
            "run",
        ])
        .unwrap();
        assert!(matches!(cli.cmd, Cmd::Run));
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/config/dispatcher.toml")));
    }

    #[test]
    fn enroll_without_default_profile_is_rejected() {
        assert!(
            try_parse_from::<Kube, _, _>([
                "cctui-dispatcher-kube",
                "enroll",
                "--server-url",
                "u",
                "--token",
                "t",
                "--name",
                "n",
                "--namespace",
                "ns",
            ])
            .is_err()
        );
    }
}
