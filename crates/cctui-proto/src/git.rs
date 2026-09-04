//! Git facts about a directory on a daemon's machine, read from `.git`
//! metadata (no `git` subprocess unless `dirty` is requested).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Reply payload of `GET /api/v1/machines/{id}/fs/gitinfo` and of the
/// daemon's `GitInfoResult` frame.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitInfo {
    /// `false` when `path` has no `.git` → every other field is empty.
    pub is_repo: bool,
    /// Checked-out branch (`refs/heads/` stripped); `None` when detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Full commit SHA when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_sha: Option<String>,
    /// `Some` only when the caller asked for it (`git status --porcelain`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    /// `.git` is a `gitdir:` file — a linked worktree, not the main checkout.
    #[serde(default)]
    pub is_worktree: bool,
}
