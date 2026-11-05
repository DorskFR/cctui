//! Config values resolved at startup that re-resolve without a restart: on
//! SIGHUP (every ref) and on an optional per-ref TTL. A refresh swaps the value
//! atomically; a failed one keeps the previous value and logs the variable name
//! and the ref, never a value.
//!
//! TTLs come from the environment, off unless set: `CCTUI_SECRETS_TTL` (seconds)
//! for every ref, `CCTUI_SECRETS_TTL_<NAME>` for one (`<NAME>` is the variable
//! name uppercased, non-alphanumerics as `_`). `0` turns a TTL off.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{OnceCell, watch};
use tokio::time::Instant;

use crate::{Engines, RefResolver, SecretRef, SecretValue};

pub const TTL_ENV: &str = "CCTUI_SECRETS_TTL";
pub const TTL_ENV_PREFIX: &str = "CCTUI_SECRETS_TTL_";

/// Upper bound on how late a TTL refresh may land.
const MAX_TICK: Duration = Duration::from_secs(10);

/// One named config value. Cheap to clone; every clone sees a refresh.
#[derive(Clone)]
pub struct Secret {
    inner: Arc<Entry>,
}

struct Entry {
    name: String,
    reference: Option<SecretRef>,
    ttl: Option<Duration>,
    value: watch::Sender<Arc<str>>,
    refreshed_at: Mutex<Instant>,
}

impl Secret {
    fn new(
        name: &str,
        reference: Option<SecretRef>,
        ttl: Option<Duration>,
        value: Arc<str>,
    ) -> Self {
        Self {
            inner: Arc::new(Entry {
                name: name.to_owned(),
                reference,
                ttl,
                value: watch::Sender::new(value),
                refreshed_at: Mutex::new(Instant::now()),
            }),
        }
    }

    /// A value that never changes, for literals and tests.
    #[must_use]
    pub fn fixed(name: &str, value: &str) -> Self {
        Self::new(name, None, None, Arc::from(value))
    }

    /// The current value. Read it at the point of use so a refresh lands on
    /// the next use.
    #[must_use]
    pub fn get(&self) -> Arc<str> {
        self.inner.value.borrow().clone()
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The ref this value resolves from; `None` for a literal.
    #[must_use]
    pub fn reference(&self) -> Option<&SecretRef> {
        self.inner.reference.as_ref()
    }

    /// Fires on every change of the value.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Arc<str>> {
        self.inner.value.subscribe()
    }

    fn due(&self, now: Instant) -> bool {
        let Some(ttl) = self.inner.ttl else { return false };
        let last = *self.inner.refreshed_at.lock().unwrap_or_else(PoisonError::into_inner);
        now.duration_since(last) >= ttl
    }

    fn mark_refreshed(&self) {
        *self.inner.refreshed_at.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("name", &self.inner.name)
            .field("reference", &self.inner.reference.as_ref().map(ToString::to_string))
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Counts from one reload pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReloadReport {
    pub changed: usize,
    pub unchanged: usize,
    pub failed: usize,
}

pub type TtlLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The reloadable values of one process, all resolved through one resolver.
pub struct SecretRegistry {
    resolver: OnceCell<Arc<dyn RefResolver>>,
    ttl_lookup: TtlLookup,
    entries: Mutex<Vec<Secret>>,
}

impl fmt::Debug for SecretRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretRegistry").field("entries", &self.snapshot()).finish_non_exhaustive()
    }
}

impl SecretRegistry {
    /// Resolves through `resolver`; TTLs from the process environment.
    #[must_use]
    pub fn new(resolver: Arc<dyn RefResolver>) -> Self {
        Self {
            resolver: OnceCell::new_with(Some(resolver)),
            ttl_lookup: Arc::new(|k: &str| std::env::var(k).ok()),
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Resolves through [`Engines::from_env`], built only when a ref needs it.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            resolver: OnceCell::new(),
            ttl_lookup: Arc::new(|k: &str| std::env::var(k).ok()),
            entries: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn with_ttl_lookup(mut self, lookup: TtlLookup) -> Self {
        self.ttl_lookup = lookup;
        self
    }

    async fn resolver(&self) -> anyhow::Result<&Arc<dyn RefResolver>> {
        self.resolver
            .get_or_try_init(|| async {
                Engines::from_env().map(|e| Arc::new(e) as Arc<dyn RefResolver>)
            })
            .await
    }

    async fn fetch(&self, name: &str, r: &SecretRef) -> anyhow::Result<String> {
        let resolver = self.resolver().await.map_err(|e| anyhow::anyhow!("{name}: {e:#}"))?;
        crate::resolve_ref(resolver.as_ref(), r).await.map_err(|e| anyhow::anyhow!("{name}: {e}"))
    }

    /// Resolves `raw` once without tracking it. Errors name `name` and the
    /// ref, never the value.
    pub async fn resolve(&self, name: &str, raw: &str) -> anyhow::Result<String> {
        match SecretValue::parse(raw).map_err(|e| anyhow::anyhow!("{name}: {e}"))? {
            SecretValue::Literal(value) => Ok(value.to_owned()),
            SecretValue::Ref(r) => self.fetch(name, &r).await,
        }
    }

    /// Resolves `raw` and tracks it for reloads. A literal is returned as a
    /// fixed value.
    pub async fn register(&self, name: &str, raw: &str) -> anyhow::Result<Secret> {
        let r = match SecretValue::parse(raw).map_err(|e| anyhow::anyhow!("{name}: {e}"))? {
            SecretValue::Literal(value) => return Ok(Secret::fixed(name, value)),
            SecretValue::Ref(r) => r,
        };
        let ttl = self.ttl_for(name)?;
        let value = self.fetch(name, &r).await?;
        let secret = Secret::new(name, Some(r), ttl, Arc::from(value));
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).push(secret.clone());
        Ok(secret)
    }

    /// [`Self::register`] for each of `names` that `get` returns, keyed by name.
    pub async fn register_env(
        &self,
        names: &[&str],
        get: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<HashMap<String, Secret>> {
        let mut out = HashMap::with_capacity(names.len());
        for name in names {
            if let Some(raw) = get(name) {
                out.insert((*name).to_owned(), self.register(name, &raw).await?);
            }
        }
        Ok(out)
    }

    /// The tracked value registered as `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Secret> {
        self.snapshot().into_iter().find(|s| s.name() == name)
    }

    fn snapshot(&self) -> Vec<Secret> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn ttl_for(&self, name: &str) -> anyhow::Result<Option<Duration>> {
        let own = format!("{TTL_ENV_PREFIX}{}", env_fragment(name));
        let raw = (self.ttl_lookup)(&own)
            .map(|v| (own, v))
            .or_else(|| (self.ttl_lookup)(TTL_ENV).map(|v| (TTL_ENV.to_owned(), v)));
        let Some((var, raw)) = raw.filter(|(_, v)| !v.trim().is_empty()) else { return Ok(None) };
        let secs: u64 = raw
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("{var} must be a whole number of seconds"))?;
        Ok((secs > 0).then_some(Duration::from_secs(secs)))
    }

