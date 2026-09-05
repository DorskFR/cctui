//! Pruning of the per-session files the claude adapter writes into
//! `~/.config/cctui`: `hook-settings-<short>.json`, `mcp-agent-<short>.json`
//! and `whip-phrases-<short>.json`.
//!
//! Two mechanisms, because neither alone is sufficient:
//!
//! * A targeted delete when a session ends *for good* (archived, spawn failed,
//!   reaped). A plain worker exit is NOT such a point — cold resume reads the
//!   hook-settings file back to recover the whip posture.
//! * A sweep at daemon boot and every few hours for everything the targeted
//!   path missed (daemon killed mid-flight, sessions removed server-side):
//!   files whose `<short>` matches no session this machine still knows about,
//!   and any file past [`MAX_AGE`] regardless.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;

const PREFIXES: &[&str] = &["hook-settings-", "mcp-agent-", "whip-phrases-"];

/// Files older than this go regardless of whether their session is still known.
pub const MAX_AGE: Duration = Duration::from_hours(24 * 30);

/// Files younger than this are never swept: a spawn writes them before the
/// session exists anywhere the sweep can see it.
pub const MIN_AGE: Duration = Duration::from_hours(1);

const SWEEP_INTERVAL: Duration = Duration::from_hours(6);

/// Shorts of the sessions the server last told this machine it still has.
/// Seeded from the `ResumeMarks` frame the server sends on every connect.
static SERVER_SHORTS: OnceLock<Mutex<Option<HashSet<String>>>> = OnceLock::new();

fn server_shorts() -> &'static Mutex<Option<HashSet<String>>> {
    SERVER_SHORTS.get_or_init(|| Mutex::new(None))
}

/// Record the session ids the server currently lists for this machine.
pub fn note_server_sessions<I, S>(ids: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let shorts: HashSet<String> = ids.into_iter().filter_map(|id| short_of(id.as_ref())).collect();
    if let Ok(mut slot) = server_shorts().lock() {
        *slot = Some(shorts);
    }
}

fn short_of(session_id: &str) -> Option<String> {
    let short = session_id.get(..8)?.to_ascii_lowercase();
    is_short(&short).then_some(short)
}

fn is_short(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The `<short>` a per-session config file belongs to, or `None` for any other
/// file in the directory (`ask-hook-settings.json`, user files, …).
#[must_use]
pub fn session_short(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".json")?;
    let short = PREFIXES.iter().find_map(|p| stem.strip_prefix(p))?;
    is_short(short).then_some(short)
}

#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cctui"))
}

/// Drop every per-session file belonging to `short`. Best-effort.
pub fn remove_session_files(short: &str) {
    let Some(dir) = config_dir() else { return };
    let removed = remove_session_files_in(&dir, short);
    if removed > 0 {
        tracing::debug!(%short, removed, "pruned per-session config files");
    }
}

fn remove_session_files_in(dir: &Path, short: &str) -> usize {
    PREFIXES
        .iter()
        .filter(|p| std::fs::remove_file(dir.join(format!("{p}{short}.json"))).is_ok())
        .count()
}

fn expired(modified: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(modified).is_ok_and(|age| age > max_age)
}

fn too_young(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified).is_ok_and(|age| age < MIN_AGE)
}

/// Delete per-session files in `dir` whose short is absent from `live`, plus
/// any past `max_age` regardless. Returns the number of files removed.
pub fn sweep_dir<S: std::hash::BuildHasher>(
    dir: &Path,
    live: &HashSet<String, S>,
    now: SystemTime,
    max_age: Duration,
) -> std::io::Result<usize> {
    let mut removed = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(short) = session_short(name) else { continue };
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        let stale = expired(modified, now, max_age);
        if !stale && (live.contains(short) || too_young(modified, now)) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Sessions this machine still knows about: what the server last listed, unioned
/// with the shorts the claude daemon still holds job state for. The union is
/// deliberate — either source alone can lag, and keeping a file too long is far
/// cheaper than deleting a live session's settings.
fn live_shorts(jobs_root: &Path) -> HashSet<String> {
    let mut live: HashSet<String> =
        server_shorts().lock().ok().and_then(|s| s.clone()).unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(jobs_root) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && is_short(name)
            {
                live.insert(name.to_owned());
            }
        }
    }
    live
}

fn sweep_once() {
    let Some(dir) = config_dir() else { return };
    if !dir.is_dir() {
        return;
    }
    let live = live_shorts(&crate::adapters::claude_code::state::default_jobs_root());
    match sweep_dir(&dir, &live, SystemTime::now(), MAX_AGE) {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, live = live.len(), "swept per-session config files"),
        Err(err) => tracing::warn!(%err, path = %dir.display(), "config sweep failed"),
    }
}

