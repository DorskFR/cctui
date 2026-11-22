use super::*;
use serde_json::json;

fn cap(adapters: &[&str], max_budget: Option<f64>, max_children: Option<u32>) -> SpawnCapability {
    SpawnCapability {
        adapters: adapters.iter().map(|s| (*s).to_owned()).collect(),
        max_budget_usd: max_budget,
        max_children,
        ..SpawnCapability::default()
    }
}

fn usage(live_children: u32) -> Usage {
    Usage { live_children, ..Usage::default() }
}

fn req(adapter: &str, budget: Option<f64>) -> SpawnChildRequest {
    SpawnChildRequest {
        adapter: adapter.to_owned(),
        prompt: "review this".into(),
        budget_usd: budget,
        ..SpawnChildRequest::default()
    }
}

async fn test_pool(test: &str) -> Option<sqlx::PgPool> {
    let url = cctui_server_kernel::test_support::db_url(test)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn bind_parent(pool: &sqlx::PgPool, provider: Uuid) -> String {
    let parent_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
    )
    .bind(format!("th-{parent_id}"))
    .bind(&parent_id)
    .bind(provider)
    .execute(pool)
    .await
    .expect("bind parent");
    parent_id
}

async fn account_name(pool: &sqlx::PgPool, account: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM accounts WHERE id = $1")
        .bind(account)
        .fetch_one(pool)
        .await
        .expect("account name")
}

async fn child_token(pool: &sqlx::PgPool, child_key: &str) -> Option<Uuid> {
    sqlx::query_scalar("SELECT account_id FROM session_tokens WHERE session_id = $1")
        .bind(child_key)
        .fetch_optional(pool)
        .await
        .expect("query child token")
}

