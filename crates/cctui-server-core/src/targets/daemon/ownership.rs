use std::collections::HashSet;

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    Mine,
    Foreign,
    Absent,
}

const MAX_CACHED_OWNERS: usize = 10_000;

/// Which sessions a daemon connection's machine owns: the row's `machine_uuid`
///
/// and `user_id` both match. A NULL on either is foreign. Only settled answers
/// are cached; an absent row may be created by this connection's next upsert.
pub struct SessionOwners {
    machine_id: Uuid,
    user_id: Uuid,
    known: std::collections::HashMap<String, bool>,
    warned: HashSet<String>,
}

impl SessionOwners {
    #[must_use]
    pub fn new(machine_id: Uuid, user_id: Uuid) -> Self {
        Self {
            machine_id,
            user_id,
            known: std::collections::HashMap::new(),
            warned: HashSet::new(),
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
