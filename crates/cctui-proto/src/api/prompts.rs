//! `/api/v1/prompts` — prompt CRUD + repo-scoped resolution.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct Prompt {
    pub id: Uuid,
    pub name: String,
    pub content: String,
    pub description: Option<String>,
    /// Purpose tag: `general` (default) or `review` (a "Review with agent"
    /// prompt). The resolver filters on this so review-prompt scoping never
    /// collides with ordinary prompts.
    pub kind: String,
    /// GitHub owner this prompt is scoped to, or `None` for a global prompt.
    pub scope_owner: Option<String>,
    /// Repo name (within `scope_owner`) this prompt is scoped to. Requires
    /// `scope_owner`; `None` means owner-wide (or global, when owner is also
    /// `None`).
    pub scope_repo: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
