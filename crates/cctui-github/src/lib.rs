//! GitHub integration for cctui — connector, diff proxy, review store, and
//! routes, fully encapsulated behind the server's `github` Cargo feature.
//!
//! Everything GitHub-specific lives in this crate so a build without the feature
//! contains zero GitHub code, routes, or schema. See `docs/github-integration.md`
//! §7 for the plugin design (dedicated `github` Postgres schema, one-directional
//! FKs into core, `DROP SCHEMA github CASCADE` uninstall).
//!
//! GH-PKG-2 lands the `github` schema and a `search_path`-isolated migrator:
//! the crate's embedded migrations (and sqlx's own `_sqlx_migrations`
//! bookkeeping) live entirely inside `github.*`, so they are independent of
//! core's migration history and are removed wholesale by [`uninstall`]'s
//! `DROP SCHEMA github CASCADE`. The real handler bodies land in later GH-*
//! tickets.

use axum::Router;
use axum::routing::{get, post};
use sqlx::{Executor, PgPool};

mod routes;

/// The dedicated Postgres schema that holds **all** GitHub-integration state.
const SCHEMA: &str = "github";

/// The crate's embedded migrations, applied inside the `github` schema.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Per-crate shared state.
///
/// The GitHub routes operate entirely on the Postgres pool — their stores, diff
/// cache, and review drafts live in the `github` schema. They never touch the
/// server's `AppState`, which keeps this crate independent of `cctui-server`.
#[derive(Clone)]
pub struct GithubState {
    pub pool: PgPool,
}

/// Run the crate's embedded migrations against the dedicated `github` Postgres
/// schema.
///
/// The schema is created if absent, then the migrator runs on a connection
/// whose `search_path` is pinned to `github`. sqlx resolves the unqualified
/// objects in each migration — *including its own `_sqlx_migrations`
/// bookkeeping table* — against that `search_path`, so all of it lands in
/// `github.*` rather than `public.*`. The GitHub migration history is therefore
/// fully independent of core's `public._sqlx_migrations`, and a single
/// [`uninstall`] (`DROP SCHEMA github CASCADE`) removes every trace of it.
///
/// The pinned `search_path` is acquired with [`PgPool::acquire`] and only
/// applies to that one connection, so other pool users are unaffected.
///
/// # Errors
/// Returns an error if the schema cannot be created or a migration fails.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    // Create the schema up front so the pinned `search_path` resolves. The
    // identifier is a hard-coded constant, so the lack of binding is safe.
    pool.execute(format!("CREATE SCHEMA IF NOT EXISTS {SCHEMA}").as_str()).await?;

    // Pin one connection's search_path to `github`. The migrator creates
    // `_sqlx_migrations` and every migration's tables relative to it.
    let mut conn = pool.acquire().await?;
    conn.execute(format!("SET search_path TO {SCHEMA}").as_str()).await?;
    MIGRATOR.run(&mut conn).await?;

    tracing::info!("cctui-github: migrate() — {SCHEMA} schema up to date");
    Ok(())
}

/// Remove the GitHub integration's entire database footprint.
///
/// `DROP SCHEMA github CASCADE` drops every `github.*` table, its
/// `_sqlx_migrations` history, and all the **outbound** FK constraints that
/// `github.*` rows hold into core. Because core never references `github.*`
/// (the one-directional-FK invariant, docs/github-integration.md §7.2), core
/// tables are left completely intact — no stale rows, no dangling constraints.
///
/// Idempotent: a no-op if the schema was never created.
///
/// # Errors
/// Returns an error if the `DROP SCHEMA` statement fails.
pub async fn uninstall(pool: &PgPool) -> anyhow::Result<()> {
    pool.execute(format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE").as_str()).await?;
    tracing::info!("cctui-github: uninstall() — {SCHEMA} schema dropped");
    Ok(())
}

/// The GitHub route surface, mounted by `cctui-server` under `/api/v1`.
///
/// Returns a self-contained [`Router`] with its own [`GithubState`] already
/// applied, so the server merges it after `with_state`.
///
/// Paths mirror §9 of the design doc; handler bodies are placeholders until the
/// later GH-* tickets.
pub fn routes(pool: PgPool) -> Router {
    let state = GithubState { pool };
    Router::new()
        .route("/github/connectors", get(routes::list_connectors).post(routes::create_connector))
        .route("/github/pulls", get(routes::list_pulls))
        .route("/triggers/github", post(routes::webhook))
        .with_state(state)
}
