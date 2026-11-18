use cctui_proto::api::redteam::{
    RedteamOverview, RedteamRun, RedteamRunReport, RedteamTier, RedteamVersion,
};
use serde_json::Value;
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

const KEPT_PER_LANE: i64 = 200;
const VERSIONS_SHOWN: i64 = 10;

/// Also drops the lane's runs past the newest [`KEPT_PER_LANE`].
pub async fn insert(
    pool: &PgPool,
    report: &RedteamRunReport,
    posted_by: Uuid,
) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO redteam_runs \
         (tier, lane, version, run_id, verdict, finished_at, posted_by, report) \
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6::float8), $7, $8) RETURNING id",
    )
    .bind(report.tier.as_str())
    .bind(&report.lane)
    .bind(&report.version)
    .bind(&report.run_id)
    .bind(report.verdict.as_str())
    .bind(report.finished_at)
    .bind(posted_by)
    .bind(Json(report))
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM redteam_runs WHERE tier = $1 AND lane = $2 AND id NOT IN \
         (SELECT id FROM redteam_runs WHERE tier = $1 AND lane = $2 ORDER BY id DESC LIMIT $3)",
    )
    .bind(report.tier.as_str())
    .bind(&report.lane)
    .bind(KEPT_PER_LANE)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn overview(
    pool: &PgPool,
    current_version: &str,
) -> Result<RedteamOverview, sqlx::Error> {
    let versions: Vec<String> = sqlx::query_scalar(
        "SELECT version FROM redteam_runs GROUP BY version \
         ORDER BY max(received_at) DESC, version DESC LIMIT $1",
    )
    .bind(VERSIONS_SHOWN)
    .fetch_all(pool)
    .await?;
    Ok(RedteamOverview {
        current_version: current_version.to_owned(),
        versions: latest_for(pool, &versions).await?,
    })
}

pub async fn latest_for(
    pool: &PgPool,
    versions: &[String],
) -> Result<Vec<RedteamVersion>, sqlx::Error> {
    let rows: Vec<(i64, i64, Option<String>, Value)> = sqlx::query_as(
        "SELECT DISTINCT ON (r.version, r.tier, r.lane) \
                r.id, extract(epoch FROM r.received_at)::bigint, u.name, r.report \
         FROM redteam_runs r LEFT JOIN users u ON u.id = r.posted_by \
         WHERE r.version = ANY($1) \
         ORDER BY r.version, r.tier, r.lane, r.finished_at DESC, r.id DESC",
    )
    .bind(versions)
    .fetch_all(pool)
    .await?;
    let runs: Vec<RedteamRun> = rows
        .into_iter()
        .filter_map(|(id, received_at, posted_by, report)| {
            let report: RedteamRunReport = serde_json::from_value(report).ok()?;
            let escapes = report.escapes();
            Some(RedteamRun { id, received_at, posted_by, report, escapes })
        })
        .collect();
    Ok(versions
        .iter()
        .map(|version| {
            let mine = || runs.iter().filter(move |r| r.report.version == *version);
            RedteamVersion {
                version: version.clone(),
                t2: mine().find(|r| r.report.tier == RedteamTier::T2).cloned(),
                t3: mine().filter(|r| r.report.tier == RedteamTier::T3).cloned().collect(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::redteam::{RedteamEscape, RedteamOutcome};

    use super::*;

    fn fixture() -> RedteamRunReport {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../redteam/ci/run-report.fixture.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    async fn seeded(tag: &str) -> Option<(PgPool, Uuid, String)> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let name = format!("{tag}-{uid}");
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(&name)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        Some((pool, uid, name))
    }

    fn version() -> String {
        format!("0.0.0-rt{}", &Uuid::new_v4().simple().to_string()[..12])
    }

    fn report(tier: RedteamTier, lane: &str, version: &str, finished_at: i64) -> RedteamRunReport {
        RedteamRunReport {
            tier,
            lane: lane.to_owned(),
            version: version.to_owned(),
            finished_at,
            ..fixture()
        }
    }

    #[tokio::test]
    async fn a_stored_run_reads_back_with_its_poster_and_escapes() {
        let Some((pool, uid, name)) = seeded("redteam_round_trip").await else { return };
        let v = version();
        let posted = report(RedteamTier::T2, "", &v, 1_700_000_000);
        let id = insert(&pool, &posted, uid).await.unwrap();
        let got = latest_for(&pool, std::slice::from_ref(&v)).await.unwrap();
        assert_eq!(got.len(), 1);
        let run = got[0].t2.as_ref().expect("the T2 run");
        assert_eq!(run.id, id);
        assert_eq!(run.report, posted);
        assert_eq!(run.posted_by.as_deref(), Some(name.as_str()));
        assert_eq!(
            run.escapes,
            vec![RedteamEscape {
                cell: "codex/kube/transparent/root".to_owned(),
                probe: Some("EGR-04".to_owned())
            }]
        );
        assert!(got[0].t3.is_empty());
    }

    #[tokio::test]
    async fn latest_is_the_newest_t2_and_the_newest_t3_per_lane_of_each_version() {
        let Some((pool, uid, _)) = seeded("redteam_latest").await else { return };
        let (v1, v2) = (version(), version());
        let mut newer_t2 = report(RedteamTier::T2, "", &v1, 1_700_000_200);
        newer_t2.verdict = RedteamOutcome::Contained;
        newer_t2.run_id = "newer".to_owned();
        for r in [
            report(RedteamTier::T2, "", &v1, 1_700_000_100),
            newer_t2.clone(),
            report(RedteamTier::T3, "codex", &v1, 1_700_000_050),
            report(RedteamTier::T3, "claude-code", &v1, 1_700_000_010),
            report(RedteamTier::T3, "claude-code", &v1, 1_700_000_020),
            report(RedteamTier::T3, "claude-code", &v2, 1_700_000_900),
        ] {
            insert(&pool, &r, uid).await.unwrap();
        }
        let got = latest_for(&pool, &[v1.clone(), v2.clone()]).await.unwrap();
        let order: Vec<&str> = got.iter().map(|g| g.version.as_str()).collect();
        assert_eq!(order, [v1.as_str(), v2.as_str()]);
        assert_eq!(got[0].t2.as_ref().unwrap().report, newer_t2);
        let lanes: Vec<(&str, i64)> =
            got[0].t3.iter().map(|r| (r.report.lane.as_str(), r.report.finished_at)).collect();
        assert_eq!(lanes, [("claude-code", 1_700_000_020), ("codex", 1_700_000_050)]);
        assert!(got[1].t2.is_none());
        assert_eq!(got[1].t3.len(), 1);
    }

    #[tokio::test]
    async fn the_table_refuses_an_unsafe_version() {
        let Some((pool, _, _)) = seeded("redteam_bad_version").await else { return };
        let inserted = sqlx::query(
            "INSERT INTO redteam_runs (tier, version, run_id, verdict, finished_at, report) \
             VALUES ('t2', '../x', 'r', 'contained', now(), '{}')",
        )
        .execute(&pool)
        .await;
        assert!(inserted.is_err());
    }
}
