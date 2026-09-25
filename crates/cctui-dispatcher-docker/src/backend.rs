use cctui_dispatcher_core::cli::{Backend, Enrolled};
use clap::Args;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::spawn::Spawner;

pub struct Docker;

#[derive(Args, Debug)]
pub struct EnrollArgs {
    /// Worker image to spawn on dispatch.
    #[arg(long)]
    pub image: String,
    /// Docker network to attach spawned containers to.
    #[arg(long)]
    pub network: Option<String>,
    /// Docker host/socket (defaults to the local socket).
    #[arg(long)]
    pub docker_host: Option<String>,
    /// Bind mount(s) for spawned containers (`/host:/container[:ro]`).
    #[arg(long)]
    pub mount: Vec<String>,
}

impl Backend for Docker {
    const NAME: &'static str = "cctui-dispatcher-docker";
    const KIND: &'static str = "docker";
    const ABOUT: &'static str =
        "Standalone docker dispatcher: enrolls to an account and spawns worker containers on dispatch";

    type Config = Config;
    type EnrollArgs = EnrollArgs;
    type Spawner = Spawner;

    fn config(args: EnrollArgs, enrolled: Enrolled) -> Config {
        Config {
            server_url: enrolled.server_url,
            dispatcher_key: enrolled.dispatcher_key,
            dispatcher_id: Some(enrolled.dispatcher_id),
            image: args.image,
            worker_cctui_url: enrolled.worker_cctui_url,
            network: args.network,
            docker_host: args.docker_host,
            mounts: args.mount,
        }
    }

    async fn spawner(cfg: &Config, _shutdown: &CancellationToken) -> anyhow::Result<Spawner> {
        use cctui_dispatcher_core::DispatcherConfig;
        Spawner::connect(
            cfg.docker_host.as_deref(),
            cfg.image.clone(),
            cfg.network.clone(),
            cfg.worker_url().to_owned(),
            cfg.mounts.clone(),
        )
        .await
    }

    fn status_lines(cfg: &Config) -> Vec<(&'static str, String)> {
        vec![("image", cfg.image.clone())]
    }
}

#[cfg(test)]
mod tests {
    use cctui_dispatcher_core::cli::{Cmd, try_parse_from};

    use super::*;

    #[test]
    fn enroll_flags_parse() {
        let cli = try_parse_from::<Docker, _, _>([
            "cctui-dispatcher-docker",
            "--config",
            "/etc/cctui/dispatcher.toml",
            "enroll",
            "--server-url",
            "https://s.example.test",
            "--token",
            "tok",
            "--name",
            "box",
            "--image",
            "img:1",
            "--worker-cctui-url",
            "http://w.example.test",
            "--network",
            "net",
            "--docker-host",
            "unix:///var/run/docker.sock",
            "--mount",
            "/a:/a",
            "--mount",
            "/b:/b:ro",
            "--account",
            "acct",
            "--provider",
            "anthropic",
        ])
        .unwrap();
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/etc/cctui/dispatcher.toml")));
        let Cmd::Enroll { common, backend } = cli.cmd else { panic!("expected enroll") };
        assert_eq!(common.server_url, "https://s.example.test");
        assert_eq!(common.token, "tok");
        assert_eq!(common.name, "box");
        assert_eq!(common.worker_cctui_url.as_deref(), Some("http://w.example.test"));
        assert_eq!(common.account.as_deref(), Some("acct"));
        assert_eq!(common.provider.as_deref(), Some("anthropic"));
        assert_eq!(backend.image, "img:1");
        assert_eq!(backend.network.as_deref(), Some("net"));
        assert_eq!(backend.docker_host.as_deref(), Some("unix:///var/run/docker.sock"));
        assert_eq!(backend.mount, vec!["/a:/a".to_owned(), "/b:/b:ro".to_owned()]);
    }

    #[test]
    fn minimal_enroll_run_and_status_parse() {
        let cli = try_parse_from::<Docker, _, _>([
            "cctui-dispatcher-docker",
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
        let Cmd::Enroll { backend, .. } = cli.cmd else { panic!("expected enroll") };
        assert!(backend.mount.is_empty());
        assert!(matches!(
            try_parse_from::<Docker, _, _>(["cctui-dispatcher-docker", "run"]).unwrap().cmd,
            Cmd::Run
        ));
        assert!(matches!(
            try_parse_from::<Docker, _, _>(["cctui-dispatcher-docker", "status"]).unwrap().cmd,
            Cmd::Status
        ));
    }

    #[test]
    fn enroll_without_image_is_rejected() {
        assert!(
            try_parse_from::<Docker, _, _>([
                "cctui-dispatcher-docker",
                "enroll",
                "--server-url",
                "u",
                "--token",
                "t",
                "--name",
                "n",
            ])
            .is_err()
        );
    }

    #[test]
    fn command_carries_the_binary_name() {
        assert_eq!(cctui_dispatcher_core::cli::command::<Docker>().get_name(), Docker::NAME);
    }
}
