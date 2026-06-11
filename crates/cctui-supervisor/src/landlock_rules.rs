//! Landlock filesystem restriction.
//!
//! Port of the rule-construction in `main.go`: read-only path set + read-write
//! path set, best-effort newest-ABI-down, RW paths additionally granted
//! `IoctlDev` so `claude --bg` can ioctl its PTY. `path_beneath_rules`
//! auto-detects file vs directory and clamps access rights, replacing the Go
//! `os.Stat`/`RODirs`/`ROFiles`/`RWDirs`/`RWFiles` branching.

use anyhow::{Context, Result};
use landlock::{
    ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    path_beneath_rules,
};

/// Target Landlock ABI. The Go reference targets V5 (`landlock.V5.BestEffort()`);
/// the underlying access set at V5 includes `LANDLOCK_ACCESS_FS_IOCTL_DEV`,
/// which is why the Go code adds `WithIoctlDev()` to RW rules.
const TARGET_ABI: ABI = ABI::V5;

/// Outcome of applying Landlock, summarised for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Ruleset fully enforced at `abi`.
    Fully { abi: u32 },
    /// Ruleset partially enforced at `abi` (older kernel dropped some rights).
    Partial { abi: u32 },
    /// Kernel does not support Landlock at the target ABI.
    Unavailable,
}

impl Outcome {
    /// Human-readable ABI / status string for the report.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Fully { abi } => format!("V{abi} (fully-enforced)"),
            Self::Partial { abi } => format!("V{abi} (partially-enforced)"),
            Self::Unavailable => "unavailable".to_string(),
        }
    }

    #[must_use]
    pub const fn is_enforced(self) -> bool {
        matches!(self, Self::Fully { .. } | Self::Partial { .. })
    }
}

/// Apply Landlock restricting the caller to the given RO and RW path sets.
///
/// Best-effort: on a kernel without Landlock the ruleset reports `NotEnforced`
/// rather than erroring; the caller decides whether that is fatal (`--strict`).
/// Nonexistent paths are silently skipped by `path_beneath_rules` (it opens
/// each path and filters failures), mirroring the Go "warning: cannot stat …
/// (skipping)" behaviour.
pub fn apply(ro: &[String], rw: &[String]) -> Result<Outcome> {
    // Read-write access at the target ABI already includes IoctlDev (the V5
    // write set), so RW device files (the pts PTY) remain ioctl-able.
    let readonly = AccessFs::from_read(TARGET_ABI);
    let readwrite = AccessFs::from_all(TARGET_ABI);

    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(TARGET_ABI))
        .context("landlock: handle_access")?
        .create()
        .context("landlock: create ruleset")?
        .add_rules(path_beneath_rules(ro, readonly))
        .context("landlock: add read-only rules")?
        .add_rules(path_beneath_rules(rw, readwrite))
        .context("landlock: add read-write rules")?
        .restrict_self()
        .context("landlock: restrict_self")?;

    let abi = TARGET_ABI as u32;
    Ok(match status.ruleset {
        RulesetStatus::NotEnforced => Outcome::Unavailable,
        RulesetStatus::PartiallyEnforced => Outcome::Partial { abi },
        RulesetStatus::FullyEnforced => Outcome::Fully { abi },
    })
}
