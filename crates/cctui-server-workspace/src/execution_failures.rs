//! Workflow completion evidence belongs to the interactive workspace.
use cctui_proto::api::FailedDispatch;
use cctui_server_core::store::dispatch_records::{self as core, Listing};
use uuid::Uuid;

/// Preserve full-server failure cards after a workflow guard completes.
pub async fn list(
    pool: &sqlx::PgPool,
    listing: Listing,
) -> Result<Vec<FailedDispatch>, sqlx::Error> {
    let completed: Vec<String> = sqlx::query_scalar(
        "SELECT g.session_id FROM guard_steps g JOIN dispatch_records r ON r.session_id=g.session_id \
         WHERE ($1::uuid IS NULL OR r.user_id=$1) AND r.created_at > now() - interval '7 days' \
         AND (g.state->>'step')::bigint < 0",
    ).bind(listing.owner).fetch_all(pool).await?;
    core::list_with_completed(pool, listing, &completed).await
}

/// Dismiss the same workspace-aware list the caller sees.
pub async fn dismiss_all(
    pool: &sqlx::PgPool,
    viewer: Uuid,
    owner: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    let shown =
        list(pool, Listing { owner, viewer: Some(viewer), include_dismissed: false }).await?;
    let ids: Vec<String> = shown.into_iter().map(|record| record.session_id).collect();
    core::dismiss_many(pool, viewer, &ids).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completed_guards_hide_only_terminal_crashes_and_dismiss_the_visible_list() {
        let Some(url) = cctui_server_core::test_support::db_url("workspace_failure_evidence")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path=pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for migration in cctui_server_core::schema::MIGRATIONS {
            sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.replace("public.", "pg_temp.")))
                .execute(&pool)
                .await
                .unwrap();
        }
        let guard_table = crate::schema::BOOTSTRAP
            .split("-- TABLE: guard_steps\n")
            .nth(1)
            .unwrap()
            .split("\n-- ")
            .next()
            .unwrap()
            .replace("public.", "pg_temp.");
        sqlx::raw_sql(sqlx::AssertSqlSafe(guard_table)).execute(&pool).await.unwrap();
        let owner = Uuid::new_v4();
        let foreign = Uuid::new_v4();
        for uid in [owner, foreign] {
            sqlx::query("INSERT INTO users(id,name,key_hash) VALUES($1,'fixture',$2)")
                .bind(uid)
                .bind(uid.to_string())
                .execute(&pool)
                .await
                .unwrap();
        }
        for (id, uid, outcome, step) in [
            ("done", owner, "registered", -1),
            ("lost", owner, "registered", 3),
            ("refused", owner, "refused", -1),
            ("foreign", foreign, "registered", -1),
        ] {
            sqlx::query("INSERT INTO sessions(id,machine_id,working_dir,user_id,status,end_reason) VALUES($1,'fixture','/work/acme',$2,'failed','crashed')")
                .bind(id).bind(uid).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO dispatch_records(session_id,user_id,dispatcher_name,outcome) VALUES($1,$2,'fixture',$3)")
                .bind(id).bind(uid).bind(outcome).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO guard_steps(session_id,state) VALUES($1,$2)")
                .bind(id)
                .bind(sqlx::types::Json(serde_json::json!({"step":step})))
                .execute(&pool)
                .await
                .unwrap();
        }
        let listing = Listing { owner: Some(owner), viewer: Some(owner), include_dismissed: false };
        assert_eq!(core::list(&pool, listing).await.unwrap().len(), 3);
        let shown = list(&pool, listing).await.unwrap();
        let mut ids: Vec<_> = shown.iter().map(|row| row.session_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["lost", "refused"]);
        assert!(list(&pool, Listing { owner: Some(foreign), ..listing }).await.unwrap().is_empty());
        assert_eq!(dismiss_all(&pool, owner, Some(owner)).await.unwrap(), 2);
        assert!(list(&pool, listing).await.unwrap().is_empty());
        let history = list(&pool, Listing { include_dismissed: true, ..listing }).await.unwrap();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|row| row.dismissed));
    }
}
