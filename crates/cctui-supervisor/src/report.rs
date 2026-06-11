//! Hardening report.
//!
//! The entrypoint forwards this as the session hardening profile. Written to
//! the path given by `--report`, as JSON; the shape is asserted by tests.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// Effective hardening applied before exec.
#[derive(Debug, Serialize)]
pub struct Report {
    /// Landlock ABI/status used, or `"unavailable"`.
    pub landlock: String,
    /// Whether the seccomp denylist was installed.
    pub seccomp_applied: bool,
    /// Names of the syscalls denied by the seccomp filter (empty when not applied).
    pub seccomp_blocked: Vec<String>,
    /// Whether the inheritable/ambient/bounding capability sets were cleared.
    pub caps_dropped: bool,
    /// Effective uid the payload runs as.
    pub uid: u32,
    /// Read-only paths granted to Landlock.
    pub ro_paths: Vec<String>,
    /// Read-write paths granted to Landlock.
    pub rw_paths: Vec<String>,
    /// The command (argv) that will be exec'd.
    pub command: Vec<String>,
}

impl Report {
    /// Serialize the report to `path` as pretty JSON.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("serializing report")?;
        std::fs::write(path, json)
            .with_context(|| format!("writing report to {}", path.display()))?;
        Ok(())
    }
}
