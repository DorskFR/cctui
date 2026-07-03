//! Derive a resumable `cwd` from the on-disk Claude Code transcript when
//! `state.json` lacks one (CCT-504).
//!
//! The gateway-env heal (CCT-462) refuses to kill + cold-resume a worker whose
//! `state.json` carries no `cwd` — a kill would then be unrecoverable. But the
//! transcript at `<projects_root>/<encoded-cwd>/<session_id>.jsonl` still knows
//! the working directory, twice over:
//!
//! 1. **Authoritative:** transcript entries carry an explicit `"cwd"` field.
//! 2. **Last resort:** the project directory name IS the cwd with `/` (and `.`)
//!    encoded as `-` (see [`super::transcript::encode_cwd`]). Decoding is lossy
//!    — a `-` in a real path component is indistinguishable from an encoded
//!    `/` — so it's only trusted when the decoded path actually exists on disk.
//!
//! Read-only and local: no server calls, no schema.

use std::io::BufRead as _;
use std::path::Path;

/// Entries scanned per transcript before giving up on finding a `"cwd"` field.
/// Real transcripts carry `cwd` on essentially every entry, so this only bounds
/// the cost of scanning a huge transcript that (unexpectedly) has none.
const MAX_JSONL_LINES: usize = 100;

/// Find the transcript for `session_id` under `projects_root`
/// (`~/.claude/projects` in production — pass the adapter's configured
/// `projects_root`, which already honors any override) and derive the
/// session's working directory from it. Returns `None` when no transcript
/// exists for the session (a true orphan) or no candidate cwd checks out.
pub fn derive_cwd_from_transcript(projects_root: &Path, session_id: &str) -> Option<String> {
    let file_name = format!("{session_id}.jsonl");
    let entries = std::fs::read_dir(projects_root).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        let transcript = dir.join(&file_name);
        if !transcript.is_file() {
            continue;
        }
        // Authoritative source: an explicit `cwd` field in the JSONL entries.
        // Still verify it exists — resuming into a deleted directory would be
        // exactly the unrecoverable kill the pre-flight guards against.
        if let Some(cwd) = cwd_from_jsonl(&transcript)
            && Path::new(&cwd).is_dir()
        {
            return Some(cwd);
        }
        // Last resort: decode the (lossy) project directory name.
        if let Some(cwd) = entry.file_name().to_str().and_then(decode_project_dir_name) {
            return Some(cwd);
        }
    }
    None
}

/// Scan the first [`MAX_JSONL_LINES`] transcript entries for a top-level
/// string `"cwd"` field and return the first one found.
fn cwd_from_jsonl(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines().map_while(Result::ok).take(MAX_JSONL_LINES) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
            && let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str)
        {
            return Some(cwd.to_owned());
        }
    }
    None
}

/// Decode a `~/.claude/projects/` directory name back into a path:
/// `-home-you-Documents` → `/home/you/Documents`. The encoding maps BOTH `/`
/// and `.` to `-`, so this is lossy — only accept the result when the decoded
/// directory really exists.
fn decode_project_dir_name(name: &str) -> Option<String> {
    if !name.starts_with('-') {
        return None; // Not an encoded absolute path.
    }
    let candidate: String = name.chars().map(|c| if c == '-' { '/' } else { c }).collect();
    if Path::new(&candidate).is_dir() { Some(candidate) } else { None }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// Create `<root>/<dir_name>/<session>.jsonl` with the given lines.
    fn write_transcript(root: &Path, dir_name: &str, session: &str, lines: &[&str]) {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join(format!("{session}.jsonl"))).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    #[test]
    fn prefers_explicit_cwd_field_from_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        // A real cwd the transcript points at (contains a dash, which the
        // encoded dir name would decode wrongly — proves the field wins).
        let cwd = tmp.path().join("my-proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = cwd.to_str().unwrap().to_owned();
        write_transcript(
            &projects,
            "-somewhere-else",
            "sess-1",
            &[
                r#"{"type":"summary","summary":"no cwd here"}"#,
                &format!(r#"{{"type":"user","cwd":{}}}"#, serde_json::json!(cwd)),
            ],
        );
        assert_eq!(derive_cwd_from_transcript(&projects, "sess-1"), Some(cwd));
    }

    #[test]
    fn jsonl_cwd_must_exist_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        write_transcript(
            &projects,
            "-nonexistent-encoded",
            "sess-2",
            &[r#"{"cwd":"/nonexistent/deleted/path"}"#],
        );
        // Neither the jsonl cwd nor the decoded dir name exists → refuse.
        assert_eq!(derive_cwd_from_transcript(&projects, "sess-2"), None);
    }

    #[test]
    fn falls_back_to_decoded_dir_name_when_no_cwd_field() {
        // A dot/dash-free tempdir (the default `.tmpXXXX` prefix contains a
        // `.`, which the encoding maps to `-` — decode would then miss) so the
        // lossy decode round-trips.
        let tmp = tempfile::Builder::new().prefix("cctui504").tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = cwd.to_str().unwrap().to_owned();
        let encoded = super::super::transcript::encode_cwd(&cwd);
        write_transcript(&projects, &encoded, "sess-3", &[r#"{"type":"summary"}"#]);
        assert_eq!(derive_cwd_from_transcript(&projects, "sess-3"), Some(cwd));
    }

    #[test]
    fn decoded_dir_name_must_exist_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        write_transcript(&projects, "-no-such-dir", "sess-4", &[r#"{"type":"summary"}"#]);
        assert_eq!(derive_cwd_from_transcript(&projects, "sess-4"), None);
    }

    #[test]
    fn missing_transcript_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        write_transcript(&projects, "-somewhere", "other-session", &[r#"{"cwd":"/tmp"}"#]);
        assert_eq!(derive_cwd_from_transcript(&projects, "sess-5"), None);
        // And a projects root that doesn't exist at all.
        assert_eq!(derive_cwd_from_transcript(&projects.join("nope"), "sess-5"), None);
    }

    #[test]
    fn skips_malformed_jsonl_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = cwd.to_str().unwrap().to_owned();
        write_transcript(
            &projects,
            "-elsewhere",
            "sess-6",
            &["not json at all", &format!(r#"{{"cwd":{}}}"#, serde_json::json!(cwd))],
        );
        assert_eq!(derive_cwd_from_transcript(&projects, "sess-6"), Some(cwd));
    }
}
