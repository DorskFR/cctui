use std::collections::HashSet;

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    Mine,
    Foreign,
    Absent,
}

const MAX_CACHED_OWNERS: usize = 10_000;

/// Which sessions a daemon connection's machine owns.
///
/// The row's `machine_uuid` and `user_id` must both match; a NULL on either is
/// foreign. A connection opened
/// with a session-bound worker key owns only that session and the ones it
/// spawned. Only settled answers are cached; an absent row may be created by
/// this connection's next upsert.
pub struct SessionOwners {
    machine_id: Uuid,
    user_id: Uuid,
    bound: Option<String>,
    known: std::collections::HashMap<String, bool>,
    warned: HashSet<String>,
}

impl SessionOwners {
    #[must_use]
    pub fn new(machine_id: Uuid, user_id: Uuid) -> Self {
        Self {
            machine_id,
            user_id,
            bound: None,
            known: std::collections::HashMap::new(),
            warned: HashSet::new(),
        }
    }

    /// Confine this connection to `root`'s spawn tree.
    #[must_use]
    pub fn bound_to(mut self, root: Option<String>) -> Self {
        self.bound = root;
        self
    }

    /// Whether a bound connection may act on `local_id` at all; always true
    /// for an unbound one. A lookup failure refuses.
    pub async fn reaches(&self, pool: &sqlx::PgPool, local_id: &str) -> bool {
        self.in_tree(pool, local_id).await.unwrap_or_else(|err| {
            tracing::warn!(%err, %local_id, "worker session lineage lookup failed; frame dropped");
            false
        })
    }

    async fn in_tree(&self, pool: &sqlx::PgPool, local_id: &str) -> Result<bool, sqlx::Error> {
        match &self.bound {
            None => Ok(true),
            Some(root) if root == local_id => Ok(true),
            Some(root) => {
                cctui_server_kernel::auth::session_descends_from(pool, local_id, root).await
            }
        }
    }

    pub async fn resolve(
        &mut self,
        pool: &sqlx::PgPool,
        local_id: &str,
    ) -> Result<Ownership, sqlx::Error> {
        if let Some(&mine) = self.known.get(local_id) {
            return Ok(if mine { Ownership::Mine } else { Ownership::Foreign });
        }
        if !self.in_tree(pool, local_id).await? {
            return Ok(Ownership::Foreign);
        }
        let row: Option<(Option<Uuid>, Option<Uuid>)> =
            sqlx::query_as("SELECT machine_uuid, user_id FROM sessions WHERE id = $1")
                .bind(local_id)
                .fetch_optional(pool)
                .await?;
        let Some((machine, user)) = row else { return Ok(Ownership::Absent) };
        let mine = machine == Some(self.machine_id) && user == Some(self.user_id);
        if self.known.len() >= MAX_CACHED_OWNERS {
            self.known.clear();
            self.warned.clear();
        }
        self.known.insert(local_id.to_owned(), mine);
        Ok(if mine { Ownership::Mine } else { Ownership::Foreign })
    }
}

/// Whether a frame scoped to `local_id` may be processed for this connection.
/// A lookup failure refuses.
pub async fn admit(owners: &mut SessionOwners, pool: &sqlx::PgPool, local_id: &str) -> bool {
    match owners.resolve(pool, local_id).await {
        Ok(Ownership::Mine | Ownership::Absent) => true,
        Ok(Ownership::Foreign) => {
            if owners.warned.insert(local_id.to_owned()) {
                tracing::warn!(
                    machine_id = %owners.machine_id,
                    %local_id,
                    "dropping daemon frames for a session another machine owns",
                );
            } else {
                tracing::debug!(%local_id, "dropping daemon frame for a foreign session");
            }
            false
        }
        Err(err) => {
            tracing::warn!(%err, %local_id, "session ownership lookup failed; frame dropped");
            false
        }
    }
}

/// Settle an announcement once its upsert ran: keep the session bound to this
/// connection only if the row is now this machine's, otherwise release the
/// binding.
pub async fn claim_announced(
    owners: &mut SessionOwners,
    pool: &sqlx::PgPool,
    bus: &crate::bus::Bus,
    conn_id: Uuid,
    local_id: &str,
) -> bool {
    if matches!(owners.resolve(pool, local_id).await, Ok(Ownership::Mine)) {
        bus.bind_session_conn(local_id, conn_id);
        true
    } else {
        bus.unbind_session_conn(local_id, conn_id);
        false
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{SessionOwners, admit, claim_announced};
    use crate::routes::daemon::test_support::{drop_machines, seed_machine, seed_owned_session};

    async fn grant(pool: &sqlx::PgPool, root: &str, parent: &str, child: &str) {
        sqlx::query(
            "INSERT INTO spawn_tree_grants (root_id, parent_id, child_id, budget_usd) \
             VALUES ($1, $2, $3, 0)",
        )
        .bind(root)
        .bind(parent)
        .bind(child)
        .execute(pool)
        .await
        .expect("seed grant");
    }

    #[tokio::test]
    async fn a_bound_worker_connection_owns_only_its_own_session_tree() {
        let name = "a_bound_worker_connection_owns_only_its_own_session_tree";
        let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "dispatch").await;
        let own = seed_owned_session(&pool, uid, mid).await;
        let child = seed_owned_session(&pool, uid, mid).await;
        let sibling = seed_owned_session(&pool, uid, mid).await;
        let unborn_child = Uuid::new_v4().to_string();
        grant(&pool, &own, &own, &child).await;
        grant(&pool, &own, &child, &unborn_child).await;

        let mut worker = SessionOwners::new(mid, uid).bound_to(Some(own.clone()));
        assert!(admit(&mut worker, &pool, &own).await, "its own session");
        assert!(admit(&mut worker, &pool, &child).await, "a session it spawned");
        assert!(admit(&mut worker, &pool, &unborn_child).await, "a descendant not yet upserted");
        assert!(
            !admit(&mut worker, &pool, &sibling).await,
            "another dispatched session of the same user on the shared machine"
        );
        assert!(!admit(&mut worker, &pool, &Uuid::new_v4().to_string()).await);
        assert!(worker.reaches(&pool, &child).await);
        assert!(!worker.reaches(&pool, &sibling).await);

        let bus =
            cctui_server_kernel::bus::Bus::new(Box::new(cctui_server_kernel::bus::NoopTransport));
        let conn = Uuid::new_v4();
        assert!(!claim_announced(&mut worker, &pool, &bus, conn, &sibling).await);
        assert!(claim_announced(&mut worker, &pool, &bus, conn, &own).await);

        let mut daemon = SessionOwners::new(mid, uid);
        assert!(admit(&mut daemon, &pool, &sibling).await, "an unbound key keeps machine scope");
        assert!(daemon.reaches(&pool, &sibling).await);

        sqlx::query("DELETE FROM spawn_tree_grants WHERE root_id = $1")
            .bind(&own)
            .execute(&pool)
            .await
            .ok();
        drop_machines(&pool, &[own, child, sibling], &[(uid, mid)]).await;
    }
}
