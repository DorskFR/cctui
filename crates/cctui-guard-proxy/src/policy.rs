//! Egress allow-list policy: JSON file, fail-closed, hot-reloaded by mtime poll.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use cctui_guard::decision_log::DecisionLog;
use serde::Deserialize;

/// The on-disk policy document:
/// `{ "allowed_hosts": ["host:port", …], "default": "deny" }`.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub default: String,
}

/// Holds the active policy and the path it was loaded from. Hot-reloads when the
/// file's mtime changes. A `None` config means deny-all (fail closed): a missing,
/// unreadable, or invalid policy never opens the gate.
pub struct PolicyManager {
    config: RwLock<Option<PolicyConfig>>,
    path: PathBuf,
    last_mtime: RwLock<Option<SystemTime>>,
    decision_log: Option<Arc<DecisionLog>>,
}

impl PolicyManager {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            config: RwLock::new(None),
            path: path.into(),
            last_mtime: RwLock::new(None),
            decision_log: None,
        }
    }

    /// Attach a shared decision log so each egress verdict is appended as a
    /// `network` JSON line (best-effort). The guard writes the workflow steps to
    /// the same file, letting the end-of-run report attribute a denied host to
    /// the step that was active.
    #[must_use]
    pub fn with_decision_log(mut self, log: Option<Arc<DecisionLog>>) -> Self {
        self.decision_log = log.filter(|l| l.is_enabled());
        self
    }

    /// Record an egress verdict on `host_port` to the decision log, if attached.
    pub fn record(&self, host_port: &str, allowed: bool, rule: &str) {
        if let Some(log) = &self.decision_log {
            log.network(host_port, allowed, rule);
        }
    }

    /// Loads (or reloads) the policy from disk. A missing file clears the policy
    /// (deny-all). An unreadable or unparsable file returns an error and leaves
    /// the previous policy untouched — callers should treat that as fail-closed
    /// by never having loaded a permissive policy in the first place.
    pub fn load(&self) -> anyhow::Result<()> {
        let data = match std::fs::read(&self.path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File doesn't exist yet: clear policy (defaults to deny-all).
                *self.config.write().expect("policy lock poisoned") = None;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };

        let config: PolicyConfig = serde_json::from_slice(&data)?;
        *self.config.write().expect("policy lock poisoned") = Some(config);
        Ok(())
    }

    /// Returns true if `host_port` (e.g. `example.com:443`) is permitted.
    ///
    /// Fail-closed: with no policy loaded, everything is denied.
    #[must_use]
    pub fn is_allowed(&self, host_port: &str) -> bool {
        let guard = self.config.read().expect("policy lock poisoned");
        guard.as_ref().is_some_and(|config| {
            config.allowed_hosts.iter().any(|allowed| matches_pattern(host_port, allowed))
                || config.default == "allow"
        })
    }

    /// True if a policy is currently loaded (used by the health endpoint).
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.config.read().expect("policy lock poisoned").is_some()
    }

    /// Polls the policy file's mtime forever, reloading on change. The directory
    /// approach the Go impl used (fsnotify) is replaced by an mtime poll, which
    /// the ticket explicitly permits and which survives atomic-rename writes.
    pub async fn watch(self: std::sync::Arc<Self>, interval: Duration) {
        loop {
            tokio::time::sleep(interval).await;
            let mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
            let changed = {
                let last = self.last_mtime.read().expect("mtime lock poisoned");
                *last != mtime
            };
            if changed {
                *self.last_mtime.write().expect("mtime lock poisoned") = mtime;
                match self.load() {
                    Ok(()) => tracing::info!("policy reloaded successfully"),
                    Err(e) => tracing::warn!("failed to reload policy: {e}"),
                }
            }
        }
    }
}

/// Matches `host_port` against an allow-list pattern. An exact match wins; a
/// `host:*` pattern matches the host on any port — port-only wildcard, no host
/// globbing.
fn matches_pattern(host_port: &str, pattern: &str) -> bool {
    if pattern == host_port {
        return true;
    }

    if let Some(pattern_host) = pattern.strip_suffix(":*") {
        return get_host(host_port) == pattern_host;
    }

    false
}

