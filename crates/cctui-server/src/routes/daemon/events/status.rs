use cctui_proto::adapter::AdapterEvent;

use crate::state::AppState;

/// Latest classifier signals + display metadata from a Status event.
struct StatusSignals<'a> {
    tempo: Option<&'a str>,
    agent_state: Option<&'a str>,
    activity: Option<&'a str>,
    name: Option<&'a str>,
    intent: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    permission_mode: Option<String>,
    children: &'a [cctui_proto::adapter::SessionChild],
}

/// Persist the latest Status signals onto the session row. `COALESCE` keeps
/// the stored value when a Status event omits a field, so a sparse update
/// never clears signal. `model` is special-cased to
/// `COALESCE(model, $6)` (fill only when NULL): the requested model must not
/// overwrite the init-frame ground truth that `SessionModel` writes.
/// `effort` is safe to overwrite because the daemon reports the observed
/// (`/proc CLAUDE_EFFORT`) value in Status, not the requested one.
async fn update_status_signals(
    state: &AppState,
    local_id: &str,
    s: StatusSignals<'_>,
) -> anyhow::Result<()> {
    let decorated = s.name.and_then(crate::session_emoji::decorate);
    let row = write_status_signals(&state.pool, local_id, &s, decorated.as_deref()).await?;

    // Only a genuinely new decorated name goes to the picker model, so a title
    // re-reported on every Status costs no second call.
    if let Some((old_name, true)) = row
        && let (Some(name), Some(decorated)) = (s.name, decorated.as_deref())
        && state.config.emoji_picker().is_some()
    {
        let already = old_name.as_deref().map(crate::session_emoji::strip_emoji);
        if already != Some(name) {
            spawn_emoji_refine(state, local_id, name, decorated);
        }
    }
    Ok(())
}

