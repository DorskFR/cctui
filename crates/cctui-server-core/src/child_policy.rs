//! Capability checks and atomic reservations for child execution trees.
use crate::error::AppError;
use axum::http::StatusCode;
use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::{SpawnCapability, SpawnChildRequest};
fn deny(code: StatusCode, msg: impl Into<String>) -> AppError {
    AppError::new(code, msg)
}
/// What the parent's tree has already consumed when a child is requested.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub live_children: u32,
    /// Sum of budgets already granted to descendants of the tree root.
    pub tree_granted_usd: f64,
    /// The parent's live posture, when known.
    pub parent_mode: Option<PermissionMode>,
}

/// A spawn request that cleared the capability check.
#[derive(Debug, PartialEq)]
pub struct Authorized {
    pub adapter: String,
    pub budget_usd: Option<f64>,
    pub permission_mode: PermissionMode,
}

/// Why a `CctuiAgent` call was refused. Rendered verbatim into the tool result,
/// so each variant names the limit that stopped it.
#[derive(Debug, PartialEq)]
pub enum Denied {
    NoCapability,
    Adapter { requested: String, allowed: Vec<String> },
    Budget { requested: f64, max: f64 },
    TooManyChildren { max: u32 },
    Depth,
    PermissionMode { requested: PermissionMode, max: PermissionMode },
    TreeBudget { requested: f64, remaining: f64 },
    BadRequest(String),
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCapability => {
                f.write_str("this session has no spawn capability — CctuiAgent is disabled for it")
            }
            Self::Adapter { requested, allowed } => write!(
                f,
                "adapter {requested:?} is not permitted for this session (allowed: {})",
                allowed.join(", ")
            ),
            Self::Budget { requested, max } => {
                write!(f, "budget_usd {requested} exceeds this session's ceiling {max}")
            }
            Self::TooManyChildren { max } => write!(
                f,
                "this session already has its maximum of {max} children — archive finished \
                 children with CctuiAgentArchive to free slots"
            ),
            Self::Depth => f.write_str("this session is at its maximum spawn depth"),
            Self::PermissionMode { requested, max } => write!(
                f,
                "permission_mode {} is more permissive than this session's {}",
                requested.normalized_label(),
                max.normalized_label()
            ),
            Self::TreeBudget { requested, remaining } => write!(
                f,
                "budget_usd {requested} exceeds the {remaining} left in this spawn tree's budget"
            ),
            Self::BadRequest(msg) => f.write_str(msg),
        }
    }
}

/// Decide whether `req` is permitted by `cap`, and with what budget.
///
/// Pure and fail-closed: an absent capability, an unlisted adapter, a budget
/// over the per-child or remaining tree ceiling, a child count at the cap, an
/// exhausted depth, or a posture above the parent's all deny. A call that names
/// no budget inherits the smaller of the ceiling and what the tree has left; one
/// that names no posture inherits the parent's. An unset ceiling (per-child,
/// child count or tree) is unlimited, never a denial.
#[must_use = "authorization must be checked before spawning"]
pub fn authorize(
    cap: Option<&SpawnCapability>,
    req: &SpawnChildRequest,
    usage: &Usage,
) -> Result<Authorized, Denied> {
    let Some(cap) = cap.filter(|c| !c.is_empty()) else {
        return Err(Denied::NoCapability);
    };
    let adapter = req.adapter.trim();
    if adapter.is_empty() {
        return Err(Denied::BadRequest("adapter is required".into()));
    }
    if req.prompt.trim().is_empty() {
        return Err(Denied::BadRequest("prompt is required".into()));
    }
    if !cap.allows_adapter(adapter) {
        return Err(Denied::Adapter {
            requested: adapter.to_owned(),
            allowed: cap.adapters.clone(),
        });
    }
    if cap.max_depth == Some(0) {
        return Err(Denied::Depth);
    }
    if let Some(max) = cap.max_children
        && usage.live_children >= max
    {
        return Err(Denied::TooManyChildren { max });
    }
    let ceiling = match (cap.max_permission_mode, usage.parent_mode) {
        (Some(c), Some(p)) => PermissionMode::stricter(p, c),
        (c, p) => p.or(c).unwrap_or(PermissionMode::Ask),
    };
    let permission_mode = match req.permission_mode {
        Some(m) if !m.within(ceiling) => {
            return Err(Denied::PermissionMode { requested: m, max: ceiling });
        }
        Some(m) => m,
        None => ceiling,
    };
    let budget = match req.budget_usd {
        None => cap.max_budget_usd,
        Some(b) if !b.is_finite() || b <= 0.0 => {
            return Err(Denied::BadRequest("budget_usd must be a positive number".into()));
        }
        Some(b) => match cap.max_budget_usd {
            Some(max) if b > max => return Err(Denied::Budget { requested: b, max }),
            _ => Some(b),
        },
    };
    let budget = match cap.max_tree_budget_usd {
        None => budget,
        Some(tree) => {
            let remaining = (tree - usage.tree_granted_usd).max(0.0);
            match (req.budget_usd, budget) {
                (Some(b), _) if b > remaining => {
                    return Err(Denied::TreeBudget { requested: b, remaining });
                }
                _ if remaining <= 0.0 => {
                    return Err(Denied::TreeBudget { requested: budget.unwrap_or(0.0), remaining });
                }
                (_, Some(b)) => Some(b.min(remaining)),
                (_, None) => Some(remaining),
            }
        }
    };
    Ok(Authorized { adapter: adapter.to_owned(), budget_usd: budget, permission_mode })
}

