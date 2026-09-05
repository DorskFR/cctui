//! The deterministic update hook: the shapes a daemon and the server exchange
//! while a deployment updates *itself*, without an agent in the loop.
//!
//! The contract is deliberately thin. The server knows nothing about how this
//! deployment is updated; the machine does, as one shell command it was
//! configured with (`CCTUI_UPDATE_COMMAND`). The server asks for a run, the
//! daemon reports the phases it goes through, and the answer is durable
//! because the server it reports to is usually a *different process* than the
//! one that asked (the update restarts it).
//!
//! See `docs/update-hook.md` for the deployment-side contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Tail of the hook's merged stdout/stderr kept for the UI. Enough to see why
/// a `kubectl`/`compose` invocation failed, small enough to store per run.
pub const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// Where a hook run got to.
///
/// The happy path is `Running` → `Verifying` → `Succeeded`. Any failure goes
/// to `RollingBack` → `RolledBack` when a rollback command is configured, and
/// straight to `Failed` when none is (or when the rollback itself failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UpdateHookPhase {
    /// The daemon accepted the run and the update command is executing.
    Running,
    /// The update command exited 0; the daemon is polling the health endpoint
    /// until it reports the target version.
    Verifying,
    /// The health check saw the target version. Terminal, and the only phase
    /// that means the deployment actually updated.
    Succeeded,
    /// The run failed and the rollback command is executing.
    RollingBack,
    /// The rollback command finished. Terminal.
    RolledBack,
    /// The run failed with no rollback to fall back on. Terminal.
    Failed,
}

impl UpdateHookPhase {
    /// Whether no further report is expected for this run.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::RolledBack | Self::Failed)
    }

    /// Whether the deployment ended up on the version it was asked to deploy.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Succeeded)
    }

    /// Wire form, also what the server stores in `self_update_runs.phase`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::Succeeded => "succeeded",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::Failed => "failed",
        }
    }

    /// Parse the stored form back; unknown strings read as [`Self::Failed`]
    /// rather than poisoning a listing (a run we cannot interpret is not a
    /// run that succeeded).
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "verifying" => Self::Verifying,
            "succeeded" => Self::Succeeded,
            "rolling_back" => Self::RollingBack,
            "rolled_back" => Self::RolledBack,
            _ => Self::Failed,
        }
    }
}

/// One progress report from the daemon about a hook run, the body of
/// `POST /api/v1/daemon/update-hook/{run_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateHookReport {
    pub phase: UpdateHookPhase,
    /// Exit status of the command this phase ran, once it has exited.
    pub exit_code: Option<i32>,
    /// One line for the UI: what just happened, in plain words.
    pub detail: String,
    /// Tail of the merged stdout/stderr, capped at [`OUTPUT_TAIL_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tail: Option<String>,
}

/// Keep only the last [`OUTPUT_TAIL_BYTES`] of `out`, cut on a char boundary
/// and prefixed with an ellipsis when truncated.
#[must_use]
pub fn tail(out: &str) -> String {
    if out.len() <= OUTPUT_TAIL_BYTES {
        return out.to_owned();
    }
    let mut cut = out.len() - OUTPUT_TAIL_BYTES;
    while cut < out.len() && !out.is_char_boundary(cut) {
        cut += 1;
    }
    format!("…\n{}", &out[cut..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_succeeded_means_the_deployment_moved() {
        for p in [
            UpdateHookPhase::Running,
            UpdateHookPhase::Verifying,
            UpdateHookPhase::RollingBack,
            UpdateHookPhase::RolledBack,
            UpdateHookPhase::Failed,
        ] {
            assert!(!p.is_success(), "{p:?} must not read as a success");
        }
        assert!(UpdateHookPhase::Succeeded.is_success());
    }

    #[test]
    fn terminal_phases_are_the_ones_nothing_follows() {
        assert!(!UpdateHookPhase::Running.is_terminal());
        assert!(!UpdateHookPhase::Verifying.is_terminal());
        assert!(!UpdateHookPhase::RollingBack.is_terminal());
        assert!(UpdateHookPhase::Succeeded.is_terminal());
        assert!(UpdateHookPhase::RolledBack.is_terminal());
        assert!(UpdateHookPhase::Failed.is_terminal());
    }

    #[test]
    fn phase_round_trips_through_its_stored_form() {
        for p in [
            UpdateHookPhase::Running,
            UpdateHookPhase::Verifying,
            UpdateHookPhase::Succeeded,
            UpdateHookPhase::RollingBack,
            UpdateHookPhase::RolledBack,
            UpdateHookPhase::Failed,
        ] {
            assert_eq!(UpdateHookPhase::from_str_lossy(p.as_str()), p);
        }
        // An unreadable row never reads as success.
        assert_eq!(UpdateHookPhase::from_str_lossy("wat"), UpdateHookPhase::Failed);
    }

    #[test]
    fn tail_keeps_the_end_and_stays_on_char_boundaries() {
        let short = "kubectl rollout status: ok";
        assert_eq!(tail(short), short);

        // Multi-byte chars straddling the cut must not panic or split.
        let long = "é".repeat(OUTPUT_TAIL_BYTES);
        let cut = tail(&long);
        assert!(cut.starts_with('…'));
        assert!(cut.len() <= OUTPUT_TAIL_BYTES + 8);
        assert!(long.ends_with(cut.trim_start_matches(['…', '\n'])));
    }
}
