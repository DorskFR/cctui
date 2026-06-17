//! GitHub integration for cctui — connector, diff proxy, review store, and
//! routes, fully encapsulated behind the server's `github` Cargo feature.
//!
//! Everything GitHub-specific lives in this crate so a build without the feature
//! contains zero GitHub code, routes, or schema. See `docs/github-integration.md`
//! §7 for the plugin design (dedicated `github` Postgres schema, one-directional
//! FKs into core, `DROP SCHEMA github CASCADE` uninstall).
//!
//! This crate (GH-PKG-1) is the skeleton: it exposes [`migrate`] and [`routes`]
//! and wires the route surface from §9. The `github` schema + `search_path`-
//! isolated migrator (GH-PKG-2) and the real handler bodies (later GH-* tickets)
//! land on top of this.

use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;

mod routes;

/// Per-crate shared state.
///
/// The GitHub routes operate entirely on the Postgres pool — their stores, diff
/// cache, and review drafts live in the `github` schema. They never touch the
/// server's `AppState`, which keeps this crate independent of `cctui-server`.
#[derive(Clone)]
pub struct GithubState {
    pub pool: PgPool,
}

/// Run the crate's embedded migrations against the `github` Postgres schema.
///
/// GH-PKG-1 is the skeleton: the `search_path`-isolated migrator and the schema
/// itself are owned by GH-PKG-2, so this is currently a no-op. The call site in
/// `cctui-server`'s `main.rs` is wired now so GH-PKG-2 only has to fill in the
/// body.
///
/// # Errors
/// Returns an error if a future migration fails to apply.
// `async` is kept deliberately: GH-PKG-2 fills the body with `.await`ing
// migrator calls, and the `main.rs` call site already `.await`s this.
#[allow(clippy::unused_async)]
pub async fn migrate(_pool: &PgPool) -> anyhow::Result<()> {
    tracing::info!("cctui-github: migrate() — no schema yet (owned by GH-PKG-2)");
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
