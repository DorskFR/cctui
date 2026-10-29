//! Operational tasks owned by the standalone host's runtime.
use crate::host::Host;
use std::time::Duration;

/// Dropping the runtime stops its tasks, including in-flight sweeps.
pub struct BackgroundTasks {
    webhook: tokio::task::JoinHandle<()>,
    workloads: tokio::task::JoinHandle<()>,
    launches: tokio::task::JoinHandle<()>,
    dispatchers: tokio::task::JoinHandle<()>,
    retirement: tokio::task::JoinHandle<()>,
}
impl BackgroundTasks {
    #[must_use]
    pub fn start(host: Host) -> Self {
        let webhook_host = host.clone();
        let webhook = periodic(move || {
            let host = webhook_host.clone();
            async move { cctui_server_core::webhook::sweep(&host).await }
        });
        let workload_host = host.clone();
        let workloads = periodic(move || {
            let host = workload_host.clone();
            async move { cctui_server_core::dispatch_liveness::sweep(&host).await }
        });
        let launch_host = host.clone();
        let launches = periodic(move || {
            let host = launch_host.clone();
            async move {
                if let Err(error) = crate::termination::sweep(&host).await {
                    tracing::error!(%error, "launch deadline reconciliation failed");
                }
            }
        });
        let dispatcher_host = host.clone();
        let dispatchers = periodic(move || {
            let host = dispatcher_host.clone();
            async move { cctui_server_core::targets::dispatcher_liveness::sweep(&host.core).await }
        });
        let retirement = periodic(move || {
            let host = host.clone();
            async move {
                cctui_server_core::worker_retirement::reap_ephemeral_machines(
                    &host.core,
                    host.ephemeral_machine_ttl_secs,
                )
                .await;
                cctui_server_core::worker_retirement::reap_ephemeral_keys(&host.core).await;
            }
        });
        Self { webhook, workloads, launches, dispatchers, retirement }
    }
}
fn periodic<F, Fut>(mut job: F) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            job().await;
        }
    })
}
impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        self.webhook.abort();
        self.workloads.abort();
        self.launches.abort();
        self.dispatchers.abort();
        self.retirement.abort();
    }
}
