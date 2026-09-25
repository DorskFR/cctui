use cctui_proto::adapter::AdapterEvent;
use cctui_proto::ws::DaemonFrameUp;
use uuid::Uuid;

use super::bumps::Bumps;
use super::events::handle_event;
use crate::state::AppState;

/// Whether an event's `stream_events` row is already written, and its seq if
/// it was newly inserted.
#[derive(Clone, Copy)]
pub(super) enum Stored {
    Pending,
    Done(Option<i64>),
}

/// A persistable event held back so consecutive ones from one frame share a
/// single `stream_events` insert.
pub(super) struct Ingest {
    adapter_id: String,
    event: AdapterEvent,
    row: NewEvent,
}

impl Ingest {
    pub(super) fn take(frame: DaemonFrameUp) -> Result<Self, Box<DaemonFrameUp>> {
        let DaemonFrameUp::Event { adapter_id, event } = frame else {
            return Err(Box::new(frame));
        };
        let row = match &event {
            AdapterEvent::Message { local_id, payload, turn_id } => NewEvent {
                local_id: local_id.clone(),
                event_type: "message",
                payload: payload.clone(),
                turn_id: *turn_id,
            },
            AdapterEvent::ToolUse { local_id, payload } => NewEvent {
                local_id: local_id.clone(),
                event_type: "tool_use",
                payload: payload.clone(),
                turn_id: None,
            },
            _ => return Err(Box::new(DaemonFrameUp::Event { adapter_id, event })),
        };
        Ok(Self { adapter_id, event, row })
    }
}

/// Insert a run of held-back events in one statement, then run each event's
/// side effects in order with its outcome.
pub(super) async fn ingest_run(
    state: &AppState,
    bumps: &Bumps,
    machine_id: Uuid,
    user_id: Uuid,
    run: &mut Vec<Ingest>,
) {
    if run.is_empty() {
        return;
    }
    let mut events = Vec::with_capacity(run.len());
    let mut rows = Vec::with_capacity(run.len());
    for Ingest { adapter_id, event, mut row } in run.drain(..) {
        if row.event_type == "message" {
            crate::keepalive::observe_message(state, &row.local_id, &mut row.payload).await;
        }
        events.push((adapter_id, event));
        rows.push(row);
    }
    let seqs = match insert_events(&state.pool, machine_id, user_id, rows).await {
        Ok(seqs) => seqs,
        Err(err) => {
            tracing::warn!(%err, %machine_id, events = events.len(), "batched event insert failed");
            return;
        }
    };
    for ((adapter_id, event), seq) in events.into_iter().zip(seqs) {
        if let Err(err) =
            handle_event(state, bumps, machine_id, user_id, &adapter_id, event, Stored::Done(seq))
                .await
        {
            tracing::warn!(%err, "handle_event error");
        }
    }
}

/// Bump the per-machine persisted-insert counter feeding divergence detection,
/// only when a `stream_events` row was actually written.
pub(super) fn note_insert(state: &AppState, machine_id: Uuid, newly_inserted: bool) {
    if newly_inserted {
        *state.machine_event_inserts.entry(machine_id).or_insert(0) += 1;
    }
}

