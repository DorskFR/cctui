//! Read ground-truth per-worker signals from the live process environment via
//! `/proc`.
//!
//! Each claude worker carries a `CLAUDE_CODE_SESSION_NAME=<short>` marker in its
//! environment; scanning `/proc/<pid>/environ` for it lets us read the actual
//! `CLAUDE_EFFORT` a running session booted at — which a spare-claim or a silent
//! background clamp can make differ from the `--effort` cctui requested.
//!
//! Linux-only (`/proc`); on other platforms the reads are a no-op (indeterminate).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Ground-truth reasoning effort of the live workers, read from the
/// `CLAUDE_EFFORT` env each claude worker carries in its process environment.
/// Returns `short -> effort` for every worker in `wanted` that was found with a
/// non-empty `CLAUDE_EFFORT`; missing entries are indeterminate (worker mid-exec
/// / no `/proc`) and left to the caller's fallback.
pub async fn worker_efforts(wanted: &HashSet<String>) -> HashMap<String, String> {
    #[cfg(target_os = "linux")]
    {
        use std::sync::{Arc, Mutex, OnceLock};
        static SCANNER: OnceLock<Arc<Mutex<EffortScanner>>> = OnceLock::new();
        if wanted.is_empty() {
            return HashMap::new();
        }
        let scanner =
            Arc::clone(SCANNER.get_or_init(|| Arc::new(Mutex::new(EffortScanner::new("/proc")))));
        let wanted = wanted.clone();
        tokio::task::spawn_blocking(move || {
            scanner.lock().map(|mut s| s.scan(&wanted)).unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = wanted;
        HashMap::new()
    }
}

/// Remembers which pid carries which worker short, so a steady roster costs
/// one `environ` read per worker. The full `/proc` walk only runs when a wanted
/// short has no known pid and the wanted set changed, or a known pid went away.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct EffortScanner {
    proc_root: PathBuf,
    pid_by_short: HashMap<String, String>,
    walked_for: HashSet<String>,
    reads: usize,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl EffortScanner {
    fn new(proc_root: impl Into<PathBuf>) -> Self {
        Self {
            proc_root: proc_root.into(),
            pid_by_short: HashMap::new(),
            walked_for: HashSet::new(),
            reads: 0,
        }
    }

    fn scan(&mut self, wanted: &HashSet<String>) -> HashMap<String, String> {
        self.reads = 0;
        let mut out = HashMap::new();
        self.pid_by_short.retain(|short, _| wanted.contains(short));
        let known: Vec<(String, String)> =
            self.pid_by_short.iter().map(|(s, p)| (s.clone(), p.clone())).collect();
        for (short, pid) in known {
            match self.read(&pid) {
                Some((Some(found), effort)) if found == short => {
                    if let Some(effort) = effort {
                        out.insert(short, effort);
                    }
                }
                _ => {
                    self.pid_by_short.remove(&short);
                    self.walked_for.clear();
                }
            }
        }
        let missing = wanted.iter().any(|s| !self.pid_by_short.contains_key(s));
        if missing && self.walked_for != *wanted {
            self.walked_for = wanted.clone();
            self.walk(wanted, &mut out);
        }
        out
    }

    fn walk(&mut self, wanted: &HashSet<String>, out: &mut HashMap<String, String>) {
        let Ok(dir) = std::fs::read_dir(&self.proc_root) else { return };
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().map(str::to_owned) else { continue };
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if self.pid_by_short.values().any(|p| *p == pid) {
                continue;
            }
            let Some((Some(short), effort)) = self.read(&pid) else { continue };
            if !wanted.contains(&short) || self.pid_by_short.contains_key(&short) {
                continue;
            }
            self.pid_by_short.insert(short.clone(), pid);
            if let Some(effort) = effort {
                out.insert(short, effort);
            }
        }
    }

    /// `(CLAUDE_CODE_SESSION_NAME, non-empty CLAUDE_EFFORT)` of one pid.
    fn read(&mut self, pid: &str) -> Option<(Option<String>, Option<String>)> {
        self.reads += 1;
        let environ = std::fs::read(Path::new(&self.proc_root).join(pid).join("environ")).ok()?;
        let mut short = None;
        let mut effort = None;
        for var in environ.split(|&b| b == 0) {
            let Ok(var) = std::str::from_utf8(var) else { continue };
            if let Some(s) = var.strip_prefix("CLAUDE_CODE_SESSION_NAME=") {
                short = Some(s.to_owned());
            } else if let Some(e) = var.strip_prefix("CLAUDE_EFFORT=")
                && !e.trim().is_empty()
            {
                effort = Some(e.to_owned());
            }
        }
        Some((short, effort))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::EffortScanner;

    fn fake_proc(dir: &std::path::Path, procs: &[(&str, &[&str])]) {
        for (pid, vars) in procs {
            let d = dir.join(pid);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("environ"), vars.join("\0")).unwrap();
        }
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reads_ground_truth_effort_from_worker_environ() {
        let tmp = tempfile::tempdir().unwrap();
        fake_proc(
            tmp.path(),
            &[
                ("200", &["CLAUDE_CODE_SESSION_NAME=dddd4444", "CLAUDE_EFFORT=medium"] as &[&str]),
                ("201", &["CLAUDE_CODE_SESSION_NAME=zzzz9999", "CLAUDE_EFFORT=high"]),
                ("202", &["CLAUDE_CODE_SESSION_NAME=eeee5555"]),
                ("203", &["CLAUDE_EFFORT=xhigh"]),
            ],
        );
        let mut scanner = EffortScanner::new(tmp.path());
        let got = scanner.scan(&set(&["dddd4444", "eeee5555", "ffff6666"]));
        assert_eq!(got.get("dddd4444").map(String::as_str), Some("medium"));
        assert_eq!(got.get("eeee5555"), None, "worker without CLAUDE_EFFORT → absent");
        assert_eq!(got.get("ffff6666"), None, "no such worker → absent");
        assert_eq!(got.get("zzzz9999"), None, "worker not in wanted → absent");
        assert!(scanner.scan(&HashSet::new()).is_empty());
    }

    #[test]
    fn steady_roster_reads_at_most_one_environ_per_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let noise: Vec<(String, Vec<&str>)> =
            (1000..1500).map(|pid| (pid.to_string(), vec!["PATH=/usr/bin"])).collect();
        for (pid, vars) in &noise {
            fake_proc(tmp.path(), &[(pid.as_str(), vars.as_slice())]);
        }
        fake_proc(
            tmp.path(),
            &[
                ("10", &["CLAUDE_CODE_SESSION_NAME=aaaa1111", "CLAUDE_EFFORT=high"] as &[&str]),
                ("11", &["CLAUDE_CODE_SESSION_NAME=bbbb2222", "CLAUDE_EFFORT=low"]),
                ("12", &["CLAUDE_CODE_SESSION_NAME=cccc3333"]),
            ],
        );
        let wanted = set(&["aaaa1111", "bbbb2222", "cccc3333", "gone0000"]);
        let mut scanner = EffortScanner::new(tmp.path());
        scanner.scan(&wanted);
        assert!(scanner.reads > wanted.len(), "first tick walks /proc");

        for _ in 0..3 {
            let got = scanner.scan(&wanted);
            assert!(scanner.reads <= 3, "steady tick read {} environs", scanner.reads);
            assert_eq!(got.get("aaaa1111").map(String::as_str), Some("high"));
            assert_eq!(got.get("bbbb2222").map(String::as_str), Some("low"));
        }

        fake_proc(
            tmp.path(),
            &[("11", &["CLAUDE_CODE_SESSION_NAME=bbbb2222", "CLAUDE_EFFORT=max"])],
        );
        assert_eq!(scanner.scan(&wanted).get("bbbb2222").map(String::as_str), Some("max"));
        assert!(scanner.reads <= 3);

        let grown = set(&["aaaa1111", "bbbb2222", "cccc3333", "gone0000", "dddd4444"]);
        fake_proc(
            tmp.path(),
            &[("13", &["CLAUDE_CODE_SESSION_NAME=dddd4444", "CLAUDE_EFFORT=xhigh"])],
        );
        assert_eq!(scanner.scan(&grown).get("dddd4444").map(String::as_str), Some("xhigh"));
        scanner.scan(&grown);
        assert!(scanner.reads <= 4);
    }

    #[test]
    fn respawned_worker_is_found_under_its_new_pid() {
        let tmp = tempfile::tempdir().unwrap();
        fake_proc(
            tmp.path(),
            &[("20", &["CLAUDE_CODE_SESSION_NAME=aaaa1111", "CLAUDE_EFFORT=high"] as &[&str])],
        );
        let wanted = set(&["aaaa1111"]);
        let mut scanner = EffortScanner::new(tmp.path());
        assert_eq!(scanner.scan(&wanted).len(), 1);
        std::fs::remove_dir_all(tmp.path().join("20")).unwrap();
        fake_proc(
            tmp.path(),
            &[("21", &["CLAUDE_CODE_SESSION_NAME=aaaa1111", "CLAUDE_EFFORT=low"] as &[&str])],
        );
        scanner.scan(&wanted);
        let got = scanner.scan(&wanted);
        assert_eq!(got.get("aaaa1111").map(String::as_str), Some("low"));
    }
}
