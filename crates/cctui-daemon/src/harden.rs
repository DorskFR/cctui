use std::ffi::OsString;
use std::sync::OnceLock;

use crate::childenv::CHILD_ENV_REMOVALS;

type Vars = Vec<(&'static str, OsString)>;

static SCRUBBED: OnceLock<Vars> = OnceLock::new();

/// Must run before any other thread exists.
pub fn harden_run_process() {
    deny_inspection();
    let _ = SCRUBBED.set(scrub(CHILD_ENV_REMOVALS));
}

/// `execve` resets the flag, so every image must set it again.
#[cfg(target_os = "linux")]
pub fn deny_inspection() {
    use rustix::process::{DumpableBehavior, set_dumpable_behavior};
    if let Err(err) = set_dumpable_behavior(DumpableBehavior::NotDumpable) {
        eprintln!("cctui-daemon: PR_SET_DUMPABLE failed: {err}");
    }
}

#[cfg(not(target_os = "linux"))]
pub const fn deny_inspection() {}

#[must_use]
pub fn startup_var(name: &str) -> Option<String> {
    lookup(scrubbed(), name).or_else(|| std::env::var(name).ok())
}

pub fn export_for_reexec(cmd: &mut std::process::Command) {
    restore(cmd, scrubbed());
}

fn scrubbed() -> &'static [(&'static str, OsString)] {
    SCRUBBED.get().map(Vec::as_slice).unwrap_or_default()
}

#[allow(unsafe_code)]
fn scrub(names: &[&'static str]) -> Vars {
    let taken = names
        .iter()
        .filter_map(|&name| std::env::var_os(name).map(|v| (name, v)))
        .collect();
    for name in names {
        // SAFETY: only called from `main` before the runtime starts any thread.
        unsafe { std::env::remove_var(name) };
    }
    taken
}

fn lookup(vars: &[(&'static str, OsString)], name: &str) -> Option<String> {
    vars.iter().find(|(k, _)| *k == name).and_then(|(_, v)| v.to_str()).map(str::to_owned)
}

fn restore(cmd: &mut std::process::Command, vars: &[(&'static str, OsString)]) {
    for (k, v) in vars {
        cmd.env(k, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn deny_inspection_clears_the_dumpable_flag() {
        use rustix::process::{DumpableBehavior, dumpable_behavior};
        deny_inspection();
        assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
    }

    #[test]
    #[allow(unsafe_code)]
    fn scrub_removes_the_vars_from_the_environment_and_keeps_their_values() {
        const NAMES: &[&str] =
            &["CCTUI_HARDEN_TEST_KEY", "CCTUI_HARDEN_TEST_URL", "CCTUI_HARDEN_TEST_UNSET"];
        // SAFETY: these names are private to this test.
        unsafe {
            std::env::set_var(NAMES[0], "key");
            std::env::set_var(NAMES[1], "url");
            std::env::remove_var(NAMES[2]);
        }
        let taken = scrub(NAMES);
        assert_eq!(taken, vec![(NAMES[0], OsString::from("key")), (NAMES[1], "url".into())]);
        for name in NAMES {
            assert!(std::env::var_os(name).is_none(), "{name} still in the environment");
        }
        assert_eq!(lookup(&taken, NAMES[0]).as_deref(), Some("key"));
        assert_eq!(lookup(&taken, NAMES[2]), None);
    }

    #[test]
    fn scrubbed_vars_are_handed_back_on_reexec() {
        let vars: Vars = vec![("CCTUI_MACHINE_KEY", "key".into()), ("REPLY_URL", "url".into())];
        let mut cmd = std::process::Command::new("true");
        restore(&mut cmd, &vars);
        let envs: Vec<_> =
            cmd.get_envs().map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned))).collect();
        assert!(envs.contains(&("CCTUI_MACHINE_KEY".into(), Some("key".into()))), "{envs:?}");
        assert!(envs.contains(&("REPLY_URL".into(), Some("url".into()))), "{envs:?}");
    }
}
