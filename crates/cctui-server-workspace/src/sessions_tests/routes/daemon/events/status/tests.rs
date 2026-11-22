use uuid::Uuid;

use super::*;

/// Fixture for the agent-title precedence tests: an isolated user, machine
/// and session per case, driving the real `write_status_signals` path.
struct TitleFixture {
    pool: sqlx::PgPool,
    user: Uuid,
    machine: Uuid,
}

impl TitleFixture {
    async fn new(test_name: &str) -> Option<Self> {
        let url = cctui_server_kernel::test_support::db_url(test_name)?;
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
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
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
        self.register(&id).await;
        id
    }

    async fn register(&self, id: &str) {
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, \
             machine_uuid, adapter_id) VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
        )
        .bind(id)
        .bind(self.machine.to_string())
        .bind(self.user)
        .bind(self.machine)
        .execute(&self.pool)
        .await
        .expect("seed session");
    }

    /// One Status event carrying an agent-generated title.
    async fn agent_title(&self, id: &str, name: &str) {
        let signals = StatusSignals {
            tempo: None,
            agent_state: None,
            activity: None,
            name: Some(name),
            agent_title: false,
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: &[],
        };
        write_status_signals(&self.pool, id, &signals, None).await.expect("status write");
    }

    /// A rename from before names carried a source: a bare name write.
    async fn legacy_rename(&self, id: &str, name: &str) {
        sqlx::query("UPDATE sessions SET session_name = $2 WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&self.pool)
            .await
            .expect("rename");
    }

    /// A rename from the webui.
    async fn rename(&self, id: &str, name: &str) {
        crate::routes::sessions::store_rename(&self.pool, id, name).await.expect("rename");
    }

    /// A session spawned with `name`, registered and claimed.
    async fn spawned(&self, name: &str) -> String {
        let id = self.session().await;
        cctui_server_core::machine_policy::remember_spawner(&self.pool, &id, self.user, Some(name))
            .await;
        cctui_server_core::machine_policy::claim_spawner(&self.pool, &id, None).await;
        id
    }

    /// A Title the agent generated (`ai-title`).
    async fn ai_title(&self, id: &str, title: &str) {
        self.decorated_ai_title(id, title, None).await;
    }

    async fn decorated_ai_title(&self, id: &str, title: &str, decorated: Option<&str>) {
        write_status_signals(&self.pool, id, &agent_title_signals(title), decorated)
            .await
            .expect("title write");
    }

    /// A Title the user chose (`custom-title`: `--name`, `/rename`).
    async fn custom_title(&self, id: &str, title: &str) {
        pin_user_title(&self.pool, id, title).await.expect("pin");
    }

    async fn emoji_on(&self) {
        sqlx::query(
            "INSERT INTO user_settings (user_id, version, data) \
             VALUES ($1, 1, '{\"sessionEmojiPrefix\": true}'::jsonb)",
        )
        .bind(self.user)
        .execute(&self.pool)
        .await
        .expect("emoji setting");
    }

    async fn agent_title_of(&self, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT metadata->>'agent_title' FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .expect("read agent title")
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
        let _ = sqlx::query("DELETE FROM user_settings WHERE user_id = $1")
            .bind(self.user)
            .execute(&self.pool)
            .await;
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
    fx.legacy_rename(&typed, "name the human typed").await;
    fx.agent_title(&typed, "agent title").await;
    assert_eq!(
        fx.name_of(&typed).await.as_deref(),
        Some("name the human typed"),
        "a user-set name outranks any agent title"
    );

    let renamed = fx.session().await;
    fx.agent_title(&renamed, "third agent title").await;
    fx.legacy_rename(&renamed, "renamed by hand").await;
    fx.agent_title(&renamed, "fourth agent title").await;
    assert_eq!(
        fx.name_of(&renamed).await.as_deref(),
        Some("renamed by hand"),
        "a rename over an agent title makes the name user-owned"
    );

    fx.cleanup().await;
}

/// The spawn name, echoed back as a `custom-title`, outlives every agent
/// title; the agent title is kept beside it.
#[tokio::test]
async fn a_spawn_name_is_pinned_and_the_agent_title_kept_beside_it() {
    let Some(fx) = TitleFixture::new("spawn_name_is_pinned").await else {
        return;
    };
    let id = fx.spawned("SUPER ORCHESTRATOR").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("SUPER ORCHESTRATOR"));

    fx.custom_title(&id, "SUPER ORCHESTRATOR").await;
    fx.agent_title(&id, "SUPER ORCHESTRATOR").await;
    fx.ai_title(&id, "super orchestrator resume").await;
    fx.agent_title(&id, "super orchestrator resume").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("SUPER ORCHESTRATOR"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("super orchestrator resume"));

    fx.ai_title(&id, "a later title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("SUPER ORCHESTRATOR"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("a later title"));

    fx.cleanup().await;
}

/// A claim that runs before the row exists leaves the intent for the
/// claim at registration, so the spawn name still lands.
#[tokio::test]
async fn a_spawn_name_survives_a_claim_before_registration() {
    let Some(fx) = TitleFixture::new("spawn_name_early_claim").await else {
        return;
    };
    let id = Uuid::new_v4().to_string();
    cctui_server_core::machine_policy::remember_spawner(&fx.pool, &id, fx.user, Some("qa-voice"))
        .await;
    cctui_server_core::machine_policy::claim_spawner(&fx.pool, &id, None).await;
    fx.register(&id).await;
    cctui_server_core::machine_policy::claim_spawner(&fx.pool, &id, None).await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("qa-voice"));

    fx.cleanup().await;
}

