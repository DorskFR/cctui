//! Versioned Codex `app-server` protocol contract.
//!
//! Single source of truth for the minimum Codex version cctui supports. The
//! floor is consumed by:
//!
//! - the [`super::app_server`] handshake (declared client version, and the
//!   floor the discovered server version is checked against);
//! - `deploy/worker.Dockerfile` (`ARG CODEX_VERSION`), which installs exactly
//!   the floor; `scripts/check-codex-version-drift.sh` keeps the two equal and
//!   checks any installed `codex` against the floor;
//! - the retained JSON Schema under `schema/`, generated from the floor build
//!   with `codex app-server generate-json-schema --out schema/`.
//!
//! The floor is a MINIMUM, not an exact pin: derived worker images (the harbor
//! bake) refetch the harness, so workers run whatever is current at bake time,
//! never older than the floor. The schema bundle
//! (`schema/codex_app_server_protocol.schemas.json`) therefore documents the
//! shapes the adapter is guaranteed to find — `initialize`, `initialized`,
//! `thread/start`, `thread/resume`, `thread/fork`, `turn/start`, `thread/list`,
//! `thread/read`, and the approval requests — not everything a newer server
//! may send. Additions in a newer Codex are methods and notifications the
//! adapter may not consume yet; they must surface as a visible gap, never be
//! silently dropped. Regenerate the bundle whenever [`CODEX_MIN_VERSION`] is
//! raised.

/// The minimum Codex version whose `app-server` protocol the adapter speaks
/// correctly. It is the version the worker image installs and the retained
/// JSON Schema was generated from. Sessions started against an older server
/// keep running but are flagged loudly in diagnostics — the handshake /
/// thread / approval shapes below this floor are not guaranteed.
pub const CODEX_MIN_VERSION: &str = "0.153.4";

/// A parsed `major.minor.patch` triple. Pre-release / build metadata is
/// ignored — the floor only reasons about the release line.
type SemVer = (u64, u64, u64);

/// Parse a leading `major.minor.patch` out of a version string, tolerating a
/// trailing `-pre` / `+build` suffix and extra dotted components.
fn parse_semver(v: &str) -> Option<SemVer> {
    let core = v.trim().split(['-', '+']).next().unwrap_or(v);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Extract the Codex version from the `userAgent` string returned in the
/// `initialize` response. Codex formats it as
/// `"<client-name>/<codex-version> (<os>; <arch>) …"`, e.g.
/// `"cctui/0.144.1 (Ubuntu 24.4.0; x86_64) …"`, so the version is the token
/// after the first `/`, up to the next whitespace.
#[must_use]
pub fn version_from_user_agent(user_agent: &str) -> Option<String> {
    let after_slash = user_agent.split_once('/')?.1;
    let token = after_slash.split_whitespace().next()?;
    // Only accept it if it looks like a version we can reason about.
    parse_semver(token).map(|_| token.to_owned())
}

/// Whether a discovered Codex version is at or above [`CODEX_MIN_VERSION`]. An
/// unparseable version is treated as unsupported (better to flag than to
/// silently assume compatibility).
#[must_use]
pub fn version_supported(version: &str) -> bool {
    match (parse_semver(version), parse_semver(CODEX_MIN_VERSION)) {
        (Some(got), Some(min)) => got >= min,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_yields_codex_version() {
        let ua = "cctui/0.144.1 (Ubuntu 24.4.0; x86_64) xterm-256color (cctui; 0.0.0)";
        assert_eq!(version_from_user_agent(ua).as_deref(), Some("0.144.1"));
    }

    #[test]
    fn user_agent_without_version_is_none() {
        assert_eq!(version_from_user_agent("no-slash-here").as_deref(), None);
        assert_eq!(version_from_user_agent("codex/notaversion").as_deref(), None);
    }

    #[test]
    fn support_floor_is_inclusive() {
        assert!(version_supported(CODEX_MIN_VERSION));
        assert!(version_supported("0.153.9"));
        assert!(version_supported("0.154.0"));
        assert!(version_supported("1.0.0"));
        assert!(!version_supported("0.153.3"));
        assert!(!version_supported("0.144.1"));
        assert!(!version_supported("0.99.0"));
        assert!(!version_supported("garbage"));
    }

    #[test]
    fn floor_is_a_concrete_release() {
        assert!(parse_semver(CODEX_MIN_VERSION).is_some());
        assert_eq!(
            CODEX_MIN_VERSION.split('.').count(),
            3,
            "CODEX_MIN_VERSION {CODEX_MIN_VERSION} must be a concrete x.y.z"
        );
    }

    #[test]
    fn prerelease_and_extra_components_parse() {
        assert_eq!(parse_semver("0.144.1-rc.1"), Some((0, 144, 1)));
        assert_eq!(parse_semver("0.144.1+build.5"), Some((0, 144, 1)));
        assert_eq!(parse_semver("0.144"), Some((0, 144, 0)));
        assert_eq!(parse_semver("1"), Some((1, 0, 0)));
    }

    /// The retained JSON Schema bundle must stay present, parseable, and cover
    /// the methods the adapter drives at the floor — it is the machine-readable
    /// half of the contract this module declares.
    #[test]
    fn retained_schema_bundle_is_present_and_covers_core_methods() {
        let raw = include_str!("schema/codex_app_server_protocol.schemas.json");
        let doc: serde_json::Value =
            serde_json::from_str(raw).expect("retained schema bundle must be valid JSON");
        for method in [
            "initialize",
            "initialized",
            "thread/start",
            "thread/resume",
            "thread/fork",
            "turn/start",
            "thread/list",
            "thread/read",
            "thread/turns/list",
            "thread/queue/changed",
        ] {
            assert!(
                raw.contains(method),
                "retained schema must mention `{method}` (regenerate after raising the floor)"
            );
        }
        assert!(doc.is_object(), "schema bundle should be a JSON object");
    }
}
