use cctui_proto::adapter::{AdapterEvent, TitleSource};
use cctui_proto::classifier::Bucket;

use crate::context::SessionsContext;

/// Latest classifier signals + display metadata from a Status event.
pub struct StatusSignals<'a> {
    pub tempo: Option<&'a str>,
    pub agent_state: Option<&'a str>,
    pub activity: Option<&'a str>,
    pub name: Option<&'a str>,
    /// `name` is known to be agent-generated, so a pinned session keeps it as
    /// its `agent_title`.
    pub agent_title: bool,
    pub intent: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub permission_mode: Option<String>,
    pub children: &'a [cctui_proto::adapter::SessionChild],
}

/// Persist the latest Status signals onto the session row. `COALESCE` keeps
/// the stored value when a Status event omits a field, so a sparse update
/// never clears signal; [`next_tempo`] is the one exception. `model` is special-cased to
/// `COALESCE(model, $6)` (fill only when NULL): the requested model must not
/// overwrite the init-frame ground truth that `SessionModel` writes.
/// `effort` is safe to overwrite because the daemon reports the observed
/// (`/proc CLAUDE_EFFORT`) value in Status, not the requested one.
async fn update_status_signals<S: SessionsContext>(
    state: &S,
    local_id: &str,
    s: StatusSignals<'_>,
) -> anyhow::Result<()> {
    let decorated = s.name.and_then(crate::session_emoji::decorate);
    let row = write_status_signals(state.pool(), local_id, &s, decorated.as_deref()).await?;

    if row.as_ref().is_some_and(|written| written.turn_went_idle(&s)) {
        crate::outcome::turn_ended(state, local_id);
    }

    // Only a genuinely new decorated name goes to the picker model, so a title
    // re-reported on every Status costs no second call.
    if let Some(Written { old_name, emoji_on: true, renamed: true, .. }) = row
        && let (Some(name), Some(decorated)) = (s.name, decorated.as_deref())
        && state.config().emoji_picker().is_some()
    {
        let already = old_name.as_deref().map(crate::session_emoji::strip_emoji);
        if already != Some(name) {
            spawn_emoji_refine(state, local_id, name, decorated);
        }
    }
    Ok(())
}

/// The row as a Status write found it.
#[derive(sqlx::FromRow)]
pub struct Written {
    old_name: Option<String>,
    emoji_on: bool,
    renamed: bool,
    old_tempo: Option<String>,
    old_state: Option<String>,
    old_activity: Option<String>,
}

impl Written {
    /// The write took the session from Working to Completed: its turn is over
    /// even if no turn end was signalled.
    fn turn_went_idle(&self, s: &StatusSignals<'_>) -> bool {
        let bucket = |tempo, state, activity| {
            crate::routes::sessions::bucket_from_signals(
                tempo,
                state,
                activity,
                None,
                &[],
                &std::collections::HashMap::new(),
            )
        };
        let was =
            (self.old_tempo.as_deref(), self.old_state.as_deref(), self.old_activity.as_deref());
        if (was.0.is_none() && was.1.is_none() && was.2.is_none())
            || bucket(was.0, was.1, was.2) != Bucket::Working
        {
            return false;
        }
        let tempo = next_tempo(s.tempo, s.agent_state, was.0);
        bucket(tempo, s.agent_state.or(was.1), s.activity.or(was.2)) == Bucket::Done
    }
}

/// The tempo a Status write leaves on the row; mirrors the `tempo` column in
/// [`write_status_signals`]. An adapter that reports idle only through
/// `state` (codex) must still end a stored `active`, while `blocked` and
/// `hibernated` survive.
fn next_tempo<'a>(
    tempo: Option<&'a str>,
    state: Option<&str>,
    stored: Option<&'a str>,
) -> Option<&'a str> {
    match (tempo, state, stored) {
        (None, Some("idle"), Some("active")) => Some("idle"),
        _ => tempo.or(stored),
    }
}