/// DB-gated: a child never borrows a sibling account for a family its
/// parent's account lacks, even one the same user owns.
#[tokio::test]
async fn a_child_whose_family_the_parents_account_lacks_is_refused() {
    let Some(pool) = test_pool("a_child_whose_family_the_parents_account_lacks_is_refused").await
    else {
        return;
    };
    let uid = Uuid::new_v4();
    let seeded =
        seed_accounts(&pool, uid, "refuse", &[("personal", "anthropic"), ("work", "openai")]).await;
    let (parent_account, anthropic_provider) = seeded[0];
    let parent_name = account_name(&pool, parent_account).await;
    let parent_id = bind_parent(&pool, anthropic_provider).await;

    let state = crate::state::AppState::for_test(pool.clone());
    let parent = parent_of(&parent_id, uid);

    let child_key = Uuid::new_v4().to_string();
    let refused = child_account_env(
        &state,
        &parent,
        "codex",
        cctui_server_kernel::accounts_types::Family::Openai,
        Some("gpt-5.5"),
        &child_key,
    )
    .await
    .expect_err("a codex child must not borrow the sibling openai account");
    assert!(refused.status().is_client_error(), "{refused}");
    assert_eq!(
        refused.message(),
        format!(
            "could not provision the openai child: the parent's account `{parent_name}` has \
             no openai credential, and the parent is not bound to an account pool"
        )
    );
    assert_eq!(child_token(&pool, &child_key).await, None, "a token was minted anyway");

    let same_family_key = Uuid::new_v4().to_string();
    let (env, _) = child_account_env(
        &state,
        &parent,
        "claude-code",
        cctui_server_kernel::accounts_types::Family::Anthropic,
        None,
        &same_family_key,
    )
    .await
    .expect("claude child");
    assert!(!env.is_empty());
    assert_eq!(child_token(&pool, &same_family_key).await, Some(anthropic_provider));
    let recorded: String = sqlx::query_scalar(
        "SELECT a.name FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE st.session_id = $1",
    )
    .bind(&same_family_key)
    .fetch_one(&pool)
    .await
    .expect("child account name");
    assert_eq!(recorded, parent_name, "the child session does not record its account");

    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// DB-gated: a requested model the account's catalog does not list is
/// refused, never swapped for the catalog's first entry.
#[tokio::test]
async fn spawn_child_refuses_a_model_the_account_cannot_serve() {
    let Some(pool) = test_pool("spawn_child_refuses_a_model_the_account_cannot_serve").await else {
        return;
    };
    let uid = Uuid::new_v4();
    let seeded = seed_accounts(&pool, uid, "model", &[("personal", "anthropic")]).await;
    let (parent_account, anthropic_provider) = seeded[0];
    sqlx::query(
        "INSERT INTO account_providers \
           (id, user_id, account_id, provider, auth_scheme, encrypted_access_token, models) \
         VALUES ($1, $2, $3, 'fireworks', 'api_key', 'x', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(parent_account)
    .bind(json!([{ "model": "accounts/fireworks/models/kimi-k3", "label": "kimi-k3" }]))
    .execute(&pool)
    .await
    .expect("seed fireworks provider");
    let parent_id = bind_parent(&pool, anthropic_provider).await;

    let state = crate::state::AppState::for_test(pool.clone());
    let parent = parent_of(&parent_id, uid);
    let child_key = Uuid::new_v4().to_string();
    let refused = child_account_env(
        &state,
        &parent,
        "opencode",
        cctui_server_kernel::accounts_types::Family::Fireworks,
        Some("gpt-5.5"),
        &child_key,
    )
    .await
    .expect_err("gpt-5.5 is not in the fireworks catalog");
    assert!(refused.status().is_client_error(), "{refused}");
    assert!(refused.message().contains("cannot serve model \"gpt-5.5\""), "{refused}");
    assert_eq!(child_token(&pool, &child_key).await, None, "a token was minted anyway");

    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// DB-gated: `CctuiUsage` reports, per adapter, the account a child would
/// land on or why its spawn would be refused.
#[tokio::test]
async fn spawn_child_landings_name_the_account_or_the_refusal() {
    let Some(pool) = test_pool("spawn_child_landings_name_the_account_or_the_refusal").await else {
        return;
    };
    let uid = Uuid::new_v4();
    let seeded =
        seed_accounts(&pool, uid, "landing", &[("personal", "anthropic"), ("work", "openai")])
            .await;
    let (parent_account, anthropic_provider) = seeded[0];
    let parent_name = account_name(&pool, parent_account).await;
    let parent_id = bind_parent(&pool, anthropic_provider).await;
    let state = crate::state::AppState::for_test(pool.clone());

    let adapters = ["claude-code".to_owned(), "codex".to_owned(), "kilo".to_owned()];
    let landings =
        cctui_server_core::session_limits::child_landings(&state, &parent_id, uid, &adapters).await;

    let claude = &landings["claude-code"];
    assert_eq!(claude.account.as_deref(), Some(parent_name.as_str()));
    assert_eq!(claude.refused, None);
    let codex = &landings["codex"];
    assert_eq!(codex.account, None, "codex must not land on the sibling account");
    assert!(
        codex.refused.as_deref().is_some_and(|r| r.contains("has no openai credential")),
        "{codex:?}"
    );
    assert!(landings["kilo"].refused.is_some());
    let codex_json = serde_json::to_value(codex).expect("serialize");
    assert!(codex_json.get("refused").is_some() && codex_json.get("account").is_none());

    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// Seeds a user plus one account per `(name, provider)`, returning the
/// account and provider ids in order.
#[cfg(test)]
async fn seed_accounts(
    pool: &sqlx::PgPool,
    uid: Uuid,
    label: &str,
    accounts: &[(&str, &str)],
) -> Vec<(Uuid, Uuid)> {
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{label}-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    let mut out = Vec::new();
    for (name, provider) in accounts {
        let account = Uuid::new_v4();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(account)
            .bind(uid)
            .bind(format!("{name}-{account}"))
            .execute(pool)
            .await
            .expect("seed account");
        let provider_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO account_providers \
               (id, user_id, account_id, provider, auth_scheme, encrypted_access_token) \
             VALUES ($1, $2, $3, $4, 'api_key', 'x')",
        )
        .bind(provider_id)
        .bind(uid)
        .bind(account)
        .bind(provider)
        .execute(pool)
        .await
        .expect("seed provider");
        out.push((account, provider_id));
    }
    out
}

/// A parent session bound to `uid`'s own claude account, with a colleague's
/// codex account shared to `uid`. Returns the parent's session id.
async fn seed_parent_with_shared_colleague_account(
    pool: &sqlx::PgPool,
    uid: Uuid,
    colleague: Uuid,
) -> String {
    let mine = seed_accounts(pool, uid, "owner", &[("parent-claude", "anthropic")]).await;
    let theirs = seed_accounts(pool, colleague, "colleague", &[("their-codex", "openai")]).await;
    sqlx::query(
        "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
         VALUES ('account', $1, $2)",
    )
    .bind(theirs[0].0)
    .bind(uid)
    .execute(pool)
    .await
    .expect("share the colleague's account with the parent's owner");
    let parent_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
    )
    .bind(format!("th-{parent_id}"))
    .bind(&parent_id)
    .bind(mine[0].1)
    .execute(pool)
    .await
    .expect("bind parent");
    parent_id
}

/// DB-gated: a child never spends a colleague's account shared with the
/// parent's owner.
#[tokio::test]
async fn a_child_never_binds_an_account_only_shared_with_the_owner() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "a_child_never_binds_an_account_only_shared_with_the_owner",
    ) else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");

    let uid = Uuid::new_v4();
    let colleague = Uuid::new_v4();
    let parent_id = seed_parent_with_shared_colleague_account(&pool, uid, colleague).await;

    let state = crate::state::AppState::for_test(pool.clone());
    let parent = Parent {
        session_id: parent_id.clone(),
        machine_uuid: Uuid::new_v4(),
        working_dir: None,
        user_id: uid,
        permission_mode: None,
    };

    let child_key = Uuid::new_v4().to_string();
    let refused = child_account_env(
        &state,
        &parent,
        "codex",
        cctui_server_kernel::accounts_types::Family::Openai,
        None,
        &child_key,
    )
    .await;
    assert!(refused.is_err(), "the child was provisioned off a colleague's account");
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT account_id FROM session_tokens WHERE session_id = $1")
            .bind(&child_key)
            .fetch_optional(&pool)
            .await
            .expect("query child token");
    assert_eq!(bound, None, "a token was minted on someone else's credential");

    for u in [uid, colleague] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
    }
}

