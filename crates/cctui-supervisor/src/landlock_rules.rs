//! Landlock filesystem restriction.
//!
//! Port of the rule-construction in `main.go`: read-only path set + read-write
//! path set, best-effort newest-ABI-down, RW paths additionally granted
//! `IoctlDev` so `claude --bg` can ioctl its PTY. `path_beneath_rules`
//! auto-detects file vs directory and clamps access rights, replacing the Go
//! `os.Stat`/`RODirs`/`ROFiles`/`RWDirs`/`RWFiles` branching.

use std::path::{Component, Path};

use anyhow::{Context, Result};
use landlock::{
    ABI, Access, AccessFs, BitFlags, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
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

/// One Landlock rule set: `access` beneath each of `paths`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub paths: Vec<String>,
    pub access: BitFlags<AccessFs>,
}

/// The rules [`apply_with`] installs. Under `guard_exec` the RW paths lose
/// `Execute` and only the `exec_rw` subtrees get it back, as an `Execute`-only
/// grant: Landlock unions every rule above a path, so such a grant never adds
/// write access anywhere.
#[must_use]
pub fn plan(ro: &[String], rw: &[String], exec_rw: &[String], guard_exec: bool) -> Vec<Grant> {
    let mut readwrite = AccessFs::from_all(TARGET_ABI);
    if guard_exec {
        readwrite.remove(AccessFs::Execute);
    }
    let mut grants = vec![
        Grant { paths: ro.to_vec(), access: AccessFs::from_read(TARGET_ABI) },
        Grant { paths: rw.to_vec(), access: readwrite },
    ];
    if guard_exec && !exec_rw.is_empty() {
        grants.push(Grant { paths: exec_rw.to_vec(), access: AccessFs::Execute.into() });
    }
    grants
}

/// Apply Landlock restricting the caller to the given RO and RW path sets.
///
/// Best-effort: on a kernel without Landlock the ruleset reports `NotEnforced`
/// rather than erroring; the caller decides whether that is fatal (`--strict`).
/// Nonexistent paths are silently skipped by `path_beneath_rules` (it opens
/// each path and filters failures), mirroring the Go "warning: cannot stat …
/// (skipping)" behaviour.
pub fn apply(ro: &[String], rw: &[String]) -> Result<Outcome> {
    apply_with(ro, rw, &[], false)
}

/// [`apply`] per [`plan`]: with `guard_exec`, a binary written to an RW path
/// outside the `exec_rw` subtrees cannot be run.
pub fn apply_with(
    ro: &[String],
    rw: &[String],
    exec_rw: &[String],
    guard_exec: bool,
) -> Result<Outcome> {
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(TARGET_ABI))
        .context("landlock: handle_access")?
        .create()
        .context("landlock: create ruleset")?;
    for grant in plan(ro, rw, exec_rw, guard_exec) {
        ruleset = ruleset
            .add_rules(path_beneath_rules(&grant.paths, grant.access))
            .context("landlock: add rules")?;
    }
    let status = ruleset.restrict_self().context("landlock: restrict_self")?;

    let abi = TARGET_ABI as u32;
    Ok(match status.ruleset {
        RulesetStatus::NotEnforced => Outcome::Unavailable,
        RulesetStatus::PartiallyEnforced => Outcome::Partial { abi },
        RulesetStatus::FullyEnforced => Outcome::Fully { abi },
    })
}

/// Vet the executable subtrees and create the missing ones, so a toolchain
/// installed after boot (`cargo install`, an npx cache) lands inside a rule
/// that already exists. Kept: an absolute entry without `..` beneath an RW
/// path, reached without a symlink below that path. Created directories are
/// owned by `owner` when given.
pub fn prepare_exec_rw(
    exec_rw: &[String],
    rw: &[String],
    owner: Option<u32>,
) -> std::io::Result<Vec<String>> {
    let mut kept = Vec::new();
    for entry in exec_rw {
        let path = Path::new(entry);
        if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            continue;
        }
        let Some(root) = rw.iter().map(Path::new).find(|r| path.starts_with(r)) else {
            continue;
        };
        if create_beneath(root, path, owner)? {
            kept.push(entry.clone());
        }
    }
    Ok(kept)
}