/// Insert a stream event, returning `Some(id)` (the `stream_events.id`
/// BIGSERIAL, used as the causal ordering `seq`) if a new row was
/// written and `None` if it was a duplicate suppressed by the dedup constraint
/// (or the session row was absent or not owned by `machine_id`/`user_id`). Callers use the presence to decide whether
/// to broadcast the event live, so a replayed session history doesn't re-stream
/// to clients.
pub(super) async fn insert_event(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    event_type: &str,
    mut payload: serde_json::Value,
    turn_id: Option<uuid::Uuid>,
) -> anyhow::Result<Option<i64>> {
    // Postgres jsonb/text cannot store the NUL code point (`\0`); a
    // payload carrying one (e.g. binary-ish tool output) fails the INSERT and
    // the event is silently lost. Strip NULs from every string so
    // the rest of the payload survives.
    strip_nul(&mut payload);
    // `WHERE EXISTS`: a daemon can emit events for a session the server never
    // registered (missed SessionStarted, ephemeral subagent); those are a
    // no-op instead of an FK violation.
    let links = crate::routes::fs::extract_links(&payload);
    // The ON CONFLICT target must stay character-identical to migration 121's
    // `stream_events_dedup_turn_idx` expression list or inference fails.
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO stream_events (session_id, event_type, payload, turn_id) \
         SELECT $1, $2, $3, $4 WHERE EXISTS ( \
             SELECT 1 FROM sessions WHERE id = $1 AND machine_uuid = $5 AND user_id = $6) \
         ON CONFLICT (session_id, event_type, content_hash, \
                      COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
         DO NOTHING \
         RETURNING id",
    )
    .bind(local_id)
    .bind(event_type)
    .bind(payload)
    .bind(turn_id)
    .bind(machine_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if id.is_some() {
        crate::routes::fs::record_links(pool, local_id, &links).await?;
    }
    Ok(id)
}

/// One `stream_events` row for [`insert_events`].
pub(super) struct NewEvent {
    local_id: String,
    event_type: &'static str,
    payload: serde_json::Value,
    turn_id: Option<Uuid>,
}

/// Rows per `stream_events` statement: bounds one statement's bind size.
const INSERT_BATCH: usize = 1000;

/// Batched [`insert_event`]: the same ownership guard and dedup, one statement
/// per [`INSERT_BATCH`] rows. Returns each row's outcome in input order. Ids are
/// drawn in input order, so the `seq`s of one batch keep the frame's order.
pub(super) async fn insert_events(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    user_id: Uuid,
    mut rows: Vec<NewEvent>,
) -> anyhow::Result<Vec<Option<i64>>> {
    let mut out = Vec::with_capacity(rows.len());
    for chunk in rows.chunks_mut(INSERT_BATCH) {
        let mut ids = Vec::with_capacity(chunk.len());
        let mut types = Vec::with_capacity(chunk.len());
        let mut payloads = Vec::with_capacity(chunk.len());
        let mut turns = Vec::with_capacity(chunk.len());
        let mut links = Vec::with_capacity(chunk.len());
        for row in chunk.iter_mut() {
            strip_nul(&mut row.payload);
            links.push(crate::routes::fs::extract_links(&row.payload));
            ids.push(row.local_id.clone());
            types.push(row.event_type);
            payloads.push(std::mem::take(&mut row.payload));
            turns.push(row.turn_id);
        }
        // The ON CONFLICT target must stay character-identical to migration 121's
        // `stream_events_dedup_turn_idx` expression list or inference fails.
        let inserted: Vec<(i64, i64)> = sqlx::query_as(
            "WITH src AS ( \
                 SELECT nextval(pg_get_serial_sequence('stream_events', 'id')) AS id, r.* \
                 FROM ( \
                     SELECT u.* \
                     FROM unnest($1::text[], $2::text[], $3::jsonb[], $4::uuid[]) \
                          WITH ORDINALITY AS u(session_id, event_type, payload, turn_id, ord) \
                     WHERE EXISTS ( \
                         SELECT 1 FROM sessions s \
                         WHERE s.id = u.session_id AND s.machine_uuid = $5 AND s.user_id = $6) \
                     ORDER BY u.ord \
                 ) r \
             ), ins AS ( \
                 INSERT INTO stream_events (id, session_id, event_type, payload, turn_id) \
                 SELECT id, session_id, event_type, payload, turn_id FROM src ORDER BY ord \
                 ON CONFLICT (session_id, event_type, content_hash, \
                              COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
                 DO NOTHING \
                 RETURNING id \
             ) \
             SELECT src.ord, ins.id FROM ins JOIN src USING (id)",
        )
        .bind(&ids)
        .bind(&types)
        .bind(&payloads)
        .bind(&turns)
        .bind(machine_id)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        let mut seqs = vec![None; chunk.len()];
        for (ord, id) in inserted {
            if let Some(slot) = usize::try_from(ord - 1).ok().and_then(|i| seqs.get_mut(i)) {
                *slot = Some(id);
            }
        }
        for ((seq, local_id), links) in seqs.iter().zip(&ids).zip(&links) {
            if seq.is_some() {
                crate::routes::fs::record_links(pool, local_id, links).await?;
            }
        }
        out.extend(seqs);
    }
    Ok(out)
}

/// Recursively strip NUL (`\0`) from every string in a JSON value.
/// Postgres rejects NUL in `jsonb`/`text`, so an event carrying one would be
/// dropped on insert. Stripping keeps the event; the NUL has no
/// display value anyway.
fn strip_nul(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            if s.contains('\0') {
                *s = s.replace('\0', "");
            }
        }
        serde_json::Value::Array(arr) => arr.iter_mut().for_each(strip_nul),
        serde_json::Value::Object(map) => map.values_mut().for_each(strip_nul),
        _ => {}
    }
}