/// DB-gated R3: a pool is an explicit statement of what the tree may spend,
/// so a child elected for another family stays inside it and is stamped
/// with it.
#[tokio::test]
async fn an_elected_child_stays_inside_the_parents_pool() {
    let Some(url) =
        cctui_server_kernel::test_support::db_url("an_elected_child_stays_inside_the_parents_pool")
    else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");

    let uid = Uuid::new_v4();
    let seeded = seed_accounts(
        &pool,
        uid,
        "pooled",
        &[("parent-claude", "anthropic"), ("in-pool-codex", "openai"), ("outside-codex", "openai")],
    )
    .await;
    let (parent_account, anthropic_provider) = seeded[0];
    let (in_pool_account, in_pool_provider) = seeded[1];

    let pool_id = Uuid::new_v4();
    sqlx::query("INSERT INTO account_pools (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(pool_id)
        .bind(uid)
        .bind(format!("team-{pool_id}"))
        .execute(&pool)
        .await
        .expect("seed pool");
    for account in [parent_account, in_pool_account] {
        sqlx::query("INSERT INTO account_pool_members (pool_id, account_id) VALUES ($1, $2)")
            .bind(pool_id)
            .bind(account)
            .execute(&pool)
            .await
            .expect("seed pool member");
    }

    let parent_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id, pool_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(format!("th-{parent_id}"))
    .bind(&parent_id)
    .bind(anthropic_provider)
    .bind(pool_id)
    .execute(&pool)
    .await
    .expect("bind parent to its pool");

    let state = crate::state::AppState::for_test(pool.clone());
    let parent = Parent {
        session_id: parent_id.clone(),
        machine_uuid: Uuid::new_v4(),
        working_dir: None,
        user_id: uid,
        permission_mode: None,
    };

    let child_key = Uuid::new_v4().to_string();
    child_account_env(
        &state,
        &parent,
        "codex",
        cctui_server_kernel::accounts_types::Family::Openai,
        None,
        &child_key,
    )
    .await
    .expect("a codex child from the pool");

    let (bound, stamped): (Uuid, Option<Uuid>) =
        sqlx::query_as("SELECT account_id, pool_id FROM session_tokens WHERE session_id = $1")
            .bind(&child_key)
            .fetch_one(&pool)
            .await
            .expect("child token");
    assert_eq!(bound, in_pool_provider, "the child left its parent's pool");
    assert_eq!(stamped, Some(pool_id), "the child was not stamped with the pool");

    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// DB-gated: the follow-up target must be the caller's own child — anything
/// else refuses, or any session could inject prompts into any other. Its
/// machine is free: cross-machine delivery is the bus's job.
#[tokio::test]
async fn message_child_only_reaches_own_children() {
    let Some(url) =
        cctui_server_kernel::test_support::db_url("message_child_only_reaches_own_children")
    else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");

    let uid = Uuid::new_v4();
    let stranger = Uuid::new_v4();
    let machine = Uuid::new_v4();
    let other_machine = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'mc-test', $2)")
        .bind(uid)
        .bind(format!("kh-{uid}"))
        .execute(&pool)
        .await
        .expect("seed user");
    for m in [machine, other_machine] {
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(m)
            .bind(uid)
            .bind(m.to_string())
            .bind(format!("kh-{m}"))
            .execute(&pool)
            .await
            .expect("seed machine");
    }
    let parent_id = Uuid::new_v4().to_string();
    let child_id = Uuid::new_v4().to_string();
    let elsewhere_id = Uuid::new_v4().to_string();
    let orphan_id = Uuid::new_v4().to_string();
    for (id, parent, m) in [
        (&parent_id, None::<&str>, machine),
        (&child_id, Some(parent_id.as_str()), machine),
        (&elsewhere_id, Some(parent_id.as_str()), other_machine),
        (&orphan_id, None, machine),
    ] {
        sqlx::query(
            "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
             machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code')",
        )
        .bind(id)
        .bind(parent)
        .bind(m.to_string())
        .bind(uid)
        .bind(m)
        .execute(&pool)
        .await
        .expect("seed session");
    }
    let parent = Parent {
        session_id: parent_id.clone(),
        machine_uuid: machine,
        working_dir: None,
        user_id: uid,
        permission_mode: None,
    };

    assert_eq!(resolve_child_adapter(&pool, uid, &parent, &child_id).await.unwrap(), "claude-code");
    assert_eq!(
        resolve_child_adapter(&pool, uid, &parent, &orphan_id).await.unwrap_err().status(),
        StatusCode::FORBIDDEN,
        "a non-child of the caller must refuse"
    );
    assert_eq!(
        resolve_child_adapter(&pool, uid, &parent, &elsewhere_id).await.unwrap(),
        "claude-code",
        "a child on another machine is reachable: the bus routes to ITS machine"
    );
    assert_eq!(
        resolve_child_adapter(&pool, stranger, &parent, &child_id).await.unwrap_err().status(),
        StatusCode::NOT_FOUND,
        "another user's lookup must not even see the session"
    );
    assert_eq!(
        resolve_child_adapter(&pool, uid, &parent, "no-such-session").await.unwrap_err().status(),
        StatusCode::NOT_FOUND,
    );

    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// The tree the `archive-child` tests share: `root` with children `a`
/// (itself parent of `a1`), `b`, a pinned child and another user's row, plus
/// an `outsider` outside the tree. `root` may hold 4 children.
struct ArchiveTree {
    pool: sqlx::PgPool,
    state: AppState,
    uid: Uuid,
    stranger: Uuid,
    root_cap: SpawnCapability,
    root: String,
    a: String,
    a1: String,
    b: String,
    pinned: String,
    foreign: String,
    outsider: String,
}

impl ArchiveTree {
    async fn seed(test: &str) -> Option<Self> {
        let url = cctui_server_kernel::test_support::db_url(test)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, stranger, machine) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for (u, name) in [(uid, "ac-test"), (stranger, "ac-stranger")] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(u)
                .bind(name)
                .bind(format!("kh-{u}"))
                .execute(&pool)
                .await
                .expect("seed user");
        }
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        let id = || Uuid::new_v4().to_string();
        let (root, a, a1, b, pinned, foreign, outsider) =
            (id(), id(), id(), id(), id(), id(), id());
        for (sid, parent, owner) in [
            (&root, None::<&str>, uid),
            (&a, Some(root.as_str()), uid),
            (&a1, Some(a.as_str()), uid),
            (&b, Some(root.as_str()), uid),
            (&pinned, Some(root.as_str()), uid),
            (&foreign, Some(root.as_str()), stranger),
            (&outsider, None, uid),
        ] {
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code')",
            )
            .bind(sid)
            .bind(parent)
            .bind(machine.to_string())
            .bind(owner)
            .bind(machine)
            .execute(&pool)
            .await
            .expect("seed session");
        }
        sqlx::query("UPDATE sessions SET pinned = true WHERE id = $1")
            .bind(&pinned)
            .execute(&pool)
            .await
            .expect("pin");
        let root_cap = cap(&["claude-code"], None, Some(4));
        crate::store::spawn_capabilities::upsert(&pool, &root, &root_cap)
            .await
            .expect("seed root capability");
        let state = AppState::for_test(pool.clone());
        Some(Self {
            pool,
            state,
            uid,
            stranger,
            root_cap,
            root,
            a,
            a1,
            b,
            pinned,
            foreign,
            outsider,
        })
    }

    async fn status(&self, sid: &str) -> String {
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(sid)
            .fetch_one(&self.pool)
            .await
            .expect("status")
    }

    async fn archive(
        &self,
        caller: Uuid,
        from: &str,
        to: &str,
    ) -> Result<ArchiveChildResponse, AppError> {
        archive_descendant(&self.state, caller, from, to).await
    }

    async fn cleanup(self) {
        let users = vec![self.uid, self.stranger];
        sqlx::query("DELETE FROM sessions WHERE user_id = ANY($1)")
            .bind(&users)
            .execute(&self.pool)
            .await
            .ok();
        crate::store::spawn_capabilities::delete(&self.pool, &self.root).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1")
            .bind(self.uid)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(&users)
            .execute(&self.pool)
            .await
            .ok();
    }
}

