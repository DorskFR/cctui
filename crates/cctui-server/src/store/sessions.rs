use cctui_proto::models::SessionStatus;
use sqlx::PgExecutor;

use crate::routes::sessions::DbSession;

/// Every value `sessions.status` may hold; migration 139 enforces the same set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRowStatus {
    New,
    Active,
    Inactive,
    Archived,
    Draft,
    Ended,
    Failed,
}

impl SessionRowStatus {
    pub const ALL: &'static [Self] = &[
        Self::New,
        Self::Active,
        Self::Inactive,
        Self::Archived,
        Self::Draft,
        Self::Ended,
        Self::Failed,
    ];
    /// Rows a daemon may still be running.
    pub const RUNNING: &'static [Self] = &[Self::New, Self::Active, Self::Inactive];
    /// Rows a keep-alive tick may be sent to.
    pub const KEEPALIVE: &'static [Self] = &[Self::Active, Self::Inactive];
    /// Rows auto-archive must never touch.
    pub const NOT_ARCHIVABLE: &'static [Self] = &[Self::Archived, Self::Draft];
    /// Rows auto-resume must never touch.
    pub const NOT_RESUMABLE: &'static [Self] =
        &[Self::Archived, Self::Ended, Self::Failed, Self::Draft];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Archived => "archived",
            Self::Draft => "draft",
            Self::Ended => "ended",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.as_str() == s)
    }

    /// The set as bindable text, for `status = ANY($n)` / `status <> ALL($n)`.
    #[must_use]
    pub fn names(set: &[Self]) -> Vec<&'static str> {
        set.iter().map(|s| s.as_str()).collect()
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Archived | Self::Ended | Self::Failed)
    }

    /// Persisted states the API reports as-is instead of re-deriving from
    /// heartbeat age.
    #[must_use]
    pub const fn is_sticky(self) -> bool {
        self.is_terminal() || matches!(self, Self::Draft)
    }

    #[must_use]
    pub const fn to_wire(self) -> SessionStatus {
        match self {
            Self::New => SessionStatus::New,
            Self::Active => SessionStatus::Active,
            Self::Inactive | Self::Ended | Self::Failed => SessionStatus::Inactive,
            Self::Archived => SessionStatus::Archived,
            Self::Draft => SessionStatus::Draft,
        }
    }
}

pub async fn set_inactive(
    exec: impl PgExecutor<'_>,
    id: &str,
    require_archived: bool,
) -> Result<(), sqlx::Error> {
    let sql = if require_archived {
        "UPDATE sessions SET status = 'inactive' WHERE id = $1 AND status = 'archived'"
    } else {
        "UPDATE sessions SET status = 'inactive' WHERE id = $1"
    };
    sqlx::query(sql).bind(id).execute(exec).await?;
    Ok(())
}

/// Insert a freshly registered session, or reset an existing row to `new`.
/// An existing row is only touched when it belongs to `user_id` on
/// `machine_uuid`; returns whether a row was written.
pub async fn upsert_registered(
    exec: impl PgExecutor<'_>,
    session: &cctui_proto::models::Session,
    machine_uuid: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    let written: Option<String> = sqlx::query_scalar(
        r"INSERT INTO sessions (id, parent_id, account_id, machine_id, machine_uuid, user_id, working_dir, status, registered_at, last_heartbeat, metadata, model)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'new', $8, $9, $10, NULLIF($10->>'model', ''))
           ON CONFLICT (id) DO UPDATE SET status = 'new', last_heartbeat = $9, metadata = $10, model = COALESCE(sessions.model, EXCLUDED.model)
           WHERE sessions.user_id = EXCLUDED.user_id AND sessions.machine_uuid = EXCLUDED.machine_uuid
           RETURNING id",
    )
    .bind(&session.id)
    .bind(&session.parent_id)
    .bind(&session.account_id)
    .bind(&session.machine_id)
    .bind(machine_uuid)
    .bind(user_id)
    .bind(&session.working_dir)
    .bind(session.registered_at)
    .bind(session.last_heartbeat)
    .bind(&session.metadata)
    .fetch_optional(exec)
    .await?;
    Ok(written.is_some())
}

/// The subset of `ids` whose session runs on a machine owned by `user_id`.
pub async fn visible_session_ids(
    exec: impl PgExecutor<'_>,
    ids: &[String],
    user_id: uuid::Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT s.id FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = ANY($1) AND m.user_id = $2",
    )
    .bind(ids)
    .bind(user_id)
    .fetch_all(exec)
    .await
}

pub async fn fetch_by_id(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<DbSession>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
                s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
                COALESCE(m.display_name, m.name) AS resolved_machine_name, \
                m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await
}

