//! Ownership queries for shared execution resources.
use sqlx::PgPool;
use uuid::Uuid;
/// A resource kind that can be shared via `resource_shares`, keyed by its
/// `resource_type` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shareable {
    Account,
    Machine,
    Dispatcher,
    /// Accepted so the share table/routes are ready; it has no backing table
    /// yet, so its owner is always unknown.
    ContextPack,
}

impl Shareable {
    #[must_use]
    pub const fn as_share_type(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Machine => "machine",
            Self::Dispatcher => "dispatcher",
            Self::ContextPack => "context_pack",
        }
    }
}

impl std::str::FromStr for Shareable {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "account" => Ok(Self::Account),
            "machine" => Ok(Self::Machine),
            "dispatcher" => Ok(Self::Dispatcher),
            "context_pack" => Ok(Self::ContextPack),
            _ => Err(()),
        }
    }
}

/// The owning user of a directly-owned resource, or `None` when it does not
/// exist. The only place these owner lookups are written.
pub async fn shareable_owner(
    kind: Shareable,
    id: Uuid,
    pool: &PgPool,
) -> Result<Option<Uuid>, sqlx::Error> {
    let sql = match kind {
        Shareable::Account => "SELECT user_id FROM accounts WHERE id = $1",
        Shareable::Machine => "SELECT user_id FROM machines WHERE id = $1",
        Shareable::Dispatcher => {
            "SELECT user_id FROM dispatchers WHERE id = $1 AND deleted_at IS NULL"
        }
        Shareable::ContextPack => return Ok(None),
    };
    cctui_telemetry::db::traced(sql, |sql| sqlx::query_scalar(sql).bind(id).fetch_optional(pool))
        .await
}
