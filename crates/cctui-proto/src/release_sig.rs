//! Release-asset authenticity: every published binary ships a detached
//! minisign signature (`<asset>.minisig`) made with the release key below.

use minisign_verify::{PublicKey, Signature};

pub const RELEASE_PUBLIC_KEY: &str = include_str!("../../../keys/release.minisign.pub");

pub const SIG_SUFFIX: &str = ".minisig";

/// Verify `bytes` against a `.minisig` document using the embedded release key.
pub fn verify(bytes: &[u8], minisig: &str) -> Result<(), String> {
    verify_with(RELEASE_PUBLIC_KEY, bytes, minisig)
}

pub fn verify_with(public_key: &str, bytes: &[u8], minisig: &str) -> Result<(), String> {
    let pk = PublicKey::decode(public_key.trim()).map_err(|e| format!("release key: {e}"))?;
    let sig = Signature::decode(minisig).map_err(|e| format!("malformed signature: {e}"))?;
    pk.verify(bytes, &sig, false).map_err(|e| format!("signature verification failed: {e}"))
}

/// `true` when `candidate` is strictly older than `running`. Unparseable
/// versions count as older so they are never installed silently.
#[must_use]
pub fn is_downgrade(running: &str, candidate: &str) -> bool {
    match (semver::Version::parse(running), semver::Version::parse(candidate)) {
        (Ok(r), Ok(c)) => c < r,
        _ => true,
    }
}

/// Release channel: `vX.Y.Z-beta.N` tags ship as beta pre-releases, `vX.Y.Z` as stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    /// Any semver pre-release is beta; unparseable versions are too, so they
    /// never reach a stable machine.
    #[must_use]
    pub fn of_version(version: &str) -> Self {
        match semver::Version::parse(version) {
            Ok(v) if v.pre.is_empty() => Self::Stable,
            _ => Self::Beta,
        }
    }
}

impl std::str::FromStr for Channel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            other => Err(format!("unknown release channel {other:?} (expected stable or beta)")),
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    Install,
    UpToDate,
    /// The offered build is beta and this machine follows stable.
    WrongChannel,
    Downgrade,
}

/// Whether a machine on `machine` running `running` should install `candidate`.
/// A beta machine takes stable builds too, so it rolls forward onto the
/// matching stable release; going back to an *older* stable is a downgrade
/// and needs `allow_downgrade`.
#[must_use]
pub fn update_decision(
    running: &str,
    candidate: &str,
    machine: Channel,
    allow_downgrade: bool,
) -> UpdateDecision {
    if candidate == running {
        return UpdateDecision::UpToDate;
    }
    if machine == Channel::Stable && Channel::of_version(candidate) == Channel::Beta {
        return UpdateDecision::WrongChannel;
    }
    if is_downgrade(running, candidate) && !allow_downgrade {
        return UpdateDecision::Downgrade;
    }
    UpdateDecision::Install
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOREIGN_SIG: &str = "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1633700835\tfile:test\tprehashed
wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==
";

    #[test]
    fn embedded_key_decodes() {
        PublicKey::decode(RELEASE_PUBLIC_KEY.trim()).expect("release key must parse");
    }

    #[test]
    fn rejects_garbage_and_foreign_signatures() {
        assert!(verify(b"binary", "not a signature").is_err());
        assert!(verify(b"binary", FOREIGN_SIG).is_err());
    }

    #[test]
    fn downgrade_detection() {
        assert!(is_downgrade("0.20.0", "0.19.9"));
        assert!(!is_downgrade("0.20.0", "0.20.0"));
        assert!(!is_downgrade("0.20.0", "0.20.1"));
        assert!(is_downgrade("0.20.0", "garbage"));
        assert!(is_downgrade("0.20.0", "0.20.0-beta.3"));
        assert!(!is_downgrade("0.20.0-beta.3", "0.20.0"));
        assert!(!is_downgrade("0.20.0-beta.2", "0.20.0-beta.10"));
    }

    #[test]
    fn channel_follows_the_version() {
        assert_eq!(Channel::of_version("0.20.0"), Channel::Stable);
        assert_eq!(Channel::of_version("0.20.0-beta.1"), Channel::Beta);
        assert_eq!(Channel::of_version("garbage"), Channel::Beta);
        assert_eq!("BETA".parse::<Channel>(), Ok(Channel::Beta));
        assert_eq!(" stable ".parse::<Channel>(), Ok(Channel::Stable));
        assert!("nightly".parse::<Channel>().is_err());
        assert_eq!(serde_json::to_string(&Channel::Beta).unwrap(), "\"beta\"");
    }

    #[test]
    fn update_decision_matrix() {
        use Channel::{Beta, Stable};
        use UpdateDecision::{Downgrade, Install, UpToDate, WrongChannel};
        let cases = [
            ("0.20.0", "0.20.0", Stable, false, UpToDate),
            ("0.20.0", "0.20.1", Stable, false, Install),
            ("0.20.0", "0.21.0-beta.1", Stable, false, WrongChannel),
            ("0.20.0", "0.21.0-beta.1", Stable, true, WrongChannel),
            ("0.20.0", "0.19.0", Stable, false, Downgrade),
            ("0.20.0", "0.19.0", Stable, true, Install),
            ("0.20.0", "0.21.0-beta.1", Beta, false, Install),
            ("0.20.0", "0.20.1", Beta, false, Install),
            ("0.21.0-beta.1", "0.21.0-beta.2", Beta, false, Install),
            ("0.21.0-beta.2", "0.21.0-beta.1", Beta, false, Downgrade),
            ("0.21.0-beta.1", "0.21.0", Beta, false, Install),
            ("0.21.0-beta.1", "0.21.0", Stable, false, Install),
            ("0.21.0-beta.1", "0.20.0", Stable, false, Downgrade),
            ("0.21.0-beta.1", "0.20.0", Stable, true, Install),
            ("0.21.0-beta.1", "0.20.0", Beta, true, Install),
        ];
        for (running, candidate, machine, allow, want) in cases {
            assert_eq!(
                update_decision(running, candidate, machine, allow),
                want,
                "{running} -> {candidate} on {machine} (allow_downgrade={allow})"
            );
        }
    }
}
