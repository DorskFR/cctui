//! Operational tasks owned by the standalone host's runtime.
use crate::host::Host;
use std::time::Duration;

/// Dropping the runtime stops its tasks, including an in-flight sweep.
pub struct BackgroundTasks {
    webhook: tokio::task::JoinHandle<()>,
}
impl BackgroundTasks {
    #[must_use]
    pub fn start(host: Host) -> Self {
        let webhook = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                cctui_server_core::webhook::sweep(&host).await;
            }
        });
        Self { webhook }
    }
}
impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        self.webhook.abort();
    }
}
