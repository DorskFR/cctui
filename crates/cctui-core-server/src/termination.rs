//! Durable standalone stop decisions. Callers hold the host lifecycle lock.
use crate::host::Host;
use cctui_server_core as core;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct Stop {
    pub user_id: Uuid,
    pub machine_id: Option<Uuid>,
    pub end_reason: String,
    pub detail: Option<String>,
}

pub async fn find(host: &Host, session: &str) -> Result<Option<Stop>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id,machine_id,end_reason,detail FROM execution_stops WHERE session_id=$1",
    )
    .bind(session)
    .fetch_optional(&host.pool)
    .await
}

pub async fn stopped(
    host: &Host,
    session: &str,
    user: Uuid,
    machine: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(find(host, session)
        .await?
        .is_some_and(|stop| stop.user_id == user && stop.machine_id.is_none_or(|id| id == machine)))
}

/// All durable cleanup commits with the stop marker, before acknowledging it.
/// `identities` starts with the provisional key, followed by the canonical ID.
pub async fn record(
    host: &Host,
    identities: &[String],
    user: Uuid,
    machine: Option<Uuid>,
    adapter: &str,
    reason: &str,
    detail: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = host.pool.begin().await?;
    let mut identities = identities.to_vec();
    if let Some(machine) = machine
        && let Some(canonical) = identities.last().cloned()
    {
        let aliases:Vec<String>=sqlx::query_scalar("SELECT session_id FROM machine_launches WHERE registered_session_id=$1 AND user_id=$2 AND machine_id=$3")
            .bind(&canonical).bind(user).bind(machine).fetch_all(&mut *tx).await?;
        for alias in aliases {
            if !identities.contains(&alias) {
                identities.insert(identities.len() - 1, alias);
            }
        }
    }
    let revoked_keys =
        record_transaction(&mut tx, &identities, user, machine, adapter, reason, detail).await?;
    tx.commit().await?;
    clear_caches(host, &identities, revoked_keys);
    Ok(())
}

async fn record_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    identities: &[String],
    user: Uuid,
    machine: Option<Uuid>,
    adapter: &str,
    reason: &str,
    detail: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let mut revoked_keys = Vec::<String>::new();
    // Never refund a registered child's historical tree budget.
    let registered: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sessions WHERE id=ANY($1))")
            .bind(identities)
            .fetch_one(&mut **tx)
            .await?;
    for session in identities {
        let (reason, detail):(String,Option<String>)=sqlx::query_as("INSERT INTO execution_stops (session_id,user_id,machine_id,end_reason,detail) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (session_id) DO UPDATE SET session_id=EXCLUDED.session_id WHERE execution_stops.user_id=EXCLUDED.user_id AND (execution_stops.machine_id IS NULL OR execution_stops.machine_id=EXCLUDED.machine_id) RETURNING end_reason,detail")
            .bind(session).bind(user).bind(machine).bind(reason).bind(detail).fetch_one(&mut **tx).await?;
        if let Some(machine) = machine
            && identities.last() == Some(session)
        {
            sqlx::query("INSERT INTO sessions (id,machine_id,machine_uuid,user_id,adapter_id,working_dir,status,ended_at,end_reason,end_detail) VALUES ($1,$2::text,$2,$3,$4,'','ended',now(),$5,$6) ON CONFLICT (id) DO UPDATE SET status='ended',ended_at=COALESCE(sessions.ended_at,now()),end_reason=EXCLUDED.end_reason,end_detail=EXCLUDED.end_detail WHERE sessions.user_id=$3 AND sessions.machine_uuid=$2 AND sessions.status<>'archived'")
                .bind(session).bind(machine).bind(user).bind(adapter).bind(&reason).bind(&detail).execute(&mut **tx).await?;
        }
        core::store::tokens::revoke_by_session(&mut **tx, session).await?;
        core::store::spawn_capabilities::delete(&mut **tx, session).await?;
        if !registered {
            sqlx::query("DELETE FROM spawn_tree_grants WHERE child_id=$1")
                .bind(session)
                .execute(&mut **tx)
                .await?;
        }
        revoked_keys.extend(sqlx::query_scalar::<_,String>("UPDATE auth_keys SET revoked_at=now() WHERE session_id=$1 AND kind='ephemeral' AND revoked_at IS NULL RETURNING key_hash").bind(session).fetch_all(&mut **tx).await?);
        sqlx::query("UPDATE machine_launches SET state='refused',detail=$2,updated_at=now() WHERE (session_id=$1 OR registered_session_id=$1) AND user_id=$3")
            .bind(session).bind(&detail).bind(user).execute(&mut **tx).await?;
        sqlx::query("UPDATE dispatch_records SET outcome='failed',end_reason=$2,detail=$3,queued=false WHERE session_id=$1 AND user_id=$4")
            .bind(session).bind(&reason).bind(&detail).bind(user).execute(&mut **tx).await?;
    }
    Ok(revoked_keys)
}

