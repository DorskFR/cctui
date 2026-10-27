//! Capability + uid/gid privilege drop.
//!
//! The Go reference relies on the seccomp denylist (setuid/setgid/etc. blocked)
//! plus running as a non-root user assigned by the container runtime. The
//! ticket additionally asks the supervisor to actively clear the inheritable,
//! ambient and bounding capability sets and `setgid`/`setuid` to the target
//! user before exec.
//!
//! IMPORTANT ordering: capability clearing and `setgroups`/`setgid`/`setuid`
//! must happen BEFORE the seccomp denylist is installed, because the denylist
//! blocks `setuid`/`setgid`. The caller is responsible for sequencing.

use anyhow::{Context, Result, bail};
use caps::CapSet;
use nix::sys::prctl;
use nix::unistd::{Gid, Uid, getresgid, getresuid, setgid, setgroups, setresgid, setresuid};

/// Clear the inheritable, ambient and bounding capability sets.
///
/// After this the target user inherits nothing and cannot regain privilege
/// across exec. Ambient/bounding may already be empty for a non-root caller;
/// clearing is idempotent. A non-root caller can't shrink the bounding set
/// (needs `CAP_SETPCAP`), so it gets `no_new_privs` instead: exec then can't
/// gain anything the bounding set still lists.
pub fn drop_capabilities() -> Result<()> {
    // Ambient first (must be empty before/independent of bounding on some
    // kernels); ignore "not supported" only by surfacing the error to caller.
    let sets: &[CapSet] = if Uid::effective().is_root() {
        &[CapSet::Ambient, CapSet::Bounding, CapSet::Inheritable]
    } else {
        prctl::set_no_new_privs().context("setting no_new_privs")?;
        &[CapSet::Ambient, CapSet::Inheritable]
    };
    for &set in sets {
        caps::clear(None, set).with_context(|| format!("clearing {set:?} capability set"))?;
    }
    Ok(())
}

/// What [`switch_user`] has to do, from the current (real, effective, saved)
/// uids and gids.
#[derive(Debug, PartialEq, Eq)]
pub enum Switch {
    /// Not yet the target: drop to it.
    Drop,
    /// Already the target uid and gid (a non-root start): nothing to change.
    AlreadyTarget,
    /// Already the target uid, but an unprivileged process can't fix the gid.
    WrongGroup(u32),
}

/// Decides the [`Switch`] for the given credentials and target uid.
#[must_use]
pub fn plan_switch(uids: [u32; 3], gids: [u32; 3], target: u32) -> Switch {
    if uids.iter().any(|&u| u != target) {
        return Switch::Drop;
    }
    gids.iter().find(|&&g| g != target).map_or(Switch::AlreadyTarget, |&g| Switch::WrongGroup(g))
}

/// Drop to the target uid, using the uid as the gid as well.
///
/// (The worker user's primary group equals its uid.) Clears supplementary
/// groups, then sets gid before uid so the uid change does not strip the
/// privilege needed to set the gid. Returns `false` when the process already
/// runs as the target, so there was nothing to switch.
pub fn switch_user(uid: u32) -> Result<bool> {
    let ru = getresuid().context("getresuid")?;
    let rg = getresgid().context("getresgid")?;
    match plan_switch(
        [ru.real.as_raw(), ru.effective.as_raw(), ru.saved.as_raw()],
        [rg.real.as_raw(), rg.effective.as_raw(), rg.saved.as_raw()],
        uid,
    ) {
        Switch::AlreadyTarget => return Ok(false),
        Switch::WrongGroup(gid) => {
            bail!("already uid {uid} but gid {gid}, unfixable without root: set runAsGroup: {uid}")
        }
        Switch::Drop => {}
    }

    let user = Uid::from_raw(uid);
    let group = Gid::from_raw(uid);

    // Drop supplementary groups (best-effort: requires CAP_SETGID, which we
    // still hold if launched as root; if it fails because we are already the
    // unprivileged user, the subsequent setresuid no-op is fine).
    let _ = setgroups(&[]);

    setgid(group).context("setgid")?;
    // Pin all three gid fields so nothing can be restored.
    setresgid(group, group, group).context("setresgid")?;
    // Pin all three uid fields.
    setresuid(user, user, user).context("setresuid")?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{Switch, plan_switch};

    #[test]
    fn root_start_drops() {
        assert_eq!(plan_switch([0, 0, 0], [0, 0, 0], 1000), Switch::Drop);
    }

    #[test]
    fn non_root_start_at_target_needs_no_switch() {
        assert_eq!(plan_switch([1000; 3], [1000; 3], 1000), Switch::AlreadyTarget);
    }

    #[test]
    fn non_root_start_with_wrong_gid_is_reported() {
        assert_eq!(plan_switch([1000; 3], [1000, 0, 1000], 1000), Switch::WrongGroup(0));
    }

    #[test]
    fn root_saved_uid_still_drops() {
        assert_eq!(plan_switch([1000, 1000, 0], [1000; 3], 1000), Switch::Drop);
    }

    #[test]
    fn another_uid_drops() {
        assert_eq!(plan_switch([1001; 3], [1001; 3], 1000), Switch::Drop);
    }
}
