use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;

use super::Driver;

const RESEED_CONCURRENCY: usize = 4;

#[derive(Clone, Default)]
pub(super) struct PullClock(Arc<Mutex<HashMap<String, Instant>>>);

impl PullClock {
    fn map(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn note(&self, local_id: &str) {
        self.map().insert(local_id.to_owned(), Instant::now());
    }

    /// Drops every pull older than `within`, then answers for `local_id`.
    fn fresh(&self, local_id: &str, within: Duration) -> bool {
        let mut map = self.map();
        map.retain(|_, at| at.elapsed() < within);
        map.contains_key(local_id)
    }
}

/// `pull` answers whether the session is account-bound.
pub(super) fn spawn_reseed<F, Fut>(
    targets: Vec<String>,
    clock: PullClock,
    pull: F,
) -> tokio::task::JoinHandle<usize>
where
    F: Fn(String) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<bool>> + Send + 'static,
{
    tokio::spawn(async move {
        let renewed = futures_util::stream::iter(targets)
            .map(|local_id| {
                let pulled = pull(local_id.clone());
                let clock = clock.clone();
                async move {
                    match pulled.await {
                        Ok(bound) => {
                            clock.note(&local_id);
                            bound
                        }
                        Err(err) => {
                            tracing::debug!(
                                %local_id, %err,
                                "gateway-env re-seed pull failed (will retry)"
                            );
                            false
                        }
                    }
                }
            })
            .buffer_unordered(RESEED_CONCURRENCY)
            .fold(0, |renewed, bound| std::future::ready(renewed + usize::from(bound)))
            .await;
        if renewed > 0 {
            tracing::info!(renewed, "re-seeded gateway env for live account-bound workers");
        }
        renewed
    })
}

impl Driver {
    /// Skips workers pulled within `interval`: their launch already re-minted the token.
    pub(super) fn reseed_targets(&self, interval: Duration) -> Vec<String> {
        self.short_by_session
            .entries()
            .into_iter()
            .filter(|(local_id, short)| {
                self.roster.contains(short) && !self.pulls.fresh(local_id, interval)
            })
            .map(|(local_id, _)| local_id)
            .collect()
    }

    /// The token string is stable, so a re-pull only bumps its expiry.
    pub(super) fn reseed_gateway_env(&mut self, interval: Duration) {
        let (Some(server), Some(mk)) = (self.server.clone(), self.machine_key.clone()) else {
            return;
        };
        if self.reseeding.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let targets = self.reseed_targets(interval);
        if targets.is_empty() {
            return;
        }
        self.reseeding = Some(spawn_reseed(targets, self.pulls.clone(), move |local_id| {
            let (server, mk) = (server.clone(), mk.clone());
            async move { anyhow::Ok(server.gateway_env(&mk, &local_id).await?.account_bound) }
        }));
    }
}
