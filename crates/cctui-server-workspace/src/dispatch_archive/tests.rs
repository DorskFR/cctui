use serde_json::json;
use sqlx::PgPool;

use super::*;
use crate::events::tests::{seed_session, seed_user_machine};

#[test]
fn the_setting_defaults_on_with_a_three_minute_grace() {
    let want = Policy { enabled: true, grace_secs: 180, keep_profiles: vec![] };
    assert_eq!(Policy::from_settings(None), want);
    assert_eq!(Policy::from_settings(Some(&json!({}))), want);
    assert_eq!(Policy::from_settings(Some(&json!({ ENABLED_KEY: "no", GRACE_KEY: "x" }))), want);
}

#[test]
fn the_setting_reads_the_owners_choices() {
    let p = Policy::from_settings(Some(&json!({
        ENABLED_KEY: false,
        GRACE_KEY: 0,
        KEEP_PROFILES_KEY: [" review ", "review", "", 3],
    })));
    assert_eq!(p, Policy { enabled: false, grace_secs: 0, keep_profiles: vec!["review".into()] });
    assert_eq!(Policy::from_settings(Some(&json!({ GRACE_KEY: 99_999 }))).grace_secs, 3600);
    assert_eq!(Policy::from_settings(Some(&json!({ GRACE_KEY: -5 }))).grace_secs, 0);
}

#[test]
fn a_kept_profile_or_a_disabled_setting_covers_nothing() {
    let p = Policy { keep_profiles: vec!["review".into()], ..Policy::default() };
    assert!(p.covers(None));
    assert!(p.covers(Some("build")));
    assert!(!p.covers(Some("review")));
    assert!(!Policy { enabled: false, ..Policy::default() }.covers(None));
}

#[test]
fn the_grace_counts_from_the_end() {
    let end = Utc::now();
    let p = Policy::default();
    assert!(!p.due(end, end + chrono::Duration::seconds(179)));
    assert!(p.due(end, end + chrono::Duration::seconds(180)));
    assert!(Policy { grace_secs: 0, ..Policy::default() }.due(end, end));
}

#[test]
fn clamp_drops_wrong_types_and_normalizes_the_rest() {
    let mut data = json!({
        ENABLED_KEY: "yes",
        GRACE_KEY: 7200,
        KEEP_PROFILES_KEY: ["a", " a ", "b"],
        "other": 1,
    });
    clamp(&mut data);
    assert_eq!(data, json!({ GRACE_KEY: 3600, KEEP_PROFILES_KEY: ["a", "b"], "other": 1 }));

    let mut data = json!({ ENABLED_KEY: false, GRACE_KEY: "soon", KEEP_PROFILES_KEY: [] });
    clamp(&mut data);
    assert_eq!(data, json!({ ENABLED_KEY: false }));
}

#[test]
fn a_session_left_on_a_question_waits_on_the_user() {
    assert!(waits_on_user(Some("blocked"), None, None));
    assert!(!waits_on_user(None, Some("stopped"), Some("success")));
    assert!(!waits_on_user(None, Some("done"), Some("failure")));
}

struct Fixture {
    pool: PgPool,
    state: AppState,
    uid: Uuid,
    machine: Uuid,
}