/// Reservations for `parent_id` whose child has not registered yet: its
/// capability is still keyed by the spawn key and no session row exists.
pub async fn pending_child_count(
    exec: impl sqlx::PgExecutor<'_>,
    parent_id: &str,
) -> Result<u32, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM spawn_tree_grants g \
         WHERE g.parent_id = $1 \
         AND EXISTS (SELECT 1 FROM session_spawn_capabilities c WHERE c.session_id = g.child_id) \
         AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = g.child_id)",
    )
    .bind(parent_id)
    .fetch_one(exec)
    .await?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Sum of the budgets ever granted to descendants of `root`.
pub async fn tree_granted_usd(
    exec: impl sqlx::PgExecutor<'_>,
    root: &str,
) -> Result<f64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(budget_usd), 0)::float8 FROM spawn_tree_grants WHERE root_id = $1",
    )
    .bind(root)
    .fetch_one(exec)
    .await
}

/// Return a reservation whose child never launched to the tree budget.
pub async fn release_child(pool: &sqlx::PgPool, child_key: &str) {
    let released = async {
        crate::store::spawn_capabilities::delete(pool, child_key).await?;
        sqlx::query("DELETE FROM spawn_tree_grants WHERE child_id = $1")
            .bind(child_key)
            .execute(pool)
            .await
            .map(|_| ())
    };
    if let Err(e) = released.await {
        tracing::error!(child = %child_key, error = %e, "spawn-child reservation release failed");
    }
}

/// Authorize and reserve one child atomically.
///
/// Count children and persist the grant under a per-tree transaction lock, so
/// concurrent spawns cannot claim the same slot or slice of the tree budget.
pub async fn reserve_child(
    counter: &impl ChildCount,
    pool: &sqlx::PgPool,
    parent_id: &str,
    cap: Option<&SpawnCapability>,
    req: &SpawnChildRequest,
    mut usage: Usage,
    child_key: &str,
) -> Result<(Authorized, SpawnCapability), AppError> {
    let root = cap.and_then(|c| c.tree_root.clone()).unwrap_or_else(|| parent_id.to_owned());
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("spawn-tree:{root}"))
        .execute(&mut *tx)
        .await?;
    usage.tree_granted_usd = tree_granted_usd(&mut *tx, &root).await?;
    usage.live_children = counter
        .live_child_count(&mut tx, parent_id)
        .await?
        .saturating_add(pending_child_count(&mut *tx, parent_id).await?);
    let authorized = authorize(cap, req, &usage).map_err(|d| match d {
        Denied::BadRequest(_) => deny(StatusCode::BAD_REQUEST, d.to_string()),
        _ => deny(StatusCode::FORBIDDEN, d.to_string()),
    })?;
    let child_cap = cap.map_or_else(SpawnCapability::machine_default, |c| {
        c.inherited(parent_id, authorized.budget_usd, Some(authorized.permission_mode))
    });
    crate::store::spawn_capabilities::upsert(&mut *tx, child_key, &child_cap).await?;
    sqlx::query(
        "INSERT INTO spawn_tree_grants (root_id, parent_id, child_id, budget_usd) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&root)
    .bind(parent_id)
    .bind(child_key)
    .bind(tree_grant_usd(cap, &authorized))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((authorized, child_cap))
}

/// Dollars a child's reservation draws from its tree. An unlimited tree draws
/// nothing: the row only holds the child's slot until it registers.
#[must_use]
pub fn tree_grant_usd(cap: Option<&SpawnCapability>, authorized: &Authorized) -> f64 {
    if cap.is_some_and(|c| c.max_tree_budget_usd.is_some()) {
        authorized.budget_usd.unwrap_or(0.0)
    } else {
        0.0
    }
}

/// Supplies the host's completion evidence under the reservation transaction.
pub trait ChildCount: Sync {
    fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent_id: &str,
    ) -> impl std::future::Future<Output = Result<u32, sqlx::Error>> + Send;
}

/// Counts execution state without requiring a transcript archive.
pub struct ExecutionChildCount;
impl ChildCount for ExecutionChildCount {
    async fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent_id: &str,
    ) -> Result<u32, sqlx::Error> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sessions s WHERE s.parent_id = $1 \
             AND COALESCE(s.metadata->>'relation', '') NOT IN ('fork', 'followup') \
             AND s.status <> 'archived' \
             AND (s.end_reason IS NULL OR s.end_reason = 'completed')",
        )
        .bind(parent_id)
        .fetch_one(conn)
        .await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests;