/// DB-gated: `CctuiAgentArchive` reaches only the caller's own descendants;
/// anything else is a 404 and stays untouched.
#[tokio::test]
async fn archive_child_is_a_404_outside_the_callers_tree() {
    let Some(t) = ArchiveTree::seed("archive_child_is_a_404_outside_the_callers_tree").await else {
        return;
    };
    for (caller, from, to, why) in [
        (t.uid, &t.a, &t.b, "a sibling"),
        (t.uid, &t.a, &t.root, "its parent"),
        (t.uid, &t.a, &t.a, "itself"),
        (t.uid, &t.root, &t.outsider, "a session outside the tree"),
        (t.uid, &t.root, &t.foreign, "another user's session"),
        (t.stranger, &t.root, &t.a, "a stranger's caller id"),
    ] {
        let status = t.archive(caller, from, to).await.err().map(|e| e.status());
        assert_eq!(status, Some(StatusCode::NOT_FOUND), "archiving {why} must be a 404");
        assert_ne!(t.status(to).await, "archived", "{why} was archived anyway");
    }
    t.cleanup().await;
}

/// DB-gated: a child's completion reaches only its own parent, owned by the
/// same user, and only while that parent can take a turn.
#[tokio::test]
async fn a_child_completion_is_only_offered_to_its_own_live_parent() {
    let Some(t) =
        ArchiveTree::seed("a_child_completion_is_only_offered_to_its_own_live_parent").await
    else {
        return;
    };
    let code = |r: Result<String, AppError>| r.err().map(|e| e.status());
    assert_eq!(
        code(deliver_completion(&t.state, t.stranger, &t.a, "done").await),
        Some(StatusCode::NOT_FOUND),
        "another user's machine key cannot post a completion"
    );
    assert_eq!(
        code(deliver_completion(&t.state, t.uid, &t.foreign, "done").await),
        Some(StatusCode::NOT_FOUND),
        "a child owned by someone else is unknown"
    );
    assert_eq!(
        code(deliver_completion(&t.state, t.uid, &t.outsider, "done").await),
        Some(StatusCode::CONFLICT),
        "a root session has no parent to report to"
    );
    assert_eq!(
        code(deliver_completion(&t.state, t.uid, &t.a, "   ").await),
        Some(StatusCode::BAD_REQUEST)
    );
    let huge = "x".repeat(MAX_COMPLETION_BYTES + 1);
    assert_eq!(
        code(deliver_completion(&t.state, t.uid, &t.a, &huge).await),
        Some(StatusCode::PAYLOAD_TOO_LARGE)
    );
    let reached = deliver_completion(&t.state, t.uid, &t.a1, "the grandchild is done").await;
    assert!(
        matches!(&reached, Ok(parent) if *parent == t.a)
            || code(reached) == Some(StatusCode::SERVICE_UNAVAILABLE),
        "an authorized completion must go to the child's own parent (or fail only on \
         delivery, with no daemon connected in the test)"
    );
    sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
        .bind(&t.root)
        .execute(&t.pool)
        .await
        .expect("archive root");
    assert_eq!(
        code(deliver_completion(&t.state, t.uid, &t.a, "done").await),
        Some(StatusCode::CONFLICT),
        "an archived parent is never offered the turn"
    );
    t.cleanup().await;
}