/// A named interactive spawn: claim, idle Status events, the `--name`
/// echo, a duplicate claim. The row keeps the name throughout.
#[tokio::test]
async fn a_named_spawn_keeps_its_row_name_through_registration() {
    let Some(fx) = TitleFixture::new("named_spawn_row_name").await else {
        return;
    };
    let id = fx.spawned("qa-voice").await;
    let idle = StatusSignals {
        tempo: Some("idle"),
        agent_state: None,
        activity: None,
        name: None,
        agent_title: false,
        intent: None,
        model: Some("claude-opus"),
        effort: None,
        permission_mode: None,
        children: &[],
    };
    write_status_signals(&fx.pool, &id, &idle, None).await.expect("status write");
    fx.custom_title(&id, "qa-voice").await;
    cctui_server_core::machine_policy::claim_spawner(&fx.pool, &id, None).await;
    write_status_signals(&fx.pool, &id, &idle, None).await.expect("status write");

    let row: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT session_name, metadata->>'name_source', metadata->>'custom_title' \
         FROM sessions WHERE id = $1",
    )
    .bind(&id)
    .fetch_one(&fx.pool)
    .await
    .expect("read row");
    assert_eq!(
        row,
        (Some("qa-voice".to_owned()), Some("user".to_owned()), Some("qa-voice".to_owned()))
    );

    fx.cleanup().await;
}

#[tokio::test]
async fn a_pinned_session_without_a_row_name_gets_its_custom_title_back() {
    async fn lose_name(pool: &sqlx::PgPool, id: &str) {
        sqlx::query("UPDATE sessions SET session_name = NULL WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("drop name");
    }

    let Some(fx) = TitleFixture::new("pinned_nameless_heals").await else {
        return;
    };
    let id = fx.spawned("qa-voice").await;
    fx.custom_title(&id, "qa-voice").await;
    let idle = StatusSignals {
        tempo: Some("idle"),
        agent_state: None,
        activity: None,
        name: None,
        agent_title: false,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: &[],
    };

    lose_name(&fx.pool, &id).await;
    write_status_signals(&fx.pool, &id, &idle, None).await.expect("status write");
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("qa-voice"), "a Status heals");

    lose_name(&fx.pool, &id).await;
    fx.custom_title(&id, "qa-voice").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("qa-voice"), "a replayed title heals");

    lose_name(&fx.pool, &id).await;
    fx.ai_title(&id, "agent title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("qa-voice"), "an agent title");
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("agent title"));

    fx.ai_title(&id, "another agent title").await;
    write_status_signals(&fx.pool, &id, &idle, None).await.expect("status write");
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("qa-voice"));

    fx.cleanup().await;
}

