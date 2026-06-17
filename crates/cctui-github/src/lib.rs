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
use axum::routing::{delete, get, post};
use sqlx::{Connection, Executor, PgPool};

mod attention;
mod classifier_feed;
mod crypto;
mod routes;
mod store;
mod webhook;

pub use attention::{Viewer, derive_bucket, derive_bucket_from_rows};
pub use classifier_feed::{derive_status, pr_href, publish as publish_pr_status, refresh};
pub use store::{
    EventTx, upsert_check, upsert_pull, upsert_review, upsert_review_comment, upsert_review_thread,
};

/// The dedicated Postgres schema that holds **all** GitHub-integration state.
const SCHEMA: &str = "github";

/// The crate's embedded migrations, applied inside the `github` schema.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Per-crate shared state.
///
/// The GitHub routes operate entirely on the Postgres pool — their stores, diff
/// cache, and review drafts live in the `github` schema. They never touch the
/// server's `AppState`, which keeps this crate independent of `cctui-server`.
///
/// `events` is a clone of the server's client-WS broadcast sender (typed on
/// `cctui_proto::ws::ServerEvent`, which proto owns — so no dependency on
/// `cctui-server`). The store functions broadcast a `GithubEvent` on every
/// successful upsert for live `/github` inbox push (docs §6.1).
#[derive(Clone)]
pub struct GithubState {
    pub pool: PgPool,
    pub events: EventTx,
    /// The core-owned, best-effort PR status cache the session classifier reads
    /// (GH-CLS-1, docs §6.1). The webhook publishes a PR's derived check/review
    /// state into it after each upsert. A `cctui-proto` type, so this stays
    /// one-directional — the crate never depends on `cctui-server`.
    pub pr_cache: cctui_proto::classifier::PrStatusCache,
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
/// The `search_path` pin is session-scoped, so the migration connection is
/// **detached from the pool and closed** afterwards rather than returned to it.
/// `SET search_path` survives on a pooled connection, so handing it back would
/// poison whichever core query next reused it (core's unqualified `sessions`,
/// `machines`, … would resolve against `github` and fail with "relation does
/// not exist"). Discarding the connection makes the pin strictly local; the
/// pool simply opens a fresh, default-`search_path` connection next time.
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

    // Detach + close so the search_path-pinned connection never re-enters the
    // pool and poisons a core query. (See the doc comment above.)
    conn.detach().close().await?;

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

/// Whether the GitHub integration is operational, plus the repos it tracks.
///
/// Returned to core's `GET /api/v1/capabilities` handler (which builds the
/// outer `{ github: … }` envelope) so the webui can capability-gate the nav
/// item, the lazy `/github` route, and the contextual actions
/// (docs/github-integration.md §7.4). The crate owns this query because it is
/// the only code that knows the `github` schema's shape; core merely forwards
/// the result when the `github` feature is compiled in.
pub struct GithubCapability {
    /// `true` when the `github` schema exists **and** at least one connector is
    /// configured. Compiling the crate in is necessary but not sufficient — a
    /// feature-on build with no connector still reports `false`.
    pub enabled: bool,
    /// `owner/name` slugs of the repos the integration tracks. Empty until a
    /// later GH-* story populates connector repos; the field exists now so the
    /// capability shape is stable across stories.
    pub repos: Vec<String>,
}

/// Compute the live GitHub [`GithubCapability`] from the database.
///
/// `enabled` requires both that the `github` schema exists (the crate's
/// migrations ran) and that at least one connector row is present. A schema
/// that is absent — or a query error — degrades gracefully to "disabled"
/// rather than failing the whole `/capabilities` response.
pub async fn capability(pool: &PgPool) -> GithubCapability {
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {SCHEMA}.connectors"))
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    // Distinct repo slugs across all connectors, so the webui can show what the
    // integration tracks. Best-effort: a query error degrades to an empty list
    // rather than failing the capability probe.
    let repos: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT DISTINCT unnest(repos) FROM {SCHEMA}.connectors ORDER BY 1"
    ))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    GithubCapability { enabled: count > 0, repos }
}

/// The GitHub route surface, mounted by `cctui-server` under `/api/v1`.
///
/// Returns a self-contained [`Router`] with its own [`GithubState`] already
/// applied, so the server merges it after `with_state`.
///
/// Paths mirror §9 of the design doc; handler bodies are placeholders until the
/// later GH-* tickets.
pub fn routes(
    pool: PgPool,
    events: EventTx,
    pr_cache: cctui_proto::classifier::PrStatusCache,
) -> Router {
    let state = GithubState { pool, events, pr_cache };
    Router::new()
        .route("/github/connectors", get(routes::list_connectors).post(routes::create_connector))
        .route("/github/connectors/{id}", delete(routes::delete_connector))
        .route("/github/pulls", get(routes::list_pulls))
        .route("/triggers/github", post(webhook::webhook))
        .with_state(state)
}