/// Extracts the host part from a `host:port`, handling bracketed IPv6 literals
/// like `[::1]:8080` via `rfind`.
fn get_host(host_port: &str) -> &str {
    if host_port.starts_with('[') {
        if let Some(idx) = host_port.rfind("]:") {
            return &host_port[..=idx]; // include the closing ']'
        }
        return host_port;
    }

    if let Some(idx) = host_port.rfind(':') {
        return &host_port[..idx];
    }

    host_port
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_policy(json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn policy_load() {
        let (_dir, path) = write_policy(
            r#"{"allowed_hosts": ["example.com:443", "api.example.com:80"], "default": "deny"}"#,
        );
        let pm = PolicyManager::new(&path);
        pm.load().unwrap();

        assert!(pm.is_allowed("example.com:443"));
        assert!(pm.is_allowed("api.example.com:80"));
        assert!(!pm.is_allowed("example.com:80"));
        assert!(!pm.is_allowed("other.example.com:443"));
    }

    #[test]
    fn policy_default_allow() {
        let (_dir, path) =
            write_policy(r#"{"allowed_hosts": ["blocked.example.com:443"], "default": "allow"}"#);
        let pm = PolicyManager::new(&path);
        pm.load().unwrap();

        assert!(pm.is_allowed("example.com:443"));
        assert!(pm.is_allowed("random.example.com:80"));
    }

    #[test]
    fn policy_deny_all_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let pm = PolicyManager::new(&path);
        // Don't create the file: deny-all when no policy is loaded.
        pm.load().unwrap();

        assert!(!pm.is_allowed("example.com:443"));
        assert!(!pm.is_allowed("api.example.com:80"));
        assert!(!pm.is_loaded());
    }

    #[test]
    fn policy_wildcard_port() {
        let (_dir, path) = write_policy(
            r#"{"allowed_hosts": ["example.com:*", "api.example.com:443"], "default": "deny"}"#,
        );
        let pm = PolicyManager::new(&path);
        pm.load().unwrap();

        assert!(pm.is_allowed("example.com:443"));
        assert!(pm.is_allowed("example.com:80"));
        assert!(pm.is_allowed("example.com:8080"));

        assert!(pm.is_allowed("api.example.com:443"));
        assert!(!pm.is_allowed("api.example.com:80"));
    }

    #[test]
    fn policy_fail_closed_on_invalid_json() {
        let (_dir, path) = write_policy("{ this is not valid json ]");
        let pm = PolicyManager::new(&path);
        // load() errors and leaves config as the initial None: deny-all.
        assert!(pm.load().is_err());
        assert!(!pm.is_allowed("example.com:443"));
        assert!(!pm.is_loaded());
    }

    #[test]
    fn policy_name_matching() {
        // IP fallback (no SNI/Host recovered) must NOT match a hostname allow-list.
        let (_dir, path) =
            write_policy(r#"{"allowed_hosts": ["api.example.com:443"], "default": "deny"}"#);
        let pm = PolicyManager::new(&path);
        pm.load().unwrap();

        assert!(pm.is_allowed("api.example.com:443"));
        assert!(!pm.is_allowed("evil.example.com:443"));
        assert!(!pm.is_allowed("203.0.113.4:443"));
    }

    #[test]
    fn records_egress_verdicts_to_decision_log() {
        let (_dir, path) =
            write_policy(r#"{"allowed_hosts": ["ok.example.com:443"], "default": "deny"}"#);
        let log_dir = tempfile::tempdir().unwrap();
        let log_path = log_dir.path().join("decisions.jsonl");
        let log = std::sync::Arc::new(DecisionLog::new(Some(log_path.clone())));
        let pm = PolicyManager::new(&path).with_decision_log(Some(log));
        pm.load().unwrap();

        pm.record("ok.example.com:443", pm.is_allowed("ok.example.com:443"), "");
        pm.record(
            "blocked.example.com:443",
            pm.is_allowed("blocked.example.com:443"),
            "not in allow-list",
        );

        let records =
            cctui_guard::decision_log::parse_log(&std::fs::read_to_string(&log_path).unwrap());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].verdict, "allow");
        assert_eq!(records[0].target, "ok.example.com:443");
        assert_eq!(records[1].verdict, "deny");
        assert_eq!(records[1].rule, "not in allow-list");
    }

    #[test]
    fn decision_log_off_when_disabled() {
        let (_dir, path) = write_policy(r#"{"allowed_hosts": [], "default": "deny"}"#);
        let pm = PolicyManager::new(&path)
            .with_decision_log(Some(std::sync::Arc::new(DecisionLog::new(None))));
        // A disabled log is dropped, so record is a pure no-op.
        pm.record("x.example.com:443", false, "not in allow-list");
    }

    #[test]
    fn get_host_handles_ipv6() {
        assert_eq!(get_host("[::1]:8080"), "[::1]");
        assert_eq!(get_host("example.com:443"), "example.com");
        assert_eq!(get_host("example.com"), "example.com");
    }

    #[tokio::test]
    async fn policy_hot_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        std::fs::write(&path, r#"{"allowed_hosts": ["example.com:443"], "default": "deny"}"#)
            .unwrap();

        let pm = std::sync::Arc::new(PolicyManager::new(&path));
        pm.load().unwrap();
        assert!(pm.is_allowed("example.com:443"));

        let watcher = pm.clone();
        let handle = tokio::spawn(async move {
            watcher.watch(Duration::from_millis(50)).await;
        });

        // Sleep past the initial mtime snapshot, then rewrite with a new policy.
        tokio::time::sleep(Duration::from_millis(120)).await;
        std::fs::write(
            &path,
            r#"{"allowed_hosts": ["newhost.example.com:80"], "default": "deny"}"#,
        )
        .unwrap();

        // Allow the poller to observe the mtime change.
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert!(!pm.is_allowed("example.com:443"));
        assert!(pm.is_allowed("newhost.example.com:80"));
        handle.abort();
    }
}