pub async fn adapter_id(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT adapter_id FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}

pub async fn working_dir(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT working_dir FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}

/// A session nested under a parent. `observe_only` marks a Task-tool subagent:
/// a transcript the daemon tails with no worker or job of its own, so it never
/// needs a `Remove`. Everything else (`CctuiAgent` children, forks) is a real job.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Child {
    pub id: String,
    pub observe_only: bool,
    /// Generations below the archived root: a direct child is 1.
    pub depth: i32,
}

/// Cap on the recursion, so a `parent_id` cycle cannot spin the query.
const MAX_DESCENDANT_DEPTH: i32 = 32;

/// Every session nested under `root`, at any depth, deepest first.
///
/// Depth order is what the caller needs: a live grandchild holds its parent's
/// worktree, so it must be removed before that parent is.
pub async fn descendants(exec: impl PgExecutor<'_>, root: &str) -> Result<Vec<Child>, sqlx::Error> {
    sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
             SELECT id, 1 AS depth FROM sessions WHERE parent_id = $1 \
             UNION ALL \
             SELECT s.id, t.depth + 1 FROM sessions s \
               JOIN tree t ON s.parent_id = t.id \
              WHERE t.depth < $2 \
         ) \
         SELECT t.id, \
                COALESCE((s.metadata->>'subagent')::boolean, false) AS observe_only, \
                t.depth \
           FROM tree t JOIN sessions s ON s.id = t.id \
          ORDER BY t.depth DESC, t.id",
    )
    .bind(root)
    .bind(MAX_DESCENDANT_DEPTH)
    .fetch_all(exec)
    .await
}