fn clear_caches(host: &Host, identities: &[String], revoked_keys: Vec<String>) {
    for hash in revoked_keys {
        host.auth_config.purge(&hash);
    }
    for session in identities {
        host.spawn_capabilities.remove(session);
        host.session_usd_budgets.remove(session);
    }
}

pub async fn stop_worker(host: &Host, machine: Uuid, session: &str, adapter: &str) {
    let delivery = host.bus.command_daemon_for_session(
        machine,
        session,
        cctui_proto::ws::DaemonFrameDown::Command {
            adapter_id: adapter.to_owned(),
            command: Box::new(cctui_proto::adapter::AdapterCommand::Kill {
                local_id: session.to_owned(),
                signal: None,
            }),
            trace: None,
        },
    );
    match tokio::time::timeout(std::time::Duration::from_secs(2), delivery).await {
        Ok(Ok(())) => {}
        result => {
            tracing::warn!(?result,%session,"stop persisted; worker stop could not be delivered");
        }
    }
}

/// Unknown delivery is terminal after ten minutes; never resend a launch.
pub async fn sweep(host: &Host) -> Result<(), sqlx::Error> {
    let pending:Vec<String>=sqlx::query_scalar("SELECT session_id FROM machine_launches WHERE state IN ('prepared','delivered','accepted') AND created_at < now()-($1 * interval '1 second') ORDER BY created_at LIMIT 100")
        .bind(core::commands::PENDING_COMMAND_TTL.as_secs_f64()).fetch_all(&host.pool).await?;
    for session in pending {
        let guard = host.lifecycle.lock().await;
        let Some(launch) = core::machine_launches::find(&host.pool, &session, None).await? else {
            continue;
        };
        if !matches!(launch.state.as_str(), "prepared" | "delivered" | "accepted") {
            continue;
        }
        record(
            host,
            std::slice::from_ref(&session),
            launch.user_id,
            Some(launch.machine_id),
            &launch.adapter_id,
            "spawn_failed",
            "launch registration deadline exceeded; delivery unknown; not resent",
        )
        .await?;
        drop(guard);
        stop_worker(host, launch.machine_id, &session, &launch.adapter_id).await;
    }
    reconcile_dispatches(host).await
}

async fn reconcile_dispatches(host: &Host) -> Result<(), sqlx::Error> {
    let pending:Vec<String>=sqlx::query_scalar("SELECT r.session_id FROM dispatch_records r WHERE r.user_id IS NOT NULL AND r.outcome IN ('pending','forwarded') AND NOT r.queued AND (CASE WHEN r.outcome='pending' THEN r.created_at ELSE r.started_at END) < now()-($1 * interval '1 minute') AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id=r.session_id) ORDER BY r.created_at LIMIT 100")
        .bind(f64::from(core::store::dispatch_records::REGISTER_TIMEOUT_MINUTES)).fetch_all(&host.pool).await?;
    for session in pending {
        let _guard = host.lifecycle.lock().await;
        let mut tx = host.pool.begin().await?;
        // Lock the dispatch record through cleanup so an in-flight queue update
        // cannot move its registration clock between the decision and commit.
        let row:Option<(Uuid,Option<String>)>=sqlx::query_as("SELECT r.user_id,r.adapter FROM dispatch_records r WHERE r.session_id=$1 AND r.outcome IN ('pending','forwarded') AND NOT r.queued AND (CASE WHEN r.outcome='pending' THEN r.created_at ELSE r.started_at END) < now()-($2 * interval '1 minute') AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id=r.session_id) FOR UPDATE")
            .bind(&session).bind(f64::from(core::store::dispatch_records::REGISTER_TIMEOUT_MINUTES)).fetch_optional(&mut *tx).await?;
        let Some((user, adapter)) = row else { continue };
        let identities = [session];
        let revoked = record_transaction(
            &mut tx,
            &identities,
            user,
            None,
            adapter.as_deref().unwrap_or(""),
            "spawn_failed",
            "dispatcher registration deadline exceeded; delivery unknown; not resent",
        )
        .await?;
        tx.commit().await?;
        clear_caches(host, &identities, revoked);
    }
    Ok(())
}
