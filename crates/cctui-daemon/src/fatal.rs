//! Non-retryable configuration errors and their distinct exit code.
//!
//! Under launchd/systemd a plain `exit 1` looks identical to a transient
//! crash and gets respawned. Errors only an operator can fix — not enrolled,
//! malformed config, a rejected machine key — exit with [`EXIT_CONFIG`]
//! (sysexits.h `EX_CONFIG`) instead, so `launchctl print` / unit status makes
//! the "fix the config" case distinguishable from a crash.

use std::fmt;

pub const EXIT_CONFIG: i32 = 78;

/// Marker context attached to errors that retrying can never fix.
#[derive(Debug, Clone, Copy)]
pub struct ConfigFatal;

impl fmt::Display for ConfigFatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("non-retryable configuration error")
    }
}

#[must_use]
pub fn mark(err: anyhow::Error) -> anyhow::Error {
    err.context(ConfigFatal)
}

#[must_use]
pub fn is_config_fatal(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ConfigFatal>().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marked_errors_classify_as_config_fatal() {
        let err = mark(anyhow::anyhow!("machine key rejected"));
        assert!(is_config_fatal(&err));
        assert!(err.to_string().contains("non-retryable"));
    }

    #[test]
    fn plain_errors_do_not() {
        assert!(!is_config_fatal(&anyhow::anyhow!("connection refused")));
    }
}
