use std::collections::HashSet;

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Mine,
    Foreign,
    Absent,
}

const MAX_CACHED_OWNERS: usize = 10_000;

/// Which sessions a daemon connection's machine owns: the row's `machine_uuid`
/// and `user_id` both match. A NULL on either is foreign. Only settled answers
/// are cached; an absent row may be created by this connection's next upsert.
pub(super) struct SessionOwners {
    machine_id: Uuid,
    user_id: Uuid,
    known: std::collections::HashMap<String, bool>,
    warned: HashSet<String>,
}

impl SessionOwners {
    pub(super) fn new(machine_id: Uuid, user_id: Uuid) -> Self {
        Self {
            machine_id,
            user_id,
            known: std::collections::HashMap::new(),
            warned: HashSet::new(),
        }
    }

    async fn resolve(
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
pub(super) async fn admit(owners: &mut SessionOwners, pool: &sqlx::PgPool, local_id: &str) -> bool {
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
pub(super) async fn claim_announced(
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
    use cctui_proto::adapter::EndReason;
    use cctui_proto::ws::DaemonFrameUp;
    use serde_json::json;

    use super::*;
    use crate::routes::daemon::connection::session_scope;
    use crate::routes::daemon::events::session::persist_session_end;
    use crate::routes::daemon::ingest::insert_event;
    use crate::routes::daemon::registration::upsert_session;
    use crate::routes::daemon::test_support::{
        drop_machines, reply, seed_machine, seed_owned_session,
    };

    #[tokio::test]
    async fn a_foreign_announce_cannot_take_over_a_session() {
        let Some(url) = crate::routes::gateway::test_db_url("foreign_announce") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (ua, ma) = seed_machine(&pool, "owner").await;
        let (ub, mb) = seed_machine(&pool, "intruder").await;
        let sid = seed_owned_session(&pool, ua, ma).await;

        let bus = crate::bus::Bus::new(Box::new(crate::bus::NoopTransport));
        let (tx_a, mut rx_a) = tokio::sync::mpsc::channel(8);
        let (tx_b, mut rx_b) = tokio::sync::mpsc::channel(8);
        let conn_b = Uuid::new_v4();
        bus.register_daemon(ma, Uuid::new_v4(), tx_a);
        bus.register_daemon(mb, conn_b, tx_b);

        let mut intruder = SessionOwners::new(mb, ub);
        let announce = DaemonFrameUp::SessionRegistered {
            adapter_id: "claude-code".into(),
            local_id: sid.clone(),
        };
        assert_eq!(session_scope(&announce), Some(sid.as_str()));
        assert!(!admit(&mut intruder, &pool, &sid).await, "a foreign announce must be dropped");

        bus.bind_session_conn(&sid, conn_b);
        assert!(!claim_announced(&mut intruder, &pool, &bus, conn_b, &sid).await);
        let upserted = upsert_session(&pool, mb, ub, "claude-code", &sid, None, None, None, None)
            .await
            .expect("upsert");
        assert_eq!(upserted, None, "the upsert must refuse a row another machine owns");
        let (machine, user): (Option<Uuid>, Option<Uuid>) =
            sqlx::query_as("SELECT machine_uuid, user_id FROM sessions WHERE id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("read back");
        assert_eq!((machine, user), (Some(ma), Some(ua)));

        bus.command_daemon_for_session(ma, &sid, reply(&sid)).await.expect("route");
        assert!(rx_a.recv().await.is_some(), "the owner still receives its session's frames");
        assert!(rx_b.try_recv().is_err(), "the intruder receives nothing for the session");

        drop_machines(&pool, &[sid], &[(ua, ma), (ub, mb)]).await;
    }

    #[tokio::test]
    async fn another_machines_events_never_reach_the_session() {
        let Some(url) = crate::routes::gateway::test_db_url("foreign_events") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (ua, ma) = seed_machine(&pool, "owner").await;
        let (ub, mb) = seed_machine(&pool, "intruder").await;
        let sid = seed_owned_session(&pool, ua, ma).await;

        let event = |event| DaemonFrameUp::Event { adapter_id: "claude-code".into(), event };
        let frames = [
            event(cctui_proto::adapter::AdapterEvent::Message {
                local_id: sid.clone(),
                payload: json!({ "type": "assistant", "text": "run this" }),
                turn_id: None,
            }),
            event(cctui_proto::adapter::AdapterEvent::SessionEnded {
                local_id: sid.clone(),
                reason: EndReason::Killed,
            }),
            event(cctui_proto::adapter::AdapterEvent::PermissionRequest {
                local_id: sid.clone(),
                request_id: "r1".into(),
                tool: "Bash".into(),
                input: json!({ "command": "true" }),
            }),
            event(cctui_proto::adapter::AdapterEvent::PtyChunk {
                local_id: sid.clone(),
                data: String::new(),
            }),
        ];
        let mut intruder = SessionOwners::new(mb, ub);
        for frame in &frames {
            let scope = session_scope(frame).expect("session-scoped");
            assert!(!admit(&mut intruder, &pool, scope).await, "foreign frame must be dropped");
        }

        let inserted = insert_event(
            &pool,
            mb,
            ub,
            &sid,
            "message",
            json!({ "type": "assistant", "text": "run this" }),
            None,
        )
        .await
        .expect("insert");
        assert_eq!(inserted, None);
        persist_session_end(&pool, mb, ub, &sid, &EndReason::Killed).await.expect("end");

        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stream_events WHERE session_id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("count events");
        assert_eq!(events, 0, "no stream_events row for a foreign machine");
        let status: String = sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .expect("status");
        assert_eq!(status, "active", "a foreign SessionEnded leaves the status unchanged");

        let mut owner = SessionOwners::new(ma, ua);
        assert!(admit(&mut owner, &pool, &sid).await);

        drop_machines(&pool, &[sid], &[(ua, ma), (ub, mb)]).await;
    }

    #[tokio::test]
    async fn a_session_with_no_owner_is_foreign_to_every_machine() {
        let Some(url) = crate::routes::gateway::test_db_url("ownerless_session") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (ua, ma) = seed_machine(&pool, "owner").await;
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, adapter_id) \
             VALUES ($1, $2, $3, '/w', 'active', 'claude-code')",
        )
        .bind(&sid)
        .bind(ma.to_string())
        .bind(ma)
        .execute(&pool)
        .await
        .expect("seed session");

        let mut owners = SessionOwners::new(ma, ua);
        assert_eq!(owners.resolve(&pool, &sid).await.expect("resolve"), super::Ownership::Foreign);
        assert_eq!(
            owners.resolve(&pool, "never-registered").await.expect("resolve"),
            super::Ownership::Absent
        );

        drop_machines(&pool, &[sid], &[(ua, ma)]).await;
    }

    #[tokio::test]
    async fn the_owner_re_announcing_on_a_new_connection_rebinds() {
        let Some(url) = crate::routes::gateway::test_db_url("owner_reannounce") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (ua, ma) = seed_machine(&pool, "owner").await;
        let sid = seed_owned_session(&pool, ua, ma).await;

        let bus = crate::bus::Bus::new(Box::new(crate::bus::NoopTransport));
        let (tx_old, mut rx_old) = tokio::sync::mpsc::channel(8);
        let (tx_new, mut rx_new) = tokio::sync::mpsc::channel(8);
        let (conn_old, conn_new) = (Uuid::new_v4(), Uuid::new_v4());
        bus.register_daemon(ma, conn_old, tx_old);
        bus.bind_session_conn(&sid, conn_old);
        bus.register_daemon(ma, conn_new, tx_new);

        let mut owners = SessionOwners::new(ma, ua);
        assert!(admit(&mut owners, &pool, &sid).await);
        assert!(claim_announced(&mut owners, &pool, &bus, conn_new, &sid).await);

        bus.command_daemon_for_session(ma, &sid, reply(&sid)).await.expect("route");
        assert!(rx_new.recv().await.is_some(), "the new connection receives the session");
        assert!(rx_old.try_recv().is_err());

        drop_machines(&pool, &[sid], &[(ua, ma)]).await;
    }
}