pub async fn write_status_signals(
    pool: &sqlx::PgPool,
    local_id: &str,
    s: &StatusSignals<'_>,
    decorated: Option<&str>,
) -> anyhow::Result<Option<Written>> {
    let children = serde_json::to_value(s.children).unwrap_or_else(|_| serde_json::json!([]));
    // `metadata.agent_title` is the exact name cctui last wrote from an agent
    // title: a stored name still equal to it is agent-owned and claimable, any
    // other name was typed by the user and an agent title must not touch it.
    // `metadata.name_source = 'user'` pins the name outright. A known agent
    // title that cannot claim the name is kept as `agent_title` beside it, and
    // pins it.
    //
    // With `sessionEmojiPrefix` on, the SQL stores the decorated form when the
    // incoming name differs from the stored one. A stored name that is the
    // incoming one behind an emoji prefix is left alone, or every Status would
    // paste the table's emoji back over the model's. The whole prefix is
    // matched, so a new title that merely ends the stored name still lands.
    let row: Option<Written> = sqlx::query_as(
        "WITH claim AS ( \
            SELECT s.id, \
                   s.tempo AS old_tempo, s.agent_state AS old_state, \
                   s.activity AS old_activity, \
                   s.session_name AS old_name, \
                   s.metadata->>'agent_title' AS agent_title, \
                   COALESCE((SELECT us.data->'sessionEmojiPrefix' = 'true'::jsonb \
                             FROM user_settings us WHERE us.user_id = s.user_id), false) \
                     AS emoji_on, \
                   ((COALESCE(s.session_name, '') = '' \
                     OR s.session_name IS NOT DISTINCT FROM s.metadata->>'agent_title') \
                    AND COALESCE(s.metadata->>'name_source', '') <> 'user') \
                     AS claimable, \
                   CASE WHEN COALESCE(s.session_name, '') = '' \
                             AND s.metadata->>'name_source' = 'user' \
                        THEN NULLIF(s.metadata->>'custom_title', '') END AS pinned_title \
            FROM sessions s WHERE s.id = $1 \
         ), \
         prev AS ( \
            SELECT c.id, c.old_name, c.emoji_on, c.pinned_title, \
                   c.old_tempo, c.old_state, c.old_activity, \
                   CASE WHEN $12 AND $5::text IS NOT NULL AND NOT c.claimable \
                             AND c.agent_title IS DISTINCT FROM $5::text \
                        THEN $5::text END AS kept_title, \
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
            tempo = CASE WHEN $2::text IS NULL AND $3::text = 'idle' \
                              AND sessions.tempo = 'active' \
                         THEN 'idle' ELSE COALESCE($2, sessions.tempo) END, \
            agent_state = COALESCE($3, sessions.agent_state), \
            activity = COALESCE($4, sessions.activity), \
            session_name = COALESCE(prev.new_name, prev.pinned_title, sessions.session_name), \
            metadata = CASE \
                WHEN prev.new_name IS NOT NULL THEN COALESCE(sessions.metadata, '{}'::jsonb) \
                     || jsonb_build_object('agent_title', prev.new_name) \
                WHEN prev.kept_title IS NOT NULL THEN COALESCE(sessions.metadata, '{}'::jsonb) \
                     || jsonb_build_object('agent_title', prev.kept_title, 'name_source', 'user') \
                ELSE sessions.metadata \
            END, \
            intent = COALESCE($6, sessions.intent), \
            model = COALESCE(sessions.model, $7), \
            effort = COALESCE($8, sessions.effort), \
            permission_mode = COALESCE($11, sessions.permission_mode), \
            children = CASE WHEN jsonb_array_length($9) > 0 THEN $9 ELSE sessions.children END \
         FROM prev \
         WHERE sessions.id = prev.id \
           AND (prev.new_name IS NOT NULL OR prev.kept_title IS NOT NULL \
                OR prev.pinned_title IS NOT NULL \
                OR (sessions.tempo, sessions.agent_state, sessions.activity, sessions.intent, \
                    sessions.model, sessions.effort, sessions.permission_mode, sessions.children) \
                   IS DISTINCT FROM \
                   (CASE WHEN $2::text IS NULL AND $3::text = 'idle' \
                              AND sessions.tempo = 'active' \
                         THEN 'idle' ELSE COALESCE($2, sessions.tempo) END, \
                    COALESCE($3, sessions.agent_state), \
                    COALESCE($4, sessions.activity), COALESCE($6, sessions.intent), \
                    COALESCE(sessions.model, $7), COALESCE($8, sessions.effort), \
                    COALESCE($11, sessions.permission_mode), \
                    CASE WHEN jsonb_array_length($9) > 0 THEN $9 ELSE sessions.children END)) \
         RETURNING prev.old_name, prev.emoji_on, prev.new_name IS NOT NULL AS renamed, \
                   prev.old_tempo, prev.old_state, prev.old_activity",
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
    .bind(s.agent_title)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Pin a title the user chose. `metadata.custom_title` remembers the last one
/// the agent reported, so a re-reported or replayed title never undoes a later
/// rename from the webui.
pub async fn pin_user_title(
    pool: &sqlx::PgPool,
    local_id: &str,
    title: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sessions SET session_name = $2, \
            metadata = COALESCE(metadata, '{}'::jsonb) \
                       || jsonb_build_object('name_source', 'user', 'custom_title', $2::text) \
         WHERE id = $1 \
           AND (metadata->>'custom_title' IS DISTINCT FROM $2::text \
                OR COALESCE(session_name, '') = '')",
    )
    .bind(local_id)
    .bind(title)
    .execute(pool)
    .await?;
    Ok(())
}

async fn on_title<S: SessionsContext>(
    state: &S,
    local_id: &str,
    title: &str,
    source: TitleSource,
) -> anyhow::Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(());
    }
    match source {
        TitleSource::User => pin_user_title(state.pool(), local_id, title).await,
        TitleSource::Agent => {
            update_status_signals(state, local_id, agent_title_signals(title)).await
        }
    }
}