    /// Re-resolves every tracked ref.
    pub async fn reload(&self) -> ReloadReport {
        self.refresh(None).await
    }

    /// Re-resolves the refs whose TTL has elapsed.
    pub async fn reload_due(&self) -> ReloadReport {
        self.refresh(Some(Instant::now())).await
    }

    /// Every tracked ref, or only those due at `due_at`.
    async fn refresh(&self, due_at: Option<Instant>) -> ReloadReport {
        let mut report = ReloadReport::default();
        let picked: Vec<Secret> =
            self.snapshot().into_iter().filter(|s| due_at.is_none_or(|now| s.due(now))).collect();
        for secret in picked {
            let Some(r) = secret.reference() else { continue };
            let outcome = self.fetch(secret.name(), r).await;
            secret.mark_refreshed();
            match outcome {
                Ok(value) => {
                    let changed = secret.inner.value.send_if_modified(|current| {
                        if **current == *value {
                            return false;
                        }
                        *current = Arc::from(value);
                        true
                    });
                    if changed {
                        report.changed += 1;
                        tracing::info!(
                            secret = secret.name(),
                            reference = %r,
                            "secret ref rotated"
                        );
                    } else {
                        report.unchanged += 1;
                    }
                }
                Err(e) => {
                    report.failed += 1;
                    tracing::warn!(
                        secret = secret.name(),
                        reference = %r,
                        "secret ref refresh failed, keeping the previous value: {e:#}"
                    );
                }
            }
        }
        report
    }

    /// How often the TTL loop wakes; `None` when no tracked ref has a TTL.
    fn tick(&self) -> Option<Duration> {
        self.snapshot()
            .iter()
            .filter_map(|s| s.inner.ttl)
            .min()
            .map(|ttl| ttl.clamp(Duration::from_secs(1), MAX_TICK))
    }

    /// Reloads on SIGHUP and on TTL until the runtime stops. Register every
    /// value first: the TTL tick is fixed when this starts.
    pub fn spawn_reloader(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let registry = Arc::clone(self);
        let mut hup = Sighup::new();
        tokio::spawn(async move {
            let mut ticker = registry.tick().map(|tick| {
                let mut t = tokio::time::interval_at(Instant::now() + tick, tick);
                t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                t
            });
            loop {
                tokio::select! {
                    () = hup.recv() => {
                        let report = registry.reload().await;
                        tracing::info!(?report, "SIGHUP: secret refs reloaded");
                    }
                    () = next_tick(ticker.as_mut()) => {
                        registry.reload_due().await;
                    }
                }
            }
        })
    }
}

async fn next_tick(ticker: Option<&mut tokio::time::Interval>) {
    match ticker {
        Some(t) => {
            t.tick().await;
        }
        None => std::future::pending().await,
    }
}

fn env_fragment(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}

/// SIGHUP as a stream; never fires where signals are unavailable.
pub struct Sighup {
    #[cfg(unix)]
    signal: Option<tokio::signal::unix::Signal>,
}

impl Sighup {
    /// Installs the handler, replacing the default terminate-on-SIGHUP.
    /// Needs a tokio runtime.
    #[must_use]
    pub fn new() -> Self {
        #[cfg(unix)]
        {
            let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .map_err(|e| tracing::warn!("SIGHUP handler unavailable, secret reload off: {e}"))
                .ok();
            Self { signal }
        }
        #[cfg(not(unix))]
        Self {}
    }

    pub async fn recv(&mut self) {
        #[cfg(unix)]
        if let Some(signal) = &mut self.signal
            && signal.recv().await.is_some()
        {
            return;
        }
        std::future::pending::<()>().await;
    }
}

impl Default for Sighup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "bao", feature = "k8s", feature = "aws-sm"))]
#[path = "reload_tests.rs"]
mod tests;
