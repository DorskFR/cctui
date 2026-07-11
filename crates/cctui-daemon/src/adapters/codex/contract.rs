//! Versioned Codex `app-server` protocol contract (CCT-630).
//!
//! Single source of truth for the Codex version cctui is built and tested
//! against. The pinned version is consumed by:
//!
//! - the [`super::app_server`] handshake (declared client version, and the
//!   floor the discovered server version is checked against);
//! - `deploy/worker.Dockerfile` (`ARG CODEX_VERSION`), kept in lockstep by the
//!   `scripts/check-codex-version-drift.sh` CI drift check;
//! - the retained JSON Schema under `schema/`, generated from this exact
//!   Codex build with `codex app-server generate-json-schema --out schema/`.
//!
//! The schema bundle (`schema/codex_app_server_protocol.schemas.json`) pins the
//! method names and shapes the adapter relies on — `initialize`, `initialized`,
//! `thread/start`, `thread/resume`, `thread/fork`, `turn/start`, `thread/list`,
//! `thread/read`, and the approval requests. Regenerate it whenever
//! [`CODEX_PINNED_VERSION`] is bumped.

/// The exact Codex version cctui is pinned to: the version installed by the
/// worker image and the one the retained JSON Schema was generated from. The
/// Dockerfile `ARG CODEX_VERSION` must equal this (CI enforces it).
pub const CODEX_PINNED_VERSION: &str = "0.144.1";

/// The minimum Codex version whose `app-server` protocol the adapter still
/// speaks correctly. Sessions started against an older server keep running but
/// are flagged loudly in diagnostics — the handshake / thread / approval shapes
/// below this floor are not guaranteed.
pub const CODEX_MIN_VERSION: &str = "0.142.0";

/// A parsed `major.minor.patch` triple. Pre-release / build metadata is
/// ignored — the pin only reasons about the release line.
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
        assert!(version_supported(CODEX_PINNED_VERSION));
        assert!(version_supported("0.142.4"));
        assert!(version_supported("1.0.0"));
        assert!(!version_supported("0.141.9"));
        assert!(!version_supported("0.99.0")); // 0.99 < 0.142
        assert!(!version_supported("garbage"));
    }

    #[test]
    fn pinned_is_at_or_above_min() {
        assert!(
            version_supported(CODEX_PINNED_VERSION),
            "pinned {CODEX_PINNED_VERSION} must be >= min {CODEX_MIN_VERSION}"
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
    /// the methods the adapter drives — it is the machine-readable half of the
    /// contract this module pins.
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
        ] {
            assert!(
                raw.contains(method),
                "retained schema must mention `{method}` (regenerate after a Codex bump)"
            );
        }
        assert!(doc.is_object(), "schema bundle should be a JSON object");
    }
}
