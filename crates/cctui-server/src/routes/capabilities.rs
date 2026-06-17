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
    /// `true` only when the crate is compiled in **and** the `github` schema
    /// exists **and** at least one connector is configured. A feature-on build
    /// with no connector reports `false`; a feature-off build always reports
    /// `false`.
    pub enabled: bool,
    /// `owner/name` slugs the integration tracks (empty until a later GH-*
    /// story populates them).
    pub repos: Vec<String>,
}

/// One self-hosted Claude model the server offers, mirrored to the webui's spawn
/// picker (CCT-LITELLM). `model` is the `--model` code; `label` is the display
/// name. Present only when the operator configured both an endpoint and a model
/// list (otherwise the list is empty and the picker shows only native families).
/// The endpoint URL and its auth token are deliberately NOT exposed — they're
/// injected server-side at spawn.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct LiteLlmModelInfo {
    pub model: String,
    pub label: String,
}

/// The capability envelope. One field per optional integration; the webui reads
/// `github.enabled` to decide whether to mount the lazy `/github` route + nav.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CapabilitiesResponse {
    pub github: GithubCapability,
    /// Operator-declared self-hosted Claude models (empty unless configured).
    pub claude_litellm_models: Vec<LiteLlmModelInfo>,
}

/// `GET /api/v1/capabilities`.
#[cfg_attr(not(feature = "github"), allow(unused_variables))]
pub async fn capabilities(State(state): State<AppState>) -> Json<CapabilitiesResponse> {
    #[cfg(feature = "github")]
    let github = {
        let cap = cctui_github::capability(&state.pool).await;
        GithubCapability { enabled: cap.enabled, repos: cap.repos }
    };
    #[cfg(not(feature = "github"))]
    let github = GithubCapability { enabled: false, repos: Vec::new() };

    let claude_litellm_models = state
        .config
        .claude_litellm_visible_models()
        .iter()
        .map(|m| LiteLlmModelInfo { model: m.model.clone(), label: m.label.clone() })
        .collect();

    Json(CapabilitiesResponse { github, claude_litellm_models })
}