#[must_use]
pub const fn agent_title_signals(title: &str) -> StatusSignals<'_> {
    StatusSignals {
        tempo: None,
        agent_state: None,
        activity: None,
        name: Some(title),
        agent_title: true,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: &[],
    }
}

/// Ask the configured picker model for a better emoji than the table's, in the
/// background, and swap it in.
///
/// Fire-and-forget on purpose: the name is already decorated and visible, so a
/// slow or failing picker costs nothing but the table's emoji. The write is
/// guarded on the exact value we wrote, so a rename or a newer title landing
/// meanwhile wins instead of being clobbered by a stale answer.
fn spawn_emoji_refine<S: SessionsContext>(state: &S, local_id: &str, name: &str, decorated: &str) {
    let (Some(picker), client) = (state.config().emoji_picker(), state.http_client().clone())
    else {
        return;
    };
    let (endpoint, model, token) =
        (picker.endpoint.to_owned(), picker.model.to_owned(), picker.token);
    let (pool, id) = (state.pool().clone(), local_id.to_owned());
    let (plain, guard) = (name.to_owned(), decorated.to_owned());
    tokio::spawn(async move {
        let picker = crate::session_emoji::Picker { endpoint: &endpoint, model: &model, token };
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
async fn persist_pr_link_children<S: SessionsContext>(
    state: &S,
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
    .execute(state.pool())
    .await?;
    Ok(())
}

/// Status snapshots, transcript PR links and rate-limit windows.
pub async fn on_status_event<S: SessionsContext>(
    state: &S,
    event: AdapterEvent,
) -> anyhow::Result<()> {
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
                    agent_title: false,
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
        AdapterEvent::Title { local_id, title, source } => {
            on_title(state, &local_id, &title, source).await?;
        }
        AdapterEvent::PrLink { local_id, children } => {
            persist_pr_link_children(state, &local_id, &children).await?;
        }
        AdapterEvent::RateLimits { local_id, windows, observed_at } => {
            state.record_agent_limits(local_id, &windows, observed_at);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;

    fn written(tempo: Option<&str>, state: Option<&str>) -> Written {
        Written {
            old_name: None,
            emoji_on: false,
            renamed: false,
            old_tempo: tempo.map(str::to_owned),
            old_state: state.map(str::to_owned),
            old_activity: None,
        }
    }

    fn signals<'a>(tempo: Option<&'a str>, state: Option<&'a str>) -> StatusSignals<'a> {
        StatusSignals { tempo, agent_state: state, ..agent_title_signals("") }
    }

    #[test]
    fn a_turn_went_idle_only_when_a_working_session_settles() {
        let working = written(Some("active"), Some("working"));
        assert!(working.turn_went_idle(&signals(Some("idle"), Some("done"))));
        assert!(working.turn_went_idle(&signals(Some("hibernated"), Some("stopped"))));
        assert!(!working.turn_went_idle(&signals(Some("blocked"), None)), "a question is open");
        assert!(!working.turn_went_idle(&signals(None, None)), "a title update");
        assert!(!working.turn_went_idle(&signals(None, Some("done"))), "tempo is still active");

        let idle = written(Some("idle"), Some("done"));
        assert!(!idle.turn_went_idle(&signals(Some("idle"), Some("done"))));
        assert!(!written(None, None).turn_went_idle(&signals(Some("idle"), Some("idle"))));
        let blocked = written(Some("blocked"), Some("working"));
        assert!(!blocked.turn_went_idle(&signals(Some("idle"), Some("done"))));
    }
    #[test]
    fn an_idle_state_ends_a_stored_active_tempo_only() {
        assert_eq!(next_tempo(None, Some("idle"), Some("active")), Some("idle"));
        assert_eq!(next_tempo(None, Some("idle"), Some("hibernated")), Some("hibernated"));
        assert_eq!(next_tempo(None, Some("idle"), Some("blocked")), Some("blocked"));
        assert_eq!(next_tempo(None, Some("working"), Some("active")), Some("active"));
        assert_eq!(next_tempo(Some("blocked"), Some("idle"), Some("active")), Some("blocked"));
        assert_eq!(next_tempo(None, Some("idle"), None), None);
    }

    #[test]
    fn a_codex_idle_status_ends_the_turn() {
        let working = written(Some("active"), Some("working"));
        assert!(working.turn_went_idle(&signals(None, Some("idle"))));
        let asking = written(Some("blocked"), Some("working"));
        assert!(!asking.turn_went_idle(&signals(None, Some("idle"))));
    }

    use crate::routes::daemon::test_support::{
        drop_machines, row_version, seed_machine, seed_owned_session,
    };

    #[tokio::test]
    async fn a_repeated_status_writes_nothing() {
        let Some(url) = cctui_server_kernel::test_support::db_url("unchanged_status") else {
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
            agent_title: false,
            intent: None,
            model: Some("m"),
            effort: None,
            permission_mode: None,
            children: &[],
        };
        let first = write_status_signals(&pool, &sid, &signals, None).await.expect("first");
        assert!(first.is_some_and(|w| !w.turn_went_idle(&signals)), "a first report ends no turn");
        let before = row_version(&pool, &sid).await;
        let again = write_status_signals(&pool, &sid, &signals, None).await.expect("again");
        assert!(again.is_none());
        assert_eq!(row_version(&pool, &sid).await, before);

        let idle = StatusSignals { tempo: Some("idle"), agent_state: Some("done"), ..signals };
        let settled = write_status_signals(&pool, &sid, &idle, None).await.expect("idle");
        assert!(
            settled.is_some_and(|w| w.turn_went_idle(&idle)),
            "going idle ends the turn without a turn end"
        );

        drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }

    async fn write(
        pool: &sqlx::PgPool,
        sid: &str,
        tempo: Option<&str>,
        state: &str,
    ) -> Option<Written> {
        write_status_signals(pool, sid, &signals(tempo, Some(state)), None)
            .await
            .expect("status write")
    }

    async fn bucket_of(pool: &sqlx::PgPool, sid: &str) -> (Option<String>, Bucket) {
        let (tempo, state, activity): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as("SELECT tempo, agent_state, activity FROM sessions WHERE id = $1")
                .bind(sid)
                .fetch_one(pool)
                .await
                .expect("signals");
        let bucket = crate::routes::sessions::bucket_from_signals(
            tempo.as_deref(),
            state.as_deref(),
            activity.as_deref(),
            None,
            &[],
            &std::collections::HashMap::new(),
        );
        (tempo, bucket)
    }

    #[tokio::test]
    async fn a_codex_idle_status_leaves_working_and_keeps_hibernated_and_blocked() {
        let Some(url) = cctui_server_kernel::test_support::db_url("codex_idle_status") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "codex-idle").await;
        let [working, asleep, asking] = [
            seed_owned_session(&pool, uid, mid).await,
            seed_owned_session(&pool, uid, mid).await,
            seed_owned_session(&pool, uid, mid).await,
        ];
        write(&pool, &working, Some("active"), "working").await;
        assert_eq!(bucket_of(&pool, &working).await.1, Bucket::Working);
        let settled = write(&pool, &working, None, "idle").await;
        assert!(
            settled.is_some_and(|w| w.turn_went_idle(&signals(None, Some("idle")))),
            "idle ends the turn"
        );
        assert_eq!(bucket_of(&pool, &working).await, (Some("idle".to_owned()), Bucket::Done));

        write(&pool, &asleep, Some("hibernated"), "idle").await;
        write(&pool, &asleep, None, "idle").await;
        assert_eq!(bucket_of(&pool, &asleep).await.0.as_deref(), Some("hibernated"));

        write(&pool, &asking, Some("blocked"), "working").await;
        write(&pool, &asking, None, "idle").await;
        assert_eq!(bucket_of(&pool, &asking).await, (Some("blocked".to_owned()), Bucket::Blocked));

        drop_machines(&pool, &[working, asleep, asking], &[(uid, mid)]).await;
    }
}