#[cfg(test)]
pub(super) fn backfill(local_id: &str, n: usize) -> Vec<NewEvent> {
    (0..n)
        .map(|i| NewEvent {
            local_id: local_id.to_owned(),
            event_type: "message",
            payload: serde_json::json!({ "type": "assistant", "text": format!("line {i}") }),
            turn_id: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::routes::daemon::test_support::{drop_machines, seed_machine, seed_owned_session};

    #[test]
    fn strip_nul_cleans_nested_strings() {
        let mut v = json!({
            "command": "echo hi",
            "aggregatedOutput": "ok\u{0000}bad",
            "nested": { "arr": ["a\u{0000}b", 1, true] },
        });
        strip_nul(&mut v);
        assert_eq!(v["aggregatedOutput"], "okbad");
        assert_eq!(v["nested"]["arr"][0], "ab");
        assert_eq!(v["command"], "echo hi");
        // Non-strings untouched.
        assert_eq!(v["nested"]["arr"][1], 1);
    }

    #[tokio::test]
    async fn turn_id_persists_and_widens_the_dedup_key() {
        let name = "turn_id_persists_and_widens_the_dedup_key";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{name}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        let sid = format!("{name}-{uid}");
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();

        let sql = "INSERT INTO stream_events (session_id, event_type, payload, turn_id) \
                   SELECT $1, $2, $3, $4 WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
                   ON CONFLICT (session_id, event_type, content_hash, \
                                COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
                   DO NOTHING \
                   RETURNING id";
        let payload = serde_json::json!({"role": "user", "text": "continue"});
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut inserted = Vec::new();
        for turn_id in [Some(first), Some(first), Some(second), None, None] {
            let id: Option<i64> = sqlx::query_scalar(sql)
                .bind(&sid)
                .bind("message")
                .bind(&payload)
                .bind(turn_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
            inserted.push(id.is_some());
        }
        assert_eq!(
            inserted,
            vec![true, false, true, true, false],
            "a replayed turn dedups, two distinct turns with the same text both persist, \
             and a turn-less row keys on content alone"
        );

        let stored: Vec<Option<Uuid>> = sqlx::query_scalar(
            "SELECT turn_id FROM stream_events WHERE session_id = $1 ORDER BY id",
        )
        .bind(&sid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(stored, vec![Some(first), Some(second), None]);

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn a_backfill_lands_in_a_handful_of_statements_in_order() {
        let Some(url) = crate::routes::gateway::test_db_url("batched_backfill") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "backfill").await;
        let sid = seed_owned_session(&pool, uid, mid).await;

        let n: usize = 5000;
        assert!(n.div_ceil(INSERT_BATCH) <= 5, "a 5k backfill is at most five statements");
        let mut rows = backfill(&sid, n);
        rows.push(NewEvent {
            local_id: sid.clone(),
            event_type: "message",
            payload: json!({ "type": "assistant", "text": "line 7" }),
            turn_id: None,
        });
        let seqs = insert_events(&pool, mid, uid, rows).await.expect("insert");
        assert_eq!(seqs.len(), n + 1);
        assert_eq!(seqs[n], None, "a duplicate inside the batch is deduped");
        let ids: Vec<i64> = seqs[..n].iter().map(|s| s.expect("fresh row")).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "seqs follow the frame's order");

        let replay = insert_events(&pool, mid, uid, backfill(&sid, n)).await.expect("replay");
        assert!(replay.iter().all(Option::is_none), "a replay inserts nothing");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stream_events WHERE session_id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("count events");
        assert_eq!(count, i64::try_from(n).expect("fits"));

        sqlx::query("DELETE FROM stream_events WHERE session_id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .ok();
        drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }

    #[tokio::test]
    async fn a_batch_refuses_the_rows_of_a_foreign_session() {
        let Some(url) = crate::routes::gateway::test_db_url("batched_foreign") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (ua, ma) = seed_machine(&pool, "owner").await;
        let (ub, mb) = seed_machine(&pool, "intruder").await;
        let own = seed_owned_session(&pool, ub, mb).await;
        let foreign = seed_owned_session(&pool, ua, ma).await;

        let mut rows = backfill(&own, 2);
        rows.insert(1, backfill(&foreign, 1).remove(0));
        let seqs = insert_events(&pool, mb, ub, rows).await.expect("insert");
        assert!(seqs[0].is_some());
        assert_eq!(seqs[1], None, "the foreign row is refused");
        assert!(seqs[2].is_some());

        let foreign_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stream_events WHERE session_id = $1")
                .bind(&foreign)
                .fetch_one(&pool)
                .await
                .expect("count events");
        assert_eq!(foreign_rows, 0);

        sqlx::query("DELETE FROM stream_events WHERE session_id = ANY($1)")
            .bind(vec![own.clone(), foreign.clone()])
            .execute(&pool)
            .await
            .ok();
        drop_machines(&pool, &[own, foreign], &[(ua, ma), (ub, mb)]).await;
    }
}