impl Fixture {
    async fn new(tag: &str) -> Option<Self> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = seed_user_machine(&pool, tag).await;
        Some(Self { pool, state, uid, machine })
    }

    async fn session(&self) -> String {
        seed_session(&self.pool, self.uid, self.machine, None).await
    }

    async fn dispatched(&self, profile: Option<&str>) -> String {
        let sid = self.session().await;
        sqlx::query(
            "INSERT INTO dispatch_records (session_id, user_id, dispatcher_name, profile, outcome) \
             VALUES ($1, $2, 'k8s', $3, 'registered')",
        )
        .bind(&sid)
        .bind(self.uid)
        .bind(profile)
        .execute(&self.pool)
        .await
        .unwrap();
        sid
    }

    async fn end(&self, sid: &str, status: &str, reason: &str, secs_ago: f64) {
        sqlx::query(
            "UPDATE sessions SET status = $2, end_reason = $3, \
                    ended_at = now() - make_interval(secs => $4) \
             WHERE id = $1",
        )
        .bind(sid)
        .bind(status)
        .bind(reason)
        .bind(secs_ago)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn nest(&self, child: &str, parent: &str, relation: Option<&str>) {
        sqlx::query(
            "UPDATE sessions SET parent_id = $2, \
                    metadata = COALESCE(metadata, '{}'::jsonb) \
                               || COALESCE(jsonb_build_object('relation', $3::text), '{}'::jsonb) \
             WHERE id = $1",
        )
        .bind(child)
        .bind(parent)
        .bind(relation)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn settings(&self, data: Value) {
        sqlx::query(
            "INSERT INTO user_settings (user_id, data) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET data = EXCLUDED.data",
        )
        .bind(self.uid)
        .bind(data)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    fn pod(&self) -> String {
        format!("dispatch-archive-test-{}", self.uid)
    }

    async fn view(&self, sid: &str) {
        sqlx::query("INSERT INTO session_viewers (session_id, pod) VALUES ($1, $2)")
            .bind(sid)
            .bind(self.pod())
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn leave(&self, sid: &str) {
        sqlx::query("DELETE FROM session_viewers WHERE session_id = $1 AND pod = $2")
            .bind(sid)
            .bind(self.pod())
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn sweep(&self) {
        run(&self.state, Some(self.uid)).await;
    }

    async fn status(&self, sid: &str) -> String {
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(sid)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        sqlx::query("DELETE FROM session_viewers WHERE pod = $1")
            .bind(self.pod())
            .execute(&self.pool)
            .await
            .ok();
        for sql in [
            "DELETE FROM dispatch_handles WHERE session_id IN \
             (SELECT id FROM sessions WHERE user_id = $1)",
            "DELETE FROM dispatch_records WHERE user_id = $1",
            "DELETE FROM sessions WHERE user_id = $1",
            "DELETE FROM machines WHERE user_id = $1",
            "DELETE FROM user_settings WHERE user_id = $1",
            "DELETE FROM users WHERE id = $1",
        ] {
            sqlx::query(sql).bind(self.uid).execute(&self.pool).await.ok();
        }
    }
}

#[tokio::test]
async fn completed_and_failed_dispatches_are_archived_once_the_grace_passes() {
    let Some(f) = Fixture::new("dispatch_archive_ends").await else { return };
    let done = f.dispatched(Some("build")).await;
    let failed = f.dispatched(None).await;
    let fresh = f.dispatched(None).await;
    let lost = f.dispatched(None).await;
    let local = f.session().await;
    let running = f.dispatched(None).await;
    f.end(&done, "ended", "completed", 300.0).await;
    f.end(&failed, "failed", "crashed", 300.0).await;
    f.end(&fresh, "ended", "completed", 10.0).await;
    f.end(&lost, "ended", "daemon_lost", 300.0).await;
    f.end(&local, "ended", "completed", 300.0).await;

    f.sweep().await;

    assert_eq!(f.status(&done).await, "archived", "a completed run");
    assert_eq!(f.status(&failed).await, "archived", "a failed run");
    assert_eq!(f.status(&fresh).await, "ended", "still inside the grace");
    assert_eq!(f.status(&lost).await, "ended", "a transport guess is left to the TTL sweep");
    assert_eq!(f.status(&local).await, "ended", "not a dispatch");
    assert_eq!(f.status(&running).await, "active", "still running");

    let (end_reason, profile): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT s.end_reason, r.profile FROM sessions s \
         JOIN dispatch_records r ON r.session_id = s.id WHERE s.id = $1",
    )
    .bind(&done)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        (end_reason.as_deref(), profile.as_deref()),
        (Some("completed"), Some("build")),
        "the session and its dispatch record stay readable"
    );

    sqlx::query("UPDATE sessions SET status = 'ended' WHERE id = $1")
        .bind(&done)
        .execute(&f.pool)
        .await
        .unwrap();
    f.sweep().await;
    assert_eq!(f.status(&done).await, "ended", "an un-archive is never undone");
    f.cleanup().await;
}

#[tokio::test]
async fn a_session_the_user_holds_stays_until_they_let_go() {
    let Some(f) = Fixture::new("dispatch_archive_held").await else { return };
    let pinned = f.dispatched(None).await;
    let action = f.dispatched(None).await;
    let asking = f.dispatched(None).await;
    let viewed = f.dispatched(None).await;
    let viewed_child_root = f.dispatched(None).await;
    let viewed_child = f.session().await;
    f.nest(&viewed_child, &viewed_child_root, None).await;
    for sid in [&pinned, &action, &asking, &viewed, &viewed_child_root] {
        f.end(sid, "ended", "completed", 300.0).await;
    }
    sqlx::query("UPDATE sessions SET pinned = true WHERE id = $1")
        .bind(&pinned)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO session_user_actions (session_id, title) VALUES ($1, 'approve')")
        .bind(&action)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET tempo = 'blocked' WHERE id = $1")
        .bind(&asking)
        .execute(&f.pool)
        .await
        .unwrap();
    f.view(&viewed).await;
    f.view(&viewed_child).await;

    f.sweep().await;
    for (sid, why) in [
        (&pinned, "pinned"),
        (&action, "open user action"),
        (&asking, "left on a question"),
        (&viewed, "open in a client"),
        (&viewed_child_root, "a session under it is open"),
    ] {
        assert_eq!(f.status(sid).await, "ended", "{why}");
    }

    f.leave(&viewed).await;
    f.leave(&viewed_child).await;
    sqlx::query("UPDATE session_user_actions SET status = 'done' WHERE session_id = $1")
        .bind(&action)
        .execute(&f.pool)
        .await
        .unwrap();
    f.sweep().await;
    assert_eq!(f.status(&viewed).await, "archived", "archived once the viewer left");
    assert_eq!(f.status(&viewed_child_root).await, "archived");
    assert_eq!(f.status(&viewed_child).await, "archived", "its tree goes with it");
    assert_eq!(f.status(&action).await, "archived", "archived once the action is resolved");
    assert_eq!(f.status(&pinned).await, "ended");
    assert_eq!(f.status(&asking).await, "ended");
    f.cleanup().await;
}

#[tokio::test]
async fn a_finished_child_of_a_dispatch_is_archived_while_the_dispatch_runs() {
    let Some(f) = Fixture::new("dispatch_archive_child").await else { return };
    let root = f.dispatched(None).await;
    let child = f.session().await;
    let grandchild = f.session().await;
    let fork = f.session().await;
    f.nest(&child, &root, None).await;
    f.nest(&grandchild, &child, None).await;
    f.nest(&fork, &root, Some("fork")).await;
    f.end(&grandchild, "ended", "completed", 300.0).await;
    f.end(&fork, "ended", "completed", 300.0).await;

    f.sweep().await;

    assert_eq!(f.status(&grandchild).await, "archived", "nested under a dispatch");
    assert_eq!(f.status(&fork).await, "ended", "a fork only links to the dispatch");
    assert_eq!(f.status(&child).await, "active");
    assert_eq!(f.status(&root).await, "active");
    f.cleanup().await;
}

#[tokio::test]
async fn the_owners_setting_and_profile_choices_apply() {
    let Some(f) = Fixture::new("dispatch_archive_setting").await else { return };
    let kept = f.dispatched(Some("review")).await;
    let instant = f.dispatched(Some("build")).await;
    f.end(&kept, "ended", "completed", 300.0).await;
    f.end(&instant, "ended", "completed", 1.0).await;

    f.settings(json!({ ENABLED_KEY: false, GRACE_KEY: 0 })).await;
    f.sweep().await;
    assert_eq!(f.status(&kept).await, "ended", "setting off");
    assert_eq!(f.status(&instant).await, "ended", "setting off");

    f.settings(json!({ GRACE_KEY: 0, KEEP_PROFILES_KEY: ["review"] })).await;
    f.sweep().await;
    assert_eq!(f.status(&kept).await, "ended", "its profile is kept");
    assert_eq!(f.status(&instant).await, "archived", "a zero grace archives at once");
    f.cleanup().await;
}