/// DB-gated: a pin is the human's mark; an agent never overrides it.
#[tokio::test]
async fn archive_child_refuses_a_pinned_child() {
    let Some(t) = ArchiveTree::seed("archive_child_refuses_a_pinned_child").await else {
        return;
    };
    let pin = t.archive(t.uid, &t.root, &t.pinned).await;
    assert_eq!(pin.err().map(|e| e.status()), Some(StatusCode::CONFLICT));
    assert_ne!(t.status(&t.pinned).await, "archived", "a pin is never overridden");
    t.cleanup().await;
}

/// DB-gated: a grandparent may release a grandchild, and archiving a child
/// cascades to its own children and reports every archived row.
#[tokio::test]
async fn archive_child_cascades_and_reaches_grandchildren() {
    let Some(t) = ArchiveTree::seed("archive_child_cascades_and_reaches_grandchildren").await
    else {
        return;
    };
    let grand = t.archive(t.uid, &t.root, &t.a1).await.expect("grandchild");
    assert_eq!(grand.archived, vec![t.a1.clone()], "a grandparent may release a grandchild");

    let released = t.archive(t.uid, &t.root, &t.a).await.expect("direct child");
    let mut expected = vec![t.a.clone(), t.a1.clone()];
    expected.sort();
    assert_eq!(released.archived, expected, "the cascade reports every archived row");
    assert_eq!(t.status(&t.a).await, "archived");
    t.cleanup().await;
}

/// DB-gated: archiving hands the slot back, so a spawn refused at the cap
/// goes through, and `CctuiUsage` reports the same count.
#[tokio::test]
async fn archive_child_frees_a_slot_for_a_refused_spawn() {
    let Some(t) = ArchiveTree::seed("archive_child_frees_a_slot_for_a_refused_spawn").await else {
        return;
    };
    assert_eq!(
        slots_used(&AppState::for_test(t.pool.clone()), &t.root).await,
        4,
        "a, b, pinned and foreign count"
    );
    assert_eq!(
        authorize(Some(&t.root_cap), &req("claude-code", None), &usage(4)),
        Err(Denied::TooManyChildren { max: 4 }),
    );

    let released = t.archive(t.uid, &t.root, &t.a).await.expect("direct child");
    assert_eq!(released.children_used, 3);
    assert_eq!(released.max_children, Some(4));
    assert!(
        authorize(Some(&t.root_cap), &req("claude-code", None), &usage(released.children_used))
            .is_ok(),
        "the freed slot lets the refused spawn through"
    );
    assert_eq!(
        child_slots(&t.state, &t.root).await,
        Some(ChildSlots { used: 3, max: Some(4), depth_left: None }),
        "CctuiUsage reports the same count"
    );
    t.cleanup().await;
}

