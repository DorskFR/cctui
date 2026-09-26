use std::collections::HashMap;
use std::sync::Mutex;

/// Session activity waiting for the next [`Bumps::flush`].
#[derive(Default)]
struct PendingBump {
    tool: Option<String>,
    tool_calls: i32,
    reset: bool,
}

/// Heartbeat and tool-activity writes of one daemon connection, coalesced per
/// session so a busy session costs one `sessions` UPDATE per flush instead of
/// one per event.
#[derive(Default)]
pub(super) struct Bumps(Mutex<HashMap<String, PendingBump>>);

impl Bumps {
    fn with(&self, local_id: &str, f: impl FnOnce(&mut PendingBump)) {
        if let Ok(mut pending) = self.0.lock() {
            f(pending.entry(local_id.to_owned()).or_default());
        }
    }

    pub(super) fn heartbeat(&self, local_id: &str) {
        self.with(local_id, |_| {});
    }

    /// Record the activity of an ingested event. A replayed event (not newly
    /// inserted) is history the row already reflects, so it bumps nothing.
    pub(super) fn note_activity(
        &self,
        local_id: &str,
        newly_inserted: bool,
        tool: Option<&str>,
        user_turn: bool,
    ) {
        if !newly_inserted {
            return;
        }
        self.with(local_id, |b| {
            if let Some(tool) = tool {
                b.tool = Some(tool.to_owned());
                b.tool_calls += 1;
            }
            if user_turn {
                b.reset = true;
                b.tool_calls = 0;
            }
        });
    }

    fn take(&self) -> HashMap<String, PendingBump> {
        self.0.lock().map(|mut pending| std::mem::take(&mut *pending)).unwrap_or_default()
    }

    /// Write every pending bump in one statement. Each session and its whole
    /// `parent_id` chain get `last_heartbeat` (so a grinding subagent keeps its
    /// parent fresh) and, after a tool call, `last_tool_at`/`last_tool_name`;
    /// `tool_use_count` is the leaf's own per-turn count. Returns rows updated.
    pub(super) async fn flush(&self, pool: &sqlx::PgPool) -> u64 {
        let pending = self.take();
        if pending.is_empty() {
            return 0;
        }
        let mut ids = Vec::with_capacity(pending.len());
        let mut tools = Vec::with_capacity(pending.len());
        let mut calls = Vec::with_capacity(pending.len());
        let mut resets = Vec::with_capacity(pending.len());
        for (id, bump) in pending {
            ids.push(id);
            tools.push(bump.tool);
            calls.push(bump.tool_calls);
            resets.push(bump.reset);
        }
        let done = sqlx::query(
            r"WITH RECURSIVE b AS (
                SELECT * FROM unnest($1::text[], $2::text[], $3::int4[], $4::bool[])
                    AS b(id, tool, calls, reset)
            ), chain AS (
                SELECT s.id, s.parent_id, b.tool FROM sessions s JOIN b ON s.id = b.id
                UNION ALL
                SELECT s.id, s.parent_id, c.tool FROM sessions s JOIN chain c ON s.id = c.parent_id
            ), agg AS (
                SELECT id, max(tool) AS tool FROM chain GROUP BY id
            )
            UPDATE sessions SET
                last_heartbeat = now(),
                last_tool_at = CASE WHEN agg.tool IS NULL THEN sessions.last_tool_at ELSE now() END,
                last_tool_name = COALESCE(agg.tool, sessions.last_tool_name),
                tool_use_count = COALESCE(
                    (SELECT CASE WHEN b.reset THEN 0 ELSE sessions.tool_use_count END + b.calls
                     FROM b WHERE b.id = sessions.id),
                    sessions.tool_use_count)
            FROM agg
            WHERE sessions.id = agg.id",
        )
        .bind(&ids)
        .bind(&tools)
        .bind(&calls)
        .bind(&resets)
        .execute(pool)
        .await;
        match done {
            Ok(done) => done.rows_affected(),
            Err(err) => {
                tracing::warn!(%err, sessions = ids.len(), "activity bump failed");
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::daemon::ingest::{backfill, insert_events};
    use crate::routes::daemon::test_support::{
        drop_machines, row_version, seed_machine, seed_owned_session,
    };

    #[tokio::test]
    async fn replaying_stored_events_leaves_the_session_row_alone() {
        let Some(url) = crate::routes::gateway::test_db_url("replay_no_bump") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "replay").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        let n = 500;
        insert_events(&pool, mid, uid, backfill(&sid, n)).await.expect("insert");
        let before = row_version(&pool, &sid).await;

        let bumps = Bumps::default();
        let replay = insert_events(&pool, mid, uid, backfill(&sid, n)).await.expect("replay");
        for seq in replay {
            bumps.note_activity(&sid, seq.is_some(), Some("Bash"), false);
        }
        assert_eq!(bumps.flush(&pool).await, 0, "a replay issues no sessions UPDATE");
        assert_eq!(row_version(&pool, &sid).await, before);

        drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }

    #[tokio::test]
    async fn live_activity_is_coalesced_into_one_update_per_flush() {
        let Some(url) = crate::routes::gateway::test_db_url("coalesced_bumps") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "bumps").await;
        let parent = seed_owned_session(&pool, uid, mid).await;
        let child = seed_owned_session(&pool, uid, mid).await;
        sqlx::query("UPDATE sessions SET parent_id = $2, tool_use_count = 9 WHERE id = $1")
            .bind(&child)
            .bind(&parent)
            .execute(&pool)
            .await
            .expect("link child");

        let bumps = Bumps::default();
        for _ in 0..100 {
            bumps.heartbeat(&child);
        }
        for _ in 0..3 {
            bumps.note_activity(&child, true, Some("Bash"), false);
        }
        bumps.note_activity(&child, true, None, true);
        bumps.note_activity(&child, true, Some("Read"), false);
        assert_eq!(bumps.flush(&pool).await, 2, "one statement updates the leaf and its parent");
        assert_eq!(bumps.flush(&pool).await, 0, "nothing left to write");

        let (count, tool): (i32, Option<String>) =
            sqlx::query_as("SELECT tool_use_count, last_tool_name FROM sessions WHERE id = $1")
                .bind(&child)
                .fetch_one(&pool)
                .await
                .expect("child row");
        assert_eq!(count, 1, "the user turn reset the count before the last tool");
        assert_eq!(tool.as_deref(), Some("Read"));
        let (parent_count, parent_tool): (i32, Option<String>) =
            sqlx::query_as("SELECT tool_use_count, last_tool_name FROM sessions WHERE id = $1")
                .bind(&parent)
                .fetch_one(&pool)
                .await
                .expect("parent row");
        assert_eq!(parent_count, 0, "the parent keeps its own count");
        assert_eq!(parent_tool.as_deref(), Some("Read"));

        drop_machines(&pool, &[child, parent], &[(uid, mid)]).await;
    }
}
