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

mod anchor;
mod attention;
mod classifier_feed;
mod crypto;
mod diff;
mod drafts;
mod publish;
mod reconcile;
mod routes;
mod store;
mod viewed;
mod webhook;

pub use anchor::resolve as resolve_comment_anchor;
pub use attention::{Viewer, derive_bucket, derive_bucket_from_rows};
pub use classifier_feed::{derive_status, pr_href, publish as publish_pr_status, refresh};
pub use drafts::{
    DraftError, add_comment, delete_comment, delete_draft, list_drafts, mark_published,
    open_user_draft, update_comment, update_verdict,
};
pub use publish::{
    PublishError, ReviewPayload, ReviewSubmitClient, assemble_review_payload, verdict_event,
};
pub use reconcile::{interval_secs as reconcile_interval_secs, spawn as spawn_reconcile};
pub use store::{
    EventTx, list_open_threads, upsert_check, upsert_pull, upsert_review, upsert_review_comment,
    upsert_review_thread,
};
pub use viewed::{
    ViewedError, is_still_reviewed, list as list_viewed_marks, mark as mark_viewed,
    unmark as unmark_viewed,
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
    /// Per-head-SHA diff cache (GH-VIEW-1). In-memory by design — it holds no
    /// `github.*` rows, so `DROP SCHEMA github CASCADE` (uninstall) leaves
    /// nothing stale, and a restart simply re-fetches once.
    pub diff_cache: diff::DiffCache,
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
    /// `true` when the crate is compiled in **and** the `github` schema exists
    /// (the migrator ran) — i.e. the integration is *installed and reachable*,
    /// regardless of whether any connector is configured yet. The webui gates
    /// the **nav item + `/github` route** on this, so the connector setup UI is
    /// reachable to add the *first* connector (CCT-395). Without this split,
    /// `enabled` (which needs a connector) gated the only UI that can create a
    /// connector — an unreachable first run.
    pub available: bool,
    /// `true` when the `github` schema exists **and** at least one connector is
    /// configured. The webui gates **data features** (the live inbox) on this;
    /// `available && !enabled` is the "add your first GitHub account" state.
    pub enabled: bool,
    /// `owner/name` slugs of the repos the integration tracks. Empty until a
    /// later GH-* story populates connector repos; the field exists now so the
    /// capability shape is stable across stories.
    pub repos: Vec<String>,
}

/// Compute the live GitHub [`GithubCapability`] from the database.
///
/// `available` is `true` when the `connectors` query succeeds (the `github`
/// schema and its tables exist — the migrator ran). `enabled` additionally
/// requires at least one connector row. If the schema is absent or the query
/// errors, both degrade to `false` (and `repos` is empty) rather than failing
/// the whole `/capabilities` response.
pub async fn capability(pool: &PgPool) -> GithubCapability {
    // Whether this query succeeds is itself the "schema exists" probe.
    let count: Option<i64> =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {SCHEMA}.connectors"))
            .fetch_one(pool)
            .await
            .ok();
    let Some(count) = count else {
        // Schema/tables not present → integration not installed/reachable.
        return GithubCapability { available: false, enabled: false, repos: Vec::new() };
    };
    // Distinct repo slugs across all connectors, so the webui can show what the
    // integration tracks. Best-effort: a query error degrades to an empty list
    // rather than failing the capability probe.
    let repos: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT DISTINCT unnest(repos) FROM {SCHEMA}.connectors ORDER BY 1"
    ))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    GithubCapability { available: true, enabled: count > 0, repos }
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
    let state = GithubState { pool, events, pr_cache, diff_cache: diff::DiffCache::new() };
    Router::new()
        .route("/github/connectors", get(routes::list_connectors).post(routes::create_connector))
        .route("/github/connectors/{id}", delete(routes::delete_connector))
        .route("/github/pulls", get(routes::list_pulls))
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/diff",
            get(routes::pull_diff),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/drafts",
            get(routes::list_drafts).post(routes::create_draft),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/drafts/{draft_id}",
            axum::routing::patch(routes::update_draft).delete(routes::delete_draft),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/drafts/{draft_id}/comments",
            post(routes::create_draft_comment),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/drafts/{draft_id}/comments/{comment_id}",
            axum::routing::patch(routes::update_draft_comment).delete(routes::delete_draft_comment),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/publish-review",
            post(routes::publish_review),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/threads",
            get(routes::list_threads),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/viewed",
            get(routes::list_viewed),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/mark-viewed",
            post(routes::mark_viewed),
        )
        .route(
            "/github/pulls/{connector_id}/{owner}/{name}/{number}/unmark-viewed",
            post(routes::unmark_viewed),
        )
        .route("/triggers/github", post(webhook::webhook))
        .with_state(state)
}