/// DB-gated: reservations across a two-level tree are summed from the
/// ledger under the root, and a released reservation frees its share.
#[tokio::test]
async fn reservations_sum_across_the_tree_under_the_root_ceiling() {
    let Some(url) = cctui_server_kernel::test_support::db_url("spawn_tree_reservations") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let root_id = Uuid::new_v4().to_string();
    let root =
        SpawnCapability { max_tree_budget_usd: Some(5.0), ..cap(&["codex"], Some(2.0), None) };
    let (a, child_cap) = reserve_child(
        &pool,
        &root_id,
        Some(&root),
        &req("codex", None),
        usage(0),
        &Uuid::new_v4().to_string(),
    )
    .await
    .expect("first child");
    assert_eq!(a.budget_usd, Some(2.0));
    assert_eq!(child_cap.tree_root.as_deref(), Some(root_id.as_str()));

    let child_id = Uuid::new_v4().to_string();
    let grandchild_key = Uuid::new_v4().to_string();
    let (a, _) = reserve_child(
        &pool,
        &child_id,
        Some(&child_cap),
        &req("codex", None),
        usage(0),
        &grandchild_key,
    )
    .await
    .expect("grandchild");
    assert_eq!(a.budget_usd, Some(2.0));

    let (a, _) = reserve_child(
        &pool,
        &root_id,
        Some(&root),
        &req("codex", None),
        usage(1),
        &Uuid::new_v4().to_string(),
    )
    .await
    .expect("clamped to the remainder");
    assert_eq!(a.budget_usd, Some(1.0));
    assert!((tree_granted_usd(&pool, &root_id).await.unwrap() - 5.0).abs() < 1e-9);

    let err = reserve_child(
        &pool,
        &child_id,
        Some(&child_cap),
        &req("codex", None),
        usage(1),
        &Uuid::new_v4().to_string(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN, "the tree is spent");

    release_child(&pool, &grandchild_key).await;
    assert!((tree_granted_usd(&pool, &root_id).await.unwrap() - 3.0).abs() < 1e-9);

    sqlx::query("DELETE FROM session_spawn_capabilities WHERE capability->>'tree_root' = $1")
        .bind(&root_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spawn_tree_grants WHERE root_id = $1")
        .bind(&root_id)
        .execute(&pool)
        .await
        .ok();
}

/// DB-gated: a reserved child that has not registered yet still occupies
/// its slot, so back-to-back spawns cannot overshoot `max_children`.
#[tokio::test]
async fn unregistered_reservations_count_against_max_children() {
    let Some(url) = cctui_server_kernel::test_support::db_url("spawn_child_pending_slots") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let parent_id = Uuid::new_v4().to_string();
    let parent = cap(&["codex"], Some(1.0), Some(2));
    for _ in 0..2 {
        let key = Uuid::new_v4().to_string();
        reserve_child(&pool, &parent_id, Some(&parent), &req("codex", None), usage(0), &key)
            .await
            .expect("within the child cap");
    }
    let key = Uuid::new_v4().to_string();
    let err = reserve_child(&pool, &parent_id, Some(&parent), &req("codex", None), usage(0), &key)
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
    assert!(err.message().contains("maximum of 2"), "{}", err.message());

    sqlx::query("DELETE FROM session_spawn_capabilities WHERE capability->>'tree_root' = $1")
        .bind(&parent_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM spawn_tree_grants WHERE root_id = $1")
        .bind(&parent_id)
        .execute(&pool)
        .await
        .ok();
}

fn parent_of(session_id: &str, owner: Uuid) -> Parent {
    Parent {
        session_id: session_id.to_owned(),
        machine_uuid: Uuid::new_v4(),
        working_dir: None,
        user_id: owner,
        permission_mode: None,
    }
}

/// DB-gated: a spawn grantee's session belongs to the machine owner but runs
/// on the grantee's account, and so does its tree.
#[tokio::test]
async fn a_grantee_trees_child_runs_on_the_grantees_account() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "a_grantee_trees_child_runs_on_the_grantees_account",
    ) else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let owner = Uuid::new_v4();
    let grantee = Uuid::new_v4();
    seed_accounts(&pool, owner, "machine-owner", &[]).await;
    let mine = seed_accounts(&pool, grantee, "grantee", &[("g-claude", "anthropic")]).await;
    let (account, provider) = mine[0];
    let state = crate::state::AppState::for_test(pool.clone());
    let parent_id = Uuid::new_v4().to_string();
    crate::routes::gateway::mint_session_env(
        &state,
        grantee,
        &format!("g-claude-{account}"),
        cctui_server_kernel::accounts_types::Family::Anthropic,
        &parent_id,
    )
    .await
    .expect("bind the parent to the grantee's account");

    let child_key = Uuid::new_v4().to_string();
    child_account_env(
        &state,
        &parent_of(&parent_id, owner),
        "claude-code",
        cctui_server_kernel::accounts_types::Family::Anthropic,
        None,
        &child_key,
    )
    .await
    .expect("the child mints on the account its parent runs on");
    let bound: (Uuid, Option<Uuid>) =
        sqlx::query_as("SELECT account_id, user_id FROM session_tokens WHERE session_id = $1")
            .bind(&child_key)
            .fetch_one(&pool)
            .await
            .expect("child token");
    assert_eq!(bound, (provider, Some(grantee)));

    for u in [owner, grantee] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
    }
}

