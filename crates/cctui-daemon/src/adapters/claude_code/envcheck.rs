//! Read ground-truth per-worker signals from the live process environment via
//! `/proc` (CCT-577).
//!
//! Each claude worker carries a `CLAUDE_CODE_SESSION_NAME=<short>` marker in its
//! environment; scanning `/proc/<pid>/environ` for it lets us read the actual
//! `CLAUDE_EFFORT` a running session booted at — which a spare-claim or a silent
//! background clamp can make differ from the `--effort` cctui requested.
//!
//! Linux-only (`/proc`); on other platforms the reads are a no-op (indeterminate).

/// Ground-truth reasoning effort of the live workers, read from the
/// `CLAUDE_EFFORT` env each claude worker carries in its process environment
/// (CCT-577). This is the actual level a running session booted at — which a
/// spare-claim or a silent background clamp can make differ from the `--effort`
/// cctui requested. Returns `short -> effort` for every worker in `wanted` that
/// was found with a non-empty `CLAUDE_EFFORT`; missing entries are indeterminate
/// (worker mid-exec / no `/proc`) and left to the caller's fallback. Done as a
/// SINGLE `/proc` pass so a busy roster doesn't rescan per session per tick.
pub fn worker_efforts(
    wanted: &std::collections::HashSet<String>,
) -> std::collections::HashMap<String, String> {
    #[cfg(target_os = "linux")]
    {
        scan_proc_efforts(std::path::Path::new("/proc"), wanted)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = wanted;
        std::collections::HashMap::new()
    }
}

/// One `/proc` pass collecting `short -> CLAUDE_EFFORT` for the wanted shorts,
/// root injectable for tests.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn scan_proc_efforts(
    proc_root: &std::path::Path,
    wanted: &std::collections::HashSet<String>,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if wanted.is_empty() {
        return out;
    }
    let Ok(dir) = std::fs::read_dir(proc_root) else { return out };
    for entry in dir.flatten() {
        if !entry.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else { continue };
        let mut short = None;
        let mut effort = None;
        for var in environ.split(|&b| b == 0) {
            let Ok(var) = std::str::from_utf8(var) else { continue };
            if let Some(s) = var.strip_prefix("CLAUDE_CODE_SESSION_NAME=") {
                short = Some(s.to_owned());
            } else if let Some(e) = var.strip_prefix("CLAUDE_EFFORT=") {
                effort = Some(e.to_owned());
            }
        }
        if let (Some(short), Some(effort)) = (short, effort)
            && !effort.trim().is_empty()
            && wanted.contains(&short)
        {
            out.insert(short, effort);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::scan_proc_efforts;

    /// Build a fake /proc: each `(pid, environ_vars)` becomes
    /// `<root>/<pid>/environ` with NUL-joined vars.
    fn fake_proc(dir: &std::path::Path, procs: &[(&str, &[&str])]) {
        for (pid, vars) in procs {
            let d = dir.join(pid);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("environ"), vars.join("\0")).unwrap();
        }
    }

    #[test]
    fn reads_ground_truth_effort_from_worker_environ() {
        let tmp = std::env::temp_dir().join(format!("cctui-effort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        fake_proc(
            &tmp,
            &[
                ("200", &["CLAUDE_CODE_SESSION_NAME=dddd4444", "CLAUDE_EFFORT=medium"] as &[&str]),
                // A worker not in `wanted` is skipped.
                ("201", &["CLAUDE_CODE_SESSION_NAME=zzzz9999", "CLAUDE_EFFORT=high"]),
                // A worker present but with no CLAUDE_EFFORT → indeterminate (absent).
                ("202", &["CLAUDE_CODE_SESSION_NAME=eeee5555"]),
                // A sibling non-worker process carrying CLAUDE_EFFORT is ignored.
                ("203", &["CLAUDE_EFFORT=xhigh"]),
            ],
        );
        let wanted: std::collections::HashSet<String> =
            ["dddd4444", "eeee5555", "ffff6666"].iter().map(|s| (*s).to_owned()).collect();
        let got = scan_proc_efforts(&tmp, &wanted);
        assert_eq!(got.get("dddd4444").map(String::as_str), Some("medium"));
        assert_eq!(got.get("eeee5555"), None, "worker without CLAUDE_EFFORT → absent");
        assert_eq!(got.get("ffff6666"), None, "no such worker → absent");
        assert_eq!(got.get("zzzz9999"), None, "worker not in wanted → absent");
        // Empty wanted set short-circuits to an empty map.
        assert!(scan_proc_efforts(&tmp, &std::collections::HashSet::new()).is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