/// `/rename` mid-session lands as a `custom-title` and pins.
#[tokio::test]
async fn a_rename_in_claude_pins_the_name() {
    let Some(fx) = TitleFixture::new("custom_title_pins").await else {
        return;
    };
    let id = fx.session().await;
    fx.ai_title(&id, "first title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("first title"));

    fx.custom_title(&id, "renamed in claude").await;
    fx.agent_title(&id, "renamed in claude").await;
    fx.ai_title(&id, "second title").await;
    fx.agent_title(&id, "second title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("renamed in claude"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("second title"));

    fx.cleanup().await;
}

/// A webui rename pins, survives Claude echoing it back, and an old
/// `custom-title` reported again does not undo it.
#[tokio::test]
async fn a_webui_rename_pins_the_name() {
    let Some(fx) = TitleFixture::new("webui_rename_pins").await else {
        return;
    };
    let id = fx.spawned("spawn name").await;
    fx.custom_title(&id, "spawn name").await;
    fx.ai_title(&id, "first title").await;

    fx.rename(&id, "by hand").await;
    fx.custom_title(&id, "spawn name").await;
    fx.agent_title(&id, "spawn name").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("by hand"), "a stale echo");

    fx.custom_title(&id, "by hand").await;
    fx.agent_title(&id, "by hand").await;
    fx.ai_title(&id, "second title").await;
    fx.agent_title(&id, "second title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("by hand"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("second title"));

    let fresh = fx.session().await;
    fx.rename(&fresh, "named first").await;
    fx.ai_title(&fresh, "named first").await;
    fx.ai_title(&fresh, "then titled").await;
    assert_eq!(fx.name_of(&fresh).await.as_deref(), Some("named first"));

    fx.cleanup().await;
}

/// With no user-set name the agent title still becomes the name.
#[tokio::test]
async fn an_unnamed_session_takes_the_agent_title() {
    let Some(fx) = TitleFixture::new("unnamed_takes_agent_title").await else {
        return;
    };
    let id = fx.session().await;
    fx.ai_title(&id, "first title").await;
    fx.agent_title(&id, "first title").await;
    fx.ai_title(&id, "second title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("second title"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("second title"));

    fx.cleanup().await;
}

/// A name renamed before names carried a source keeps today's protection,
/// and the first agent title after it is kept beside it.
#[tokio::test]
async fn a_legacy_user_name_keeps_the_agent_title_beside_it() {
    let Some(fx) = TitleFixture::new("legacy_user_name").await else {
        return;
    };
    let id = fx.session().await;
    fx.agent_title(&id, "old title").await;
    fx.legacy_rename(&id, "typed").await;
    fx.ai_title(&id, "typed").await;
    fx.ai_title(&id, "new title").await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("typed"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("new title"));

    fx.cleanup().await;
}

/// The emoji prefix still decorates agent-owned names; a pinned name is
/// left as typed and its agent title kept undecorated.
#[tokio::test]
async fn the_emoji_prefix_decorates_only_agent_owned_names() {
    let Some(fx) = TitleFixture::new("emoji_prefix_agent_owned").await else {
        return;
    };
    fx.emoji_on().await;
    let id = fx.session().await;
    let first = crate::session_emoji::decorate("fix the parser").expect("decorates");
    fx.decorated_ai_title(&id, "fix the parser", Some(&first)).await;
    assert_eq!(fx.name_of(&id).await, Some(first.clone()));
    fx.decorated_ai_title(&id, "fix the parser", Some(&first)).await;
    assert_eq!(fx.name_of(&id).await, Some(first), "the prefix is not re-applied");

    let next = crate::session_emoji::decorate("ship the release").expect("decorates");
    fx.decorated_ai_title(&id, "ship the release", Some(&next)).await;
    assert_eq!(fx.name_of(&id).await, Some(next));

    fx.rename(&id, "mine").await;
    let third = crate::session_emoji::decorate("third title").expect("decorates");
    fx.decorated_ai_title(&id, "third title", Some(&third)).await;
    assert_eq!(fx.name_of(&id).await.as_deref(), Some("mine"));
    assert_eq!(fx.agent_title_of(&id).await.as_deref(), Some("third title"));

    fx.cleanup().await;
}
