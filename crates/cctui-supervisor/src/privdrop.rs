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

use anyhow::{Context, Result};
use caps::CapSet;
use nix::unistd::{Gid, Uid, setgid, setgroups, setresgid, setresuid};

/// Clear the inheritable, ambient and bounding capability sets.
///
/// After this the target user inherits nothing and cannot regain privilege
/// across exec. Ambient/bounding may already be empty for a non-root caller;
/// clearing is idempotent.
pub fn drop_capabilities() -> Result<()> {
    // Ambient first (must be empty before/independent of bounding on some
    // kernels); ignore "not supported" only by surfacing the error to caller.
    for set in [CapSet::Ambient, CapSet::Bounding, CapSet::Inheritable] {
        caps::clear(None, set).with_context(|| format!("clearing {set:?} capability set"))?;
    }
    Ok(())
}

/// Drop to the target uid, using the uid as the gid as well.
///
/// (The worker user's primary group equals its uid.) Clears supplementary
/// groups, then sets gid before uid so the uid change does not strip the
/// privilege needed to set the gid.
pub fn switch_user(uid: u32) -> Result<()> {
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

    Ok(())
}