/// DB-gated: a child is attributed to whoever spawned its parent, else to
/// the parent's owner.
#[tokio::test]
async fn a_child_is_attributed_to_the_human_behind_its_tree() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "a_child_is_attributed_to_the_human_behind_its_tree",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = Uuid::new_v4();
    let grantee = Uuid::new_v4();
    seed_accounts(&pool, owner, "attr-owner", &[]).await;
    seed_accounts(&pool, grantee, "attr-grantee", &[]).await;
    let mut parents = Vec::new();
    for spawner in [Some(grantee), None] {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, spawned_by) \
             VALUES ($1, 'm', '/w', $2, $3)",
        )
        .bind(&sid)
        .bind(owner)
        .bind(spawner)
        .execute(&pool)
        .await
        .expect("seed parent");
        parents.push(sid);
    }
    assert_eq!(child_spawner(&pool, &parent_of(&parents[0], owner)).await, grantee);
    assert_eq!(child_spawner(&pool, &parent_of(&parents[1], owner)).await, owner);

    for sid in &parents {
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(&pool).await.ok();
    }
    for u in [owner, grantee] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
    }
}

fn policy_rule(command: &str, timeout_secs: Option<u32>) -> cctui_proto::hook_policy::HookRule {
    cctui_proto::hook_policy::HookRule {
        event: cctui_proto::hook_policy::HookEvent::PreToolUse,
        tool: cctui_proto::hook_policy::ToolClass::Shell,
        command: command.into(),
        timeout_secs,
    }
}

#[tokio::test]
async fn a_cctui_agent_child_carries_its_parents_hook_policy() {
    use cctui_proto::adapter::AdapterCommand;
    use cctui_proto::hook_policy::no_direct_harness_rule;
    use cctui_proto::ws::DaemonFrameDown;
    let name = "a_cctui_agent_child_carries_its_parents_hook_policy";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (uid, machine) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'hp-test', $2)")
        .bind(uid)
        .bind(format!("kh-{uid}"))
        .execute(&pool)
        .await
        .expect("seed user");
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine)
        .bind(uid)
        .bind(machine.to_string())
        .bind(format!("kh-{machine}"))
        .execute(&pool)
        .await
        .expect("seed machine");
    let parent = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id) \
         VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
    )
    .bind(&parent)
    .bind(machine.to_string())
    .bind(uid)
    .bind(machine)
    .execute(&pool)
    .await
    .expect("seed parent");
    let deny_rm = policy_rule("deny-rm", Some(30));
    let parent_policy =
        cctui_server_core::hook_policy::carry("claude-code", vec![deny_rm.clone()]).unwrap();
    sqlx::query(
        "INSERT INTO machine_launches (command_id, session_id, machine_id, user_id, \
         acting_user_id, adapter_id, working_dir, hook_policy) \
         VALUES ($1, $2, $3, $4, $4, 'claude-code', '/w', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&parent)
    .bind(machine)
    .bind(uid)
    .bind(sqlx::types::Json(&parent_policy))
    .execute(&pool)
    .await
    .expect("record the parent's launch policy");

    let state = AppState::for_test(pool.clone());
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    state.bus.register_daemon(machine, Uuid::new_v4(), tx);
    state.core.spawn_capabilities.insert(parent.clone(), cap(&["claude-code"], None, Some(5)));

    let audit = policy_rule("audit", None);
    let mut adding = req("claude-code", None);
    adding.hook_rules = vec![audit.clone()];
    let spawned = cctui_server_core::child_exec::spawn_child(&state, uid, &parent, &adding)
        .await
        .expect("child spawned");
    let Some(DaemonFrameDown::Command { command, .. }) = rx.recv().await else {
        panic!("expected a command frame")
    };
    let AdapterCommand::Spawn { spec, .. } = *command else { panic!("expected a spawn") };
    let carried = spec.hook_policy.expect("the child carries a policy").rules;
    assert_eq!(carried, vec![no_direct_harness_rule(), deny_rm.clone(), audit]);
    let recorded =
        cctui_server_core::hook_policy::for_session(&pool, &spawned.session_id).await.unwrap();
    assert_eq!(recorded.rules, carried, "the child's effective policy is persisted");

    let omitting = req("claude-code", None);
    cctui_server_core::child_exec::spawn_child(&state, uid, &parent, &omitting)
        .await
        .expect("a child adding nothing still spawns");
    let Some(DaemonFrameDown::Command { command, .. }) = rx.recv().await else {
        panic!("expected a command frame")
    };
    let AdapterCommand::Spawn { spec, .. } = *command else { panic!("expected a spawn") };
    assert!(spec.hook_policy.expect("policy").rules.contains(&deny_rm));

    let mut weakening = req("claude-code", None);
    weakening.hook_rules = vec![policy_rule("deny-rm", Some(1))];
    let err = cctui_server_core::child_exec::spawn_child(&state, uid, &parent, &weakening)
        .await
        .expect_err("a weakening child is refused");
    assert_eq!(err.into_parts().0, StatusCode::BAD_REQUEST);
    assert!(rx.try_recv().is_err(), "nothing reached the daemon");

    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// A user, a machine and a live parent session on it.
async fn seed_parent_on_machine(pool: &sqlx::PgPool) -> (Uuid, Uuid, String) {
    let (uid, machine) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("ctx-off-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine)
        .bind(uid)
        .bind(machine.to_string())
        .bind(format!("kh-{machine}"))
        .execute(pool)
        .await
        .expect("seed machine");
    let parent = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id) \
         VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
    )
    .bind(&parent)
    .bind(machine.to_string())
    .bind(uid)
    .bind(machine)
    .execute(pool)
    .await
    .expect("seed parent");
    (uid, machine, parent)
}

