//! `GET /api/v1/capabilities` — which optional integrations this server has,
//! and whether each is operational (CCT-375 / GH-CAP-1).
//!
//! The endpoint lives in **core** and always exists, regardless of Cargo
//! features, so the webui can fetch one stable shape and capability-gate its UI
//! (docs/github-integration.md §7.4). When the `github` feature is compiled in,
//! the handler forwards to the crate's `capability()` query (the crate owns the
//! `github` schema, so only it can answer "is a connector configured?"). When
//! the feature is off, GitHub reports `enabled: false` with no repos — the code
//! that would query the schema isn't even built.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use ts_rs::TS;

use crate::state::AppState;

/// The GitHub integration's capability, as seen by the webui.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct GithubCapability {
    /// `true` when the crate is compiled in **and** the `github` schema exists —
    /// the integration is installed and reachable, even with zero connectors.
    /// The webui gates the nav item + `/github` route on this so the connector
    /// setup UI is reachable to add the first connector (CCT-395). A feature-off
    /// build always reports `false`.
    pub available: bool,
    /// `true` only when `available` **and** at least one connector is
    /// configured. A feature-on build with no connector reports `false`; a
    /// feature-off build always reports `false`. Gates data features (the
    /// inbox); `available && !enabled` is the "add your first account" state.
    pub enabled: bool,
    /// `owner/name` slugs the integration tracks (empty until a later GH-*
    /// story populates them).
    pub repos: Vec<String>,
}

/// The capability envelope. One field per optional integration; the webui reads
/// `github.available` to mount the lazy `/github` route + nav, and
/// `github.enabled` to decide between the inbox and the first-run setup state.
///
/// CCT-399: `claude_litellm_models` was dropped — self-hosted models are now a
/// per-account property surfaced by `GET /accounts`, not a server-global list.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CapabilitiesResponse {
    pub github: GithubCapability,
}

/// `GET /api/v1/capabilities`.
#[cfg_attr(not(feature = "github"), allow(unused_variables))]
pub async fn capabilities(State(state): State<AppState>) -> Json<CapabilitiesResponse> {
    #[cfg(feature = "github")]
    let github = {
        let cap = cctui_github::capability(&state.pool).await;
        GithubCapability { available: cap.available, enabled: cap.enabled, repos: cap.repos }
    };
    #[cfg(not(feature = "github"))]
    let github = GithubCapability { available: false, enabled: false, repos: Vec::new() };

    Json(CapabilitiesResponse { github })
}
