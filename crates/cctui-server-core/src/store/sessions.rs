use cctui_proto::models::SessionStatus;
use sqlx::PgExecutor;

/// Every value `sessions.status` may hold; migration 139 enforces the same set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRowStatus {
    New,
    Active,
    Inactive,
    Archived,
    Draft,
    Ended,
    Failed,
}

impl SessionRowStatus {
    pub const ALL: &'static [Self] = &[
        Self::New,
        Self::Active,
        Self::Inactive,
        Self::Archived,
        Self::Draft,
        Self::Ended,
        Self::Failed,
    ];
    /// Rows a daemon may still be running.
    pub const RUNNING: &'static [Self] = &[Self::New, Self::Active, Self::Inactive];
    /// Rows a keep-alive tick may be sent to.
    pub const KEEPALIVE: &'static [Self] = &[Self::Active, Self::Inactive];
    /// Rows auto-archive must never touch.
    pub const NOT_ARCHIVABLE: &'static [Self] = &[Self::Archived, Self::Draft];
    /// Rows auto-resume must never touch.
    pub const NOT_RESUMABLE: &'static [Self] =
        &[Self::Archived, Self::Ended, Self::Failed, Self::Draft];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Archived => "archived",
            Self::Draft => "draft",
            Self::Ended => "ended",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.as_str() == s)
    }

    /// The set as bindable text, for `status = ANY($n)` / `status <> ALL($n)`.
    #[must_use]
    pub fn names(set: &[Self]) -> Vec<&'static str> {
        set.iter().map(|s| s.as_str()).collect()
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Archived | Self::Ended | Self::Failed)
    }

    /// Persisted states the API reports as-is instead of re-deriving from
    /// heartbeat age.
    #[must_use]
    pub const fn is_sticky(self) -> bool {
        self.is_terminal() || matches!(self, Self::Draft)
    }

    #[must_use]
    pub const fn to_wire(self) -> SessionStatus {
        match self {
            Self::New => SessionStatus::New,
            Self::Active => SessionStatus::Active,
            Self::Inactive | Self::Ended | Self::Failed => SessionStatus::Inactive,
            Self::Archived => SessionStatus::Archived,
            Self::Draft => SessionStatus::Draft,
        }
    }
}

/// Whether a row changed.
pub async fn set_inactive(
    exec: impl PgExecutor<'_>,
    id: &str,
    require_archived: bool,
) -> Result<bool, sqlx::Error> {
    let sql = if require_archived {
        "UPDATE sessions SET status = 'inactive' WHERE id = $1 AND status = 'archived'"
    } else {
        "UPDATE sessions SET status = 'inactive' WHERE id = $1"
    };
    let res = sqlx::query(sql).bind(id).execute(exec).await?;
    Ok(res.rows_affected() > 0)
}

/// Insert a freshly registered session, or reset an existing row to `new`.
///
/// An existing row is only touched when it belongs to `user_id` on
/// `machine_uuid`; `None` when nothing was written, otherwise whether the row
/// was inserted rather than reset.
pub async fn upsert_registered(
    exec: impl PgExecutor<'_>,
    session: &cctui_proto::models::Session,
    machine_uuid: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Option<bool>, sqlx::Error> {
    let written: Option<bool> = sqlx::query_scalar(
        r"INSERT INTO sessions (id, parent_id, account_id, machine_id, machine_uuid, user_id, working_dir, status, registered_at, last_heartbeat, metadata, model)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'new', $8, $9, $10, NULLIF($10->>'model', ''))
           ON CONFLICT (id) DO UPDATE SET status = 'new', last_heartbeat = $9, metadata = $10, model = COALESCE(sessions.model, EXCLUDED.model)
           WHERE sessions.user_id = EXCLUDED.user_id AND sessions.machine_uuid = EXCLUDED.machine_uuid
           RETURNING (xmax = 0)",
    )
    .bind(&session.id)
    .bind(&session.parent_id)
    .bind(&session.account_id)
    .bind(&session.machine_id)
    .bind(machine_uuid)
    .bind(user_id)
    .bind(&session.working_dir)
    .bind(session.registered_at)
    .bind(session.last_heartbeat)
    .bind(&session.metadata)
    .fetch_optional(exec)
    .await?;
    Ok(written)
}

/// The subset of `ids` whose session runs on a machine owned by `user_id`.
pub async fn visible_session_ids(
    exec: impl PgExecutor<'_>,
    ids: &[String],
    user_id: uuid::Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT s.id FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = ANY($1) AND m.user_id = $2",
    )
    .bind(ids)
    .bind(user_id)
    .fetch_all(exec)
    .await
}

pub async fn adapter_id(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT adapter_id FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}

pub async fn working_dir(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT working_dir FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}