/// Sweep now and every few hours until `shutdown`. Never fails the caller.
pub fn spawn_loop(shutdown: CancellationToken) {
    tokio::spawn(async move {
        loop {
            let _ = tokio::task::spawn_blocking(sweep_once).await;
            tokio::select! {
                () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                () = shutdown.cancelled() => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn seed(dir: &Path, name: &str, age: Duration) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"{}").unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
        path
    }

    #[test]
    fn matcher_accepts_only_the_three_per_session_shapes() {
        assert_eq!(session_short("hook-settings-aabb0011.json"), Some("aabb0011"));
        assert_eq!(session_short("mcp-agent-deadbeef.json"), Some("deadbeef"));
        assert_eq!(session_short("whip-phrases-00000000.json"), Some("00000000"));

        for other in [
            "ask-hook-settings.json",
            "hook-settings-.json",
            "hook-settings-aabb0011.txt",
            "hook-settings-aabb001.json",
            "hook-settings-aabb00112.json",
            "hook-settings-AABB0011.json",
            "hook-settings-zzzz0011.json",
            "mcp-agent-deadbeef",
            "config.json",
            "whip-phrases-dead beef.json",
        ] {
            assert_eq!(session_short(other), None, "{other} must not match");
        }
    }

    #[test]
    fn age_cutoff_is_thirty_days_with_a_young_file_grace() {
        let now = SystemTime::now();
        let day = Duration::from_hours(24);
        assert!(expired(now - 31 * day, now, MAX_AGE));
        assert!(!expired(now - 29 * day, now, MAX_AGE));
        assert!(!expired(now, now, MAX_AGE));
        // A clock that jumped backwards must not make everything expire.
        assert!(!expired(now + day, now, MAX_AGE));

        assert!(too_young(now - Duration::from_mins(1), now));
        assert!(!too_young(now - 2 * day, now));
    }

    #[test]
    fn sweep_removes_orphans_and_keeps_live_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let old = Duration::from_hours(24 * 3);
        for i in 0..50u32 {
            let short = format!("{i:08x}");
            for p in PREFIXES {
                seed(dir, &format!("{p}{short}.json"), old);
            }
        }
        for short in ["aaaa0001", "aaaa0002"] {
            for p in PREFIXES {
                seed(dir, &format!("{p}{short}.json"), old);
            }
        }
        seed(dir, "ask-hook-settings.json", old);
        seed(dir, "config.json", old);
        // Just-written files for a session nothing knows about yet survive.
        seed(dir, "hook-settings-bbbb0001.json", Duration::from_secs(5));

        let live: HashSet<String> =
            ["aaaa0001", "aaaa0002"].into_iter().map(str::to_owned).collect();
        let removed = sweep_dir(dir, &live, SystemTime::now(), MAX_AGE).unwrap();

        assert_eq!(removed, 150);
        for short in ["aaaa0001", "aaaa0002"] {
            for p in PREFIXES {
                assert!(dir.join(format!("{p}{short}.json")).exists(), "{p}{short} must survive");
            }
        }
        assert!(dir.join("ask-hook-settings.json").exists());
        assert!(dir.join("config.json").exists());
        assert!(dir.join("hook-settings-bbbb0001.json").exists());
        assert!(!dir.join("hook-settings-00000000.json").exists());
    }

    #[test]
    fn sweep_removes_a_live_sessions_files_once_past_the_max_age() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        seed(dir, "hook-settings-aaaa0001.json", Duration::from_hours(24 * 31));
        seed(dir, "mcp-agent-aaaa0001.json", Duration::from_hours(24 * 2));
        let live: HashSet<String> = std::iter::once("aaaa0001").map(str::to_owned).collect();

        assert_eq!(sweep_dir(dir, &live, SystemTime::now(), MAX_AGE).unwrap(), 1);
        assert!(!dir.join("hook-settings-aaaa0001.json").exists());
        assert!(dir.join("mcp-agent-aaaa0001.json").exists());
    }

    #[test]
    fn per_session_removal_drops_all_three_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for p in PREFIXES {
            seed(dir, &format!("{p}aaaa0001.json"), Duration::from_secs(1));
            seed(dir, &format!("{p}aaaa0002.json"), Duration::from_secs(1));
        }

        assert_eq!(remove_session_files_in(dir, "aaaa0001"), 3);
        assert_eq!(remove_session_files_in(dir, "aaaa0001"), 0);
        for p in PREFIXES {
            assert!(!dir.join(format!("{p}aaaa0001.json")).exists());
            assert!(dir.join(format!("{p}aaaa0002.json")).exists());
        }
    }

    #[test]
    fn server_session_ids_are_recorded_as_shorts() {
        note_server_sessions(["aabb0011-1111-2222-3333-444444444444", "not-a-uuid", "short"]);
        let live = live_shorts(Path::new("/nonexistent-jobs-root"));
        assert!(live.contains("aabb0011"));
        assert_eq!(live.len(), 1);
    }
}