/// A memory of `uid` pinned by its profile `kit`.
async fn seed_pinned_memory(pool: &sqlx::PgPool, uid: Uuid) -> Uuid {
    use crate::routes::context::{ContextItemSpec, clean_spec, insert};
    let memory = clean_spec(ContextItemSpec {
        kind: "memory".into(),
        name: "kit-note".into(),
        title: "Kit note".into(),
        body: "pinned".into(),
        enabled: true,
        ..ContextItemSpec::default()
    })
    .expect("valid memory");
    let memory = insert(pool, uid, &memory, None, None).await.expect("memory");
    sqlx::query(
        "INSERT INTO session_profiles (user_id, name, context_items) VALUES ($1, 'kit', $2)",
    )
    .bind(uid)
    .bind(vec![memory.id])
    .execute(pool)
    .await
    .expect("seed profile");
    memory.id
}

#[tokio::test]
async fn with_context_off_a_child_spawns_with_only_its_ownership_record() {
    use crate::routes::context::ownership::test_support::assert_no_content;
    use crate::routes::context::ownership::{self};
    use crate::routes::context::{SpawnScope, StoredScope};
    use cctui_proto::features::{Feature, FeatureSet};
    let name = "with_context_off_a_child_spawns_with_only_its_ownership_record";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (uid, machine, parent) = seed_parent_on_machine(&pool).await;
    let parent_scope =
        StoredScope { user_id: uid, coords: SpawnScope::default(), dispatched: true };
    ownership::record(&pool, &parent, &parent_scope).await;
    let memory = seed_pinned_memory(&pool, uid).await;

    let mut state = AppState::for_test(pool.clone());
    state.config.features = FeatureSet::all().without(Feature::Context);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    state.bus.register_daemon(machine, Uuid::new_v4(), tx);
    state.core.spawn_capabilities.insert(parent.clone(), cap(&["claude-code"], None, Some(5)));

    let spawned =
        cctui_server_core::child_exec::spawn_child(&state, uid, &parent, &req("claude-code", None))
            .await
            .expect("a child spawns with context off");
    assert!(rx.recv().await.is_some(), "the spawn reached the daemon");
    let child = spawned.session_id;
    assert_eq!(ownership::owner_of(&pool, &child).await.expect("lookup"), Some(uid));
    assert!(
        ownership::launch_scope(&pool, &child).await.is_some_and(|s| s.dispatched),
        "the child inherits its parent's dispatched flag from the ownership record"
    );
    assert_no_content(&pool, &child).await;

    let parent_of_tree = parent_of(&parent, uid);
    let off_key = Uuid::new_v4().to_string();
    remember_child_context(&state, &parent_of_tree, &off_key, "claude-code", None, Some("kit"))
        .await;
    assert_no_content(&pool, &off_key).await;
    state.config.features = FeatureSet::all();
    let on_key = Uuid::new_v4().to_string();
    remember_child_context(&state, &parent_of_tree, &on_key, "claude-code", None, Some("kit"))
        .await;
    let pinned: Vec<Uuid> =
        sqlx::query_scalar("SELECT item_ids FROM session_context WHERE spawn_key = $1")
            .bind(&on_key)
            .fetch_one(&pool)
            .await
            .expect("the context row");
    assert_eq!(pinned, vec![memory], "with context on the profile's memory is recorded");

    sqlx::query("DELETE FROM session_context WHERE spawn_key = ANY($1)")
        .bind(vec![parent, child, off_key, on_key])
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM session_profiles WHERE user_id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM context_items WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}
