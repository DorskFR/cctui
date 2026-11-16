use cctui_proto::api::outcome::{OutcomeStatus, SessionOutcome};
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::preset::lane as preset_lane;
use super::store::{self, Lineage, NewEvent};
use crate::routes::inbox::DEFAULT_TOPIC;
use crate::state::AppState;

#[derive(Debug, sqlx::FromRow)]
struct Origin {
    user_id: Uuid,
    key: String,
    preset_id: Uuid,
    then_preset_id: Uuid,
    hops: i16,
}

const ORIGIN_SQL: &str = "\
    SELECT l.user_id, l.key, l.preset_id, p.then_preset_id, \
           COALESCE((SELECT max(e.hops) FROM inbox_events e \
                     WHERE e.lane_id = l.id AND e.delivered_session_id = $1), 0::smallint) \
             AS hops \
    FROM inbox_lanes l JOIN presets p ON p.id = l.preset_id \
    WHERE p.then_preset_id IS NOT NULL \
      AND (l.session_id = $1 OR EXISTS (SELECT 1 FROM inbox_events e \
           WHERE e.lane_id = l.id AND e.delivered_session_id = $1)) \
    ORDER BY (l.session_id = $1) DESC NULLS LAST, l.created_at DESC \
    LIMIT 1";

fn topic(key: &str, preset_id: Uuid) -> &str {
    key.strip_prefix(&super::preset_lane_key(preset_id, "")).unwrap_or(DEFAULT_TOPIC)
}