/// Archived descendants that own a claude job and so need a `Remove` of their
/// own, keeping the deepest-first order of [`descendants`].
pub fn job_children<'a>(children: &'a [Child], archived: &[String]) -> Vec<&'a str> {
    let mut jobs: Vec<&Child> = children
        .iter()
        .filter(|c| !c.observe_only && archived.iter().any(|id| id == &c.id))
        .collect();
    jobs.sort_by_key(|c| std::cmp::Reverse(c.depth));
    jobs.into_iter().map(|c| c.id.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::{Child, SessionRowStatus, descendants, job_children, upsert_registered};

    #[test]
    fn row_status_round_trips_through_text() {
        for s in SessionRowStatus::ALL {
            assert_eq!(SessionRowStatus::parse(s.as_str()), Some(*s));
        }
        assert_eq!(SessionRowStatus::parse("registering"), None);
    }

    #[test]
    fn check_constraint_lists_exactly_the_enum() {
        let migration = include_str!("../../../../migrations/139_sessions_status_check.up.sql");
        let quoted: Vec<String> =
            SessionRowStatus::ALL.iter().map(|s| format!("'{}'", s.as_str())).collect();
        assert!(
            migration.contains(&format!("CHECK (status IN ({}))", quoted.join(", "))),
            "migration 139's CHECK drifted from SessionRowStatus::ALL"
        );
        let default = format!("SET DEFAULT '{}'", SessionRowStatus::New.as_str());
        assert!(migration.contains(&default), "sessions.status default must satisfy the CHECK");
    }

    #[test]
    fn wire_status_collapses_ended_and_failed_to_inactive() {
        use cctui_proto::models::SessionStatus;
        assert!(matches!(SessionRowStatus::Ended.to_wire(), SessionStatus::Inactive));
        assert!(matches!(SessionRowStatus::Failed.to_wire(), SessionStatus::Inactive));
        assert!(matches!(SessionRowStatus::Archived.to_wire(), SessionStatus::Archived));
        assert!(matches!(SessionRowStatus::Draft.to_wire(), SessionStatus::Draft));
        assert!(!SessionRowStatus::Active.is_sticky());
        assert!(SessionRowStatus::Draft.is_sticky() && !SessionRowStatus::Draft.is_terminal());
    }

    fn child(id: &str, observe_only: bool) -> Child {
        at_depth(id, observe_only, 1)
    }

    fn at_depth(id: &str, observe_only: bool, depth: i32) -> Child {
        Child { id: id.to_owned(), observe_only, depth }
    }

    #[test]
    fn job_children_skips_observe_only_and_unarchived() {
        let kids = [child("job-a", false), child("task-b", true), child("pinned-c", false)];
        let archived = ["job-a".to_owned(), "task-b".to_owned(), "parent".to_owned()];
        assert_eq!(job_children(&kids, &archived), ["job-a"]);
    }

    #[test]
    fn job_children_orders_the_deepest_descendant_first() {
        let kids = [
            at_depth("child", false, 1),
            at_depth("great-grandchild", false, 3),
            at_depth("grandchild", false, 2),
            at_depth("grandchild-task", true, 2),
        ];
        let archived: Vec<String> = ["child", "grandchild", "great-grandchild", "grandchild-task"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(job_children(&kids, &archived), ["great-grandchild", "grandchild", "child"]);
    }

    #[tokio::test]
    async fn descendants_recurse_and_flag_task_subagents_only() {
        let name = "descendants_recurse_and_flag_task_subagents_only";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
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
        let parent = format!("{name}-parent-{uid}");
        let job = format!("{parent}-job");
        let rows: [(&str, Option<&str>, serde_json::Value); 5] = [
            (&parent, None, serde_json::json!({"short": "aaaaaaaa"})),
            (
                "job",
                Some(&parent),
                serde_json::json!({"short": "bbbbbbbb", "relation": "subagent"}),
            ),
            ("task", Some(&parent), serde_json::json!({"subagent": true, "agent_id": "a1"})),
            ("bare", Some(&parent), serde_json::json!({})),
            ("grandchild", Some(&job), serde_json::json!({"short": "cccccccc"})),
        ];
        for (suffix, parent_id, metadata) in rows {
            let id =
                if parent_id.is_none() { suffix.to_owned() } else { format!("{parent}-{suffix}") };
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, machine_uuid, user_id, \
                 working_dir, status, metadata) VALUES ($1, $2, $3, $3, $4, '/w', 'active', $5)",
            )
            .bind(&id)
            .bind(parent_id)
            .bind(machine)
            .bind(uid)
            .bind(metadata)
            .execute(&pool)
            .await
            .unwrap();
        }

        let got = descendants(&pool, &parent).await.unwrap();
        // Deepest first; ties broken by id.
        assert_eq!(
            got,
            [
                at_depth(&format!("{parent}-grandchild"), false, 2),
                child(&format!("{parent}-bare"), false),
                child(&format!("{parent}-job"), false),
                child(&format!("{parent}-task"), true),
            ]
        );

        let archived: Vec<String> = got.iter().map(|c| c.id.clone()).collect();
        let order = job_children(&got, &archived);
        let position = |id: &str| order.iter().position(|seen| *seen == id).unwrap();
        assert!(
            position(&format!("{parent}-grandchild")) < position(&format!("{parent}-job")),
            "a grandchild must be removed before the child that owns its worktree"
        );
        assert!(!order.contains(&format!("{parent}-task").as_str()));

        for sql in [
            "DELETE FROM sessions WHERE user_id = $1",
            "DELETE FROM machines WHERE user_id = $1",
            "DELETE FROM users WHERE id = $1",
        ] {
            sqlx::query(sql).bind(uid).execute(&pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn register_only_touches_the_callers_own_row() {
        use uuid::Uuid;
        let Some(url) = crate::routes::gateway::test_db_url("register_own_row") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (me, other) = (Uuid::new_v4(), Uuid::new_v4());
        let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
        for uid in [me, other] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(uid)
                .bind(format!("reg-{uid}"))
                .bind(format!("kh-{uid}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        for (machine, uid) in [(mine, me), (theirs, other)] {
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let existing = [
            (Some(me), Some(mine), true),
            (Some(me), Some(theirs), false),
            (Some(other), Some(mine), false),
            (None, Some(mine), false),
            (Some(me), None, false),
            (None, None, false),
        ];
        for (user, machine, expect) in existing {
            let sid = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, status, metadata) \
                 VALUES ($1, 'm', '/w', $2, $3, 'active', '{\"k\":\"orig\"}'::jsonb)",
            )
            .bind(&sid)
            .bind(user)
            .bind(machine)
            .execute(&pool)
            .await
            .unwrap();
            let now = chrono::Utc::now();
            let session = cctui_proto::models::Session {
                id: sid.clone(),
                parent_id: None,
                account_id: None,
                machine_id: "m".into(),
                working_dir: "/w".into(),
                status: cctui_proto::models::SessionStatus::New,
                registered_at: now,
                last_heartbeat: now,
                metadata: serde_json::json!({"k": "new"}),
                adapter_id: None,
            };
            let written = upsert_registered(&pool, &session, mine, me).await.unwrap();
            assert_eq!(written, expect, "user {user:?} machine {machine:?}");
            let (status, owner, uuid): (String, Option<Uuid>, Option<Uuid>) =
                sqlx::query_as("SELECT status, user_id, machine_uuid FROM sessions WHERE id = $1")
                    .bind(&sid)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if expect {
                assert_eq!(status, "new");
            } else {
                assert_eq!((status.as_str(), owner, uuid), ("active", user, machine));
            }
        }
    }
}
