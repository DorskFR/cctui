//! `POST /api/v1/version/self-update` — how the deployment update was taken.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// Which of the two paths took the job, and what to watch as a result.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SelfUpdateResponse {
    /// The machine's own update command is running. There is no session to
    /// open: progress is the run, polled from `GET /version/self-update`.
    Hook {
        run_id: uuid::Uuid,
        /// Version the machine was asked to deploy.
        version: String,
    },
    /// No hook on the target machine, so an agent got the job.
    Agent {
        /// Spawn command id, to await on the websocket like a manual spawn.
        command_id: uuid::Uuid,
        /// The id the new session registers under (claude-code pre-mints it),
        /// so the webui can jump to it; `null` for adapters that mint their own.
        session_id: Option<uuid::Uuid>,
        /// Version the agent was asked to deploy.
        version: String,
        /// Account the spawn bound (see `SpawnResponse::account`).
        account: Option<String>,
    },
}