/// `Ok(true)` once every component of `path` below `root` is a real directory.
fn create_beneath(root: &Path, path: &Path, owner: Option<u32>) -> std::io::Result<bool> {
    if !root.is_dir() {
        return Ok(false);
    }
    let Ok(below) = path.strip_prefix(root) else {
        return Ok(false);
    };
    let mut cur = root.to_path_buf();
    for component in below.components() {
        cur.push(component);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&cur)?;
                if let Some(uid) = owner {
                    std::os::unix::fs::chown(&cur, Some(uid), Some(uid))?;
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use landlock::{AccessFs, BitFlags};

    use super::{Grant, plan, prepare_exec_rw};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    /// What Landlock grants at `path`: the union of every rule at or above it.
    fn access_at(grants: &[Grant], path: &str) -> BitFlags<AccessFs> {
        grants
            .iter()
            .filter(|g| g.paths.iter().any(|p| Path::new(path).starts_with(p)))
            .fold(BitFlags::empty(), |acc, g| acc | g.access)
    }

    fn guarded() -> Vec<Grant> {
        plan(
            &strings(&["/usr", "/etc"]),
            &strings(&["/dev", "/tmp", "/workspace", "/home/worker"]),
            &strings(&["/workspace", "/home/worker/.cargo"]),
            true,
        )
    }

    #[test]
    fn under_guard_exec_only_the_exec_subtrees_of_rw_paths_execute() {
        let grants = guarded();
        for path in [
            "/workspace/repo/target/debug/deps/app-0123abcd",
            "/workspace/repo/node_modules/.bin/vitest",
            "/workspace/repo/scripts/check.sh",
            "/home/worker/.cargo/bin/cargo-nextest",
            "/usr/bin/git",
        ] {
            assert!(access_at(&grants, path).contains(AccessFs::Execute), "{path} must execute");
        }
        for path in ["/tmp/payload", "/home/worker/payload", "/home/worker/.cargo-x/payload"] {
            let access = access_at(&grants, path);
            assert!(!access.contains(AccessFs::Execute), "{path} must be noexec");
            assert!(access.contains(AccessFs::WriteFile), "{path} stays writable");
        }
    }

    #[test]
    fn an_exec_subtree_never_adds_write_access() {
        let exec_rw = strings(&["/usr/local", "/opt/tools"]);
        let grants = plan(&strings(&["/usr"]), &strings(&["/tmp"]), &exec_rw, true);
        let access = access_at(&grants, "/usr/local/bin/tool");
        assert!(access.contains(AccessFs::Execute));
        assert!(!access.contains(AccessFs::WriteFile), "{access:?}");
        assert_eq!(access_at(&grants, "/opt/tools/bin/tool"), BitFlags::from(AccessFs::Execute));
    }

    #[test]
    fn without_guard_exec_rw_paths_keep_execute_and_no_exec_rule_is_added() {
        let grants = plan(&strings(&["/usr"]), &strings(&["/tmp"]), &strings(&["/tmp/x"]), false);
        assert_eq!(grants.len(), 2);
        assert!(access_at(&grants, "/tmp/payload").contains(AccessFs::Execute));
    }

    #[test]
    fn an_empty_exec_set_leaves_every_rw_path_noexec() {
        let grants = plan(&strings(&["/usr"]), &strings(&["/workspace"]), &[], true);
        assert_eq!(grants.len(), 2);
        let access = access_at(&grants, "/workspace/repo/target/debug/app");
        assert!(!access.contains(AccessFs::Execute), "{access:?}");
    }

    #[test]
    fn missing_exec_subtrees_under_rw_are_created_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let rw = dir.path().join("home");
        std::fs::create_dir(&rw).unwrap();
        let cargo = rw.join(".cargo");
        let mise = rw.join(".local/share/mise");
        let entries = strings(&[cargo.to_str().unwrap(), mise.to_str().unwrap()]);
        let kept = prepare_exec_rw(&entries, &strings(&[rw.to_str().unwrap()]), None).unwrap();
        assert_eq!(kept, entries);
        assert!(cargo.is_dir() && mise.is_dir());
    }

    #[test]
    fn exec_subtrees_outside_rw_or_through_a_symlink_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let rw = dir.path().join("home");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&rw).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, rw.join("link")).unwrap();
        let rw_set = strings(&[rw.to_str().unwrap()]);
        let entries = [
            outside.join("bin"),
            rw.join("link/bin"),
            rw.join("../outside/bin"),
            Path::new("relative/bin").to_path_buf(),
        ];
        for entry in entries {
            let entry = strings(&[entry.to_str().unwrap()]);
            assert!(prepare_exec_rw(&entry, &rw_set, None).unwrap().is_empty(), "{entry:?}");
        }
        assert!(!outside.join("bin").exists(), "nothing is created outside RW");
    }
}