fn body(outcome: &SessionOutcome) -> String {
    outcome
        .body
        .as_deref()
        .map_or_else(|| outcome.headline.clone(), |body| format!("{}\n\n{body}", outcome.headline))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chained {
    Inserted(Uuid),
    Deduplicated,
    TooManyHops,
}

/// `None`: not a result, or the session's preset chains nowhere.
pub async fn chain(pool: &PgPool, outcome: &SessionOutcome) -> sqlx::Result<Option<Chained>> {
    if outcome.status != OutcomeStatus::Result {
        return Ok(None);
    }
    let session_id = outcome.session_id.as_str();
    let origin: Option<Origin> =
        sqlx::query_as(ORIGIN_SQL).bind(session_id).fetch_optional(pool).await?;
    let Some(origin) = origin else { return Ok(None) };
    let topic = topic(&origin.key, origin.preset_id);
    let lane_id = preset_lane(pool, origin.user_id, origin.then_preset_id, topic).await?;
    let source_id = format!("{session_id}:outcome");
    let body = body(outcome);
    let payload = serde_json::json!({
        "origin_session_id": session_id,
        "origin_preset_id": origin.preset_id,
        "outcome_id": outcome.id,
    });
    let event = NewEvent {
        lane_id,
        user_id: origin.user_id,
        source: Source::Outcome,
        source_id: Some(&source_id),
        body: &body,
        payload: Some(&payload),
        not_before: None,
        expires_at: None,
    };
    let lineage = Lineage { hops: origin.hops.saturating_add(1), parent_session_id: None };
    Ok(Some(match store::insert_traced(pool, &event, lineage).await? {
        None => Chained::TooManyHops,
        Some(inserted) if inserted.deduplicated => Chained::Deduplicated,
        Some(_) => Chained::Inserted(lane_id),
    }))
}

pub async fn on_outcome(state: &AppState, outcome: &SessionOutcome) {
    match chain(&state.pool, outcome).await {
        Ok(Some(Chained::Inserted(lane))) => super::deliver::wake_lane(state, lane),
        Ok(Some(Chained::TooManyHops)) => tracing::warn!(
            session_id = %outcome.session_id,
            "chain stopped: the next event would pass {} hops",
            store::MAX_HOPS
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(session_id = %outcome.session_id, "chain event failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::outcome::{OutcomeSource, OutcomeStatus, SessionOutcome};
    use cctui_proto::api::presets::LaunchPresetRequest;
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{Chained, chain, topic};
    use crate::inbox::preset::lane as preset_lane;
    use crate::presets::store_tests::{Owner, seed_owner};
    use crate::presets::{store, validate};

    async fn pool(name: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn preset(pool: &PgPool, owner: &Owner, then: Option<Uuid>) -> Uuid {
        let req = LaunchPresetRequest {
            name: format!("chain-{}", Uuid::new_v4()),
            prompt: "p".into(),
            machine_id: Some(owner.machine),
            working_dir: Some("/w".into()),
            then_preset_id: then,
            ..LaunchPresetRequest::default()
        };
        store::create(pool, owner.user, &validate(&req).unwrap()).await.unwrap().id
    }

    async fn origin(pool: &PgPool, owner: &Owner, from: Uuid, hops: i16) -> String {
        let sid = format!("chain-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(owner.machine)
        .bind(owner.user)
        .execute(pool)
        .await
        .unwrap();
        let lane = preset_lane(pool, owner.user, from, "ops").await.unwrap();
        sqlx::query("UPDATE inbox_lanes SET session_id = $2 WHERE id = $1")
            .bind(lane)
            .bind(&sid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO inbox_events (lane_id, user_id, source, body, status, hops, \
               delivered_session_id, delivered_as, delivered_at) \
             VALUES ($1, $2, 'n8n', 'x', 'delivered', $3, $4, 'spawn', now())",
        )
        .bind(lane)
        .bind(owner.user)
        .bind(hops)
        .bind(&sid)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    fn outcome(session_id: &str, status: OutcomeStatus) -> SessionOutcome {
        SessionOutcome {
            id: Uuid::new_v4(),
            session_id: session_id.to_owned(),
            dispatch_id: None,
            turn_seq: None,
            status,
            headline: "Opened PR 12".into(),
            body: Some("tests pass".into()),
            evidence: Vec::new(),
            source: OutcomeSource::Tool,
            created_at: chrono::Utc::now(),
        }
    }

    async fn chained_events(pool: &PgPool, next: Uuid) -> Vec<(String, String, i16, String)> {
        sqlx::query_as(
            "SELECT l.key, e.source_id, e.hops, e.body FROM inbox_events e \
             JOIN inbox_lanes l ON l.id = e.lane_id \
             WHERE l.preset_id = $1 AND e.source = 'outcome'",
        )
        .bind(next)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[test]
    fn the_topic_carries_over_from_the_origin_lane() {
        let id = Uuid::nil();
        assert_eq!(topic(&format!("preset:{id}:ops"), id), "ops");
        assert_eq!(topic("session:s-1", id), "default");
    }

    #[tokio::test]
    async fn a_result_outcome_chains_once_with_one_more_hop() {
        let Some(pool) = pool("a_result_outcome_chains_once").await else { return };
        let owner = seed_owner(&pool, "chain").await;
        let next = preset(&pool, &owner, None).await;
        let first = preset(&pool, &owner, Some(next)).await;
        let sid = origin(&pool, &owner, first, 3).await;

        let failed = chain(&pool, &outcome(&sid, OutcomeStatus::Failed)).await.unwrap();
        assert_eq!(failed, None, "only a result chains");
        let done = outcome(&sid, OutcomeStatus::Result);
        assert!(matches!(chain(&pool, &done).await.unwrap(), Some(Chained::Inserted(_))));
        assert_eq!(chain(&pool, &done).await.unwrap(), Some(Chained::Deduplicated));

        assert_eq!(
            chained_events(&pool, next).await,
            [(
                format!("preset:{next}:ops"),
                format!("{sid}:outcome"),
                4,
                "Opened PR 12\n\ntests pass".to_owned()
            )]
        );
        let tail = origin(&pool, &owner, next, 0).await;
        let last = chain(&pool, &outcome(&tail, OutcomeStatus::Result)).await.unwrap();
        assert_eq!(last, None, "the last preset chains nowhere");
    }

    #[tokio::test]
    async fn a_chain_past_eight_hops_is_refused() {
        let Some(pool) = pool("a_chain_past_eight_hops_is_refused").await else { return };
        let owner = seed_owner(&pool, "chainhops").await;
        let next = preset(&pool, &owner, None).await;
        let first = preset(&pool, &owner, Some(next)).await;
        let sid = origin(&pool, &owner, first, 8).await;
        let refused = chain(&pool, &outcome(&sid, OutcomeStatus::Result)).await.unwrap();
        assert_eq!(refused, Some(Chained::TooManyHops));
        assert!(chained_events(&pool, next).await.is_empty());

        let lane = preset_lane(&pool, owner.user, next, "ops").await.unwrap();
        let raw = sqlx::query(
            "INSERT INTO inbox_events (lane_id, user_id, source, body, hops) \
             VALUES ($1, $2, 'outcome', 'x', 9)",
        )
        .bind(lane)
        .bind(owner.user)
        .execute(&pool)
        .await;
        assert!(raw.is_err(), "the table refuses hops 9 too");
    }
}
