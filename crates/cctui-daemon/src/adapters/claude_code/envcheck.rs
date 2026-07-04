//! Verify gateway-env delivery against the live worker process (CCT-574).
//!
//! Dispatching a launch with `env` in the payload is not proof the worker runs
//! with it: the claude daemon can claim the session from a pre-warmed spare and
//! exec the worker WITHOUT the dispatch env (observed on CLI 2.1.201), leaving
//! an account-bound session silently consuming the machine's ambient
//! `~/.claude` login while cctui believes it launched gateway-routed. This
//! module is the ground-truth check: find the worker's process by the
//! `CLAUDE_CODE_SESSION_NAME=<short>` marker the claude daemon sets in every
//! worker's environment, and confirm the same environment carries the gateway
//! session token the dispatch delivered.
//!
//! Linux-only (`/proc`); on other platforms verification is a no-op
//! (indeterminate), which leaves the pre-CCT-574 behaviour unchanged. Token
//! values read from `/proc` are hashed and compared in memory, never logged or
//! stored — the caller only ever holds the sha256 the launch chokepoint already
//! recorded (CCT-503 invariant).

/// Did the live worker for `short` carry a gateway token hashing to
/// `want_hash`?
///
/// * `Some(true)` — a worker process for `short` exists and one of its
///   `ANTHROPIC_AUTH_TOKEN` / `OPENAI_API_KEY` values hashes to `want_hash`.
/// * `Some(false)` — at least one worker process for `short` exists and NONE
///   of them carry the expected token: delivery demonstrably failed.
/// * `None` — indeterminate: no matching process found (worker mid-exec, died,
///   hibernated) or the platform has no `/proc`. Never treated as a failure.
pub fn worker_carries_token(short: &str, want_hash: &str) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        scan_proc(std::path::Path::new("/proc"), short, want_hash)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (short, want_hash);
        None
    }
}

/// `/proc` scan, split out with the root injectable so tests can exercise it
/// against a synthetic tree on any platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn scan_proc(proc_root: &std::path::Path, short: &str, want_hash: &str) -> Option<bool> {
    let marker = format!("CLAUDE_CODE_SESSION_NAME={short}");
    let mut found_worker = false;
    for entry in std::fs::read_dir(proc_root).ok()?.flatten() {
        // PID directories only.
        if !entry.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        // Readable only for same-uid processes — everything else errors and is
        // skipped, which also keeps the scan cheap.
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else { continue };
        let mut is_worker = false;
        let mut carries = false;
        for var in environ.split(|&b| b == 0) {
            let Ok(var) = std::str::from_utf8(var) else { continue };
            if var == marker {
                is_worker = true;
            } else if let Some(tok) = var
                .strip_prefix("ANTHROPIC_AUTH_TOKEN=")
                .or_else(|| var.strip_prefix("OPENAI_API_KEY="))
                && crate::gateway_heal::sha256_hex(tok) == want_hash
            {
                carries = true;
            }
        }
        if is_worker {
            if carries {
                return Some(true);
            }
            found_worker = true;
        }
    }
    found_worker.then_some(false)
}

#[cfg(test)]
mod tests {
    use super::scan_proc;

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
    fn verdicts_from_a_synthetic_proc_tree() {
        let tmp = std::env::temp_dir().join(format!("cctui-envcheck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let tok_hash = crate::gateway_heal::sha256_hex("cctui_s_good");

        // Worker present WITH the token → Some(true), even if a sibling
        // process (e.g. the pty-host mid-exec) matches without it.
        fake_proc(
            &tmp,
            &[
                ("100", &["CLAUDE_CODE_SESSION_NAME=aaaa1111"] as &[&str]),
                (
                    "101",
                    &["CLAUDE_CODE_SESSION_NAME=aaaa1111", "ANTHROPIC_AUTH_TOKEN=cctui_s_good"],
                ),
                ("shm", &["NOT_A_PID=1"]), // non-numeric entries are skipped
            ],
        );
        assert_eq!(scan_proc(&tmp, "aaaa1111", &tok_hash), Some(true));

        // Worker present WITHOUT the token (or with a DIFFERENT token — a
        // stale relaunch) → Some(false).
        assert_eq!(scan_proc(&tmp, "bbbb2222", &tok_hash), None, "no such worker → indeterminate");
        let _ = std::fs::remove_dir_all(&tmp);
        fake_proc(
            &tmp,
            &[(
                "100",
                &["CLAUDE_CODE_SESSION_NAME=cccc3333", "ANTHROPIC_AUTH_TOKEN=cctui_s_stale"]
                    as &[&str],
            )],
        );
        assert_eq!(scan_proc(&tmp, "cccc3333", &tok_hash), Some(false));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