async fn write_status_signals(
    pool: &sqlx::PgPool,
    local_id: &str,
    s: &StatusSignals<'_>,
    decorated: Option<&str>,
) -> anyhow::Result<Option<(Option<String>, bool)>> {
    let children = serde_json::to_value(s.children).unwrap_or_else(|_| serde_json::json!([]));
    // `metadata.agent_title` is the exact name cctui last wrote from an agent
    // title: a stored name still equal to it is agent-owned and claimable, any
    // other name was typed by the user and an agent title must not touch it.
    //
    // With `sessionEmojiPrefix` on, the SQL stores the decorated form when the
    // incoming name differs from the stored one. A stored name that is the
    // incoming one behind an emoji prefix is left alone, or every Status would
    // paste the table's emoji back over the model's. The whole prefix is
    // matched, so a new title that merely ends the stored name still lands.
    let row: Option<(Option<String>, bool)> = sqlx::query_as(
        "WITH claim AS ( \
            SELECT s.id, \
                   s.session_name AS old_name, \
                   COALESCE((SELECT us.data->'sessionEmojiPrefix' = 'true'::jsonb \
                             FROM user_settings us WHERE us.user_id = s.user_id), false) \
                     AS emoji_on, \
                   (COALESCE(s.session_name, '') = '' \
                    OR s.session_name IS NOT DISTINCT FROM s.metadata->>'agent_title') \
                     AS claimable \
            FROM sessions s WHERE s.id = $1 \
         ), \
         prev AS ( \
            SELECT c.id, c.old_name, c.emoji_on, \
                   CASE \
                       WHEN $5::text IS NULL OR NOT c.claimable THEN NULL \
                       WHEN c.old_name IS NOT DISTINCT FROM $5::text THEN NULL \
                       WHEN c.emoji_on \
                            AND right(c.old_name, length($5::text)) = $5::text \
                            AND left(c.old_name, \
                                     length(c.old_name) - length($5::text)) \
                                ~ '^[^[:alnum:][:space:]]+ $' \
                           THEN NULL \
                       WHEN c.emoji_on AND $10::text IS NOT NULL THEN $10::text \
                       ELSE $5::text \
                   END AS new_name \
            FROM claim c \
         ) \
         UPDATE sessions SET \
            tempo = COALESCE($2, sessions.tempo), \
            agent_state = COALESCE($3, sessions.agent_state), \
            activity = COALESCE($4, sessions.activity), \
            session_name = COALESCE(prev.new_name, sessions.session_name), \
            metadata = CASE \
                WHEN prev.new_name IS NULL THEN sessions.metadata \
                ELSE COALESCE(sessions.metadata, '{}'::jsonb) \
                     || jsonb_build_object('agent_title', prev.new_name) \
            END, \
            intent = COALESCE($6, sessions.intent), \
            model = COALESCE(sessions.model, $7), \
            effort = COALESCE($8, sessions.effort), \
            permission_mode = COALESCE($11, sessions.permission_mode), \
            children = CASE WHEN jsonb_array_length($9) > 0 THEN $9 ELSE sessions.children END \
         FROM prev \
         WHERE sessions.id = prev.id \
           AND (prev.new_name IS NOT NULL \
                OR (sessions.tempo, sessions.agent_state, sessions.activity, sessions.intent, \
                    sessions.model, sessions.effort, sessions.permission_mode, sessions.children) \
                   IS DISTINCT FROM \
                   (COALESCE($2, sessions.tempo), COALESCE($3, sessions.agent_state), \
                    COALESCE($4, sessions.activity), COALESCE($6, sessions.intent), \
                    COALESCE(sessions.model, $7), COALESCE($8, sessions.effort), \
                    COALESCE($11, sessions.permission_mode), \
                    CASE WHEN jsonb_array_length($9) > 0 THEN $9 ELSE sessions.children END)) \
         RETURNING prev.old_name, prev.emoji_on",
    )
    .bind(local_id)
    .bind(s.tempo)
    .bind(s.agent_state)
    .bind(s.activity)
    .bind(s.name)
    .bind(s.intent)
    .bind(s.model)
    .bind(s.effort)
    .bind(children)
    .bind(decorated)
    .bind(s.permission_mode.as_deref())
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Ask the configured picker model for a better emoji than the table's, in the
/// background, and swap it in.
///
/// Fire-and-forget on purpose: the name is already decorated and visible, so a
/// slow or failing picker costs nothing but the table's emoji. The write is
/// guarded on the exact value we wrote, so a rename or a newer title landing
/// meanwhile wins instead of being clobbered by a stale answer.
fn spawn_emoji_refine(state: &AppState, local_id: &str, name: &str, decorated: &str) {
    let (Some(picker), client) = (state.config.emoji_picker(), state.http_client.clone()) else {
        return;
    };
    let (endpoint, model, token) =
        (picker.endpoint.to_owned(), picker.model.to_owned(), picker.token.map(str::to_owned));
    let (pool, id) = (state.pool.clone(), local_id.to_owned());
    let (plain, guard) = (name.to_owned(), decorated.to_owned());
    tokio::spawn(async move {
        let picker = crate::session_emoji::Picker {
            endpoint: &endpoint,
            model: &model,
            token: token.as_deref(),
        };
        let Some(emoji) = crate::session_emoji::pick_with_model(&client, picker, &plain).await
        else {
            return;
        };
        let refined = format!("{emoji} {plain}");
        if refined == guard {
            return;
        }
        let _ = sqlx::query(
            "UPDATE sessions SET session_name = $2, \
                metadata = COALESCE(metadata, '{}'::jsonb) \
                           || jsonb_build_object('agent_title', $2::text) \
             WHERE id = $1 AND session_name = $3",
        )
        .bind(&id)
        .bind(&refined)
        .bind(&guard)
        .execute(&pool)
        .await;
    });
}

/// Fill the session row's `children` from a transcript `pr-link` line, but only
/// when it has none: an authoritative `Status` snapshot (from `state.json`) must
/// always win, so the transcript source is a gap-filler for sessions whose
/// `state.json` carries no children.
async fn persist_pr_link_children(
    state: &AppState,
    local_id: &str,
    children: &[cctui_proto::adapter::SessionChild],
) -> anyhow::Result<()> {
    if children.is_empty() {
        return Ok(());
    }
    let children = serde_json::to_value(children).unwrap_or_else(|_| serde_json::json!([]));
    sqlx::query(
        "UPDATE sessions SET children = $2 \
         WHERE id = $1 AND (children IS NULL OR children = '[]'::jsonb)",
    )
    .bind(local_id)
    .bind(children)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Status snapshots, transcript PR links and rate-limit windows.
pub(super) async fn on_status_event(state: &AppState, event: AdapterEvent) -> anyhow::Result<()> {
    match event {
        AdapterEvent::Status {
            local_id,
            tempo,
            state: agent_state,
            detail: _,
            activity,
            name,
            intent,
            model,
            effort,
            permission_mode,
            children,
        } => {
            // Persist the classifier signals + display metadata so
            // `list_sessions` can derive the "needs input" attention flag and
            // show name/model/effort. Status events are otherwise not stored
            // as stream_events (heartbeat bump below handles liveness).
            update_status_signals(
                state,
                &local_id,
                StatusSignals {
                    tempo: tempo.as_deref(),
                    agent_state: agent_state.as_deref(),
                    activity: activity.as_deref(),
                    name: name.as_deref(),
                    intent: intent.as_deref(),
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                    permission_mode: permission_mode
                        .and_then(|m| serde_json::to_value(m).ok())
                        .and_then(|v| v.as_str().map(str::to_owned)),
                    children: &children,
                },
            )
            .await?;
        }
        AdapterEvent::PrLink { local_id, children } => {
            persist_pr_link_children(state, &local_id, &children).await?;
        }
        AdapterEvent::RateLimits { local_id, windows, observed_at } => {
            crate::usage_history::record_agent_limits(state, local_id, &windows, observed_at);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::routes::daemon::test_support::{
        drop_machines, row_version, seed_machine, seed_owned_session,
    };

    #[tokio::test]
    async fn a_repeated_status_writes_nothing() {
        let Some(url) = crate::routes::gateway::test_db_url("unchanged_status") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "status").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        let signals = StatusSignals {
            tempo: Some("active"),
            agent_state: Some("working"),
            activity: None,
            name: None,
            intent: None,
            model: Some("m"),
            effort: None,
            permission_mode: None,
            children: &[],
        };
        let first = write_status_signals(&pool, &sid, &signals, None).await.expect("first");
        assert!(first.is_some());
        let before = row_version(&pool, &sid).await;
        let again = write_status_signals(&pool, &sid, &signals, None).await.expect("again");
        assert!(again.is_none());
        assert_eq!(row_version(&pool, &sid).await, before);

        drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }

    /// Fixture for the agent-title precedence tests: an isolated user, machine
    /// and session per case, driving the real `write_status_signals` path.
    struct TitleFixture {
        pool: sqlx::PgPool,
        user: Uuid,
        machine: Uuid,
    }

    impl TitleFixture {
        async fn new(test_name: &str) -> Option<Self> {
            let url = crate::routes::gateway::test_db_url(test_name)?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db");
            let (user, machine) = (Uuid::new_v4(), Uuid::new_v4());
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(user)
                .bind(format!("title-{user}"))
                .bind(format!("kh-{user}"))
                .execute(&pool)
                .await
                .expect("seed user");
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(machine)
            .bind(user)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");
            Some(Self { pool, user, machine })
        }

        async fn session(&self) -> String {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id) VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
            )
            .bind(&id)
            .bind(self.machine.to_string())
            .bind(self.user)
            .bind(self.machine)
            .execute(&self.pool)
            .await
            .expect("seed session");
            id
        }

        /// One Status event carrying an agent-generated title.
        async fn agent_title(&self, id: &str, name: &str) {
            let signals = StatusSignals {
                tempo: None,
                agent_state: None,
                activity: None,
                name: Some(name),
                intent: None,
                model: None,
                effort: None,
                permission_mode: None,
                children: &[],
            };
            write_status_signals(&self.pool, id, &signals, None).await.expect("status write");
        }

        /// What `rename_session` does: a bare name write, no provenance marker.
        async fn rename(&self, id: &str, name: &str) {
            sqlx::query("UPDATE sessions SET session_name = $2 WHERE id = $1")
                .bind(id)
                .bind(name)
                .execute(&self.pool)
                .await
                .expect("rename");
        }

        async fn name_of(&self, id: &str) -> Option<String> {
            let (name,): (Option<String>,) =
                sqlx::query_as("SELECT session_name FROM sessions WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pool)
                    .await
                    .expect("read name");
            name
        }

        async fn cleanup(self) {
            sqlx::query("DELETE FROM sessions WHERE machine_uuid = $1")
                .bind(self.machine)
                .execute(&self.pool)
                .await
                .expect("cleanup sessions");
            let _ = sqlx::query("DELETE FROM machines WHERE id = $1")
                .bind(self.machine)
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(self.user)
                .execute(&self.pool)
                .await;
        }
    }

    /// An agent title owns a name it wrote — it fills an empty one and
    /// replaces its own earlier title.
    #[tokio::test]
    async fn an_agent_title_fills_an_empty_name_and_replaces_an_agent_title() {
        let Some(fx) = TitleFixture::new("agent_title_claims_agent_owned_name").await else {
            return;
        };
        let id = fx.session().await;

        fx.agent_title(&id, "first agent title").await;
        assert_eq!(
            fx.name_of(&id).await.as_deref(),
            Some("first agent title"),
            "an agent title fills an empty name"
        );

        fx.agent_title(&id, "second agent title").await;
        assert_eq!(
            fx.name_of(&id).await.as_deref(),
            Some("second agent title"),
            "an agent title replaces a previous agent title"
        );

        fx.cleanup().await;
    }

    /// A name the user typed outranks any agent
    /// title, whether it was typed at spawn or renamed over an agent title.
    #[tokio::test]
    async fn an_agent_title_never_overwrites_a_user_set_name() {
        let Some(fx) = TitleFixture::new("agent_title_yields_to_user_name").await else {
            return;
        };

        let typed = fx.session().await;
        fx.rename(&typed, "name the human typed").await;
        fx.agent_title(&typed, "agent title").await;
        assert_eq!(
            fx.name_of(&typed).await.as_deref(),
            Some("name the human typed"),
            "a user-set name outranks any agent title"
        );

        let renamed = fx.session().await;
        fx.agent_title(&renamed, "third agent title").await;
        fx.rename(&renamed, "renamed by hand").await;
        fx.agent_title(&renamed, "fourth agent title").await;
        assert_eq!(
            fx.name_of(&renamed).await.as_deref(),
            Some("renamed by hand"),
            "a rename over an agent title makes the name user-owned"
        );

        fx.cleanup().await;
    }
}
