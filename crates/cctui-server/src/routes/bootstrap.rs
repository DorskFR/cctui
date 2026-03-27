use axum::Extension;
use axum::extract::State;

use crate::auth::AuthContext;
use crate::state::AppState;

const BOOTSTRAP_TPL: &str = include_str!("../../../../scripts/bootstrap.sh.tpl");
const SETUP_TPL: &str = include_str!("../../../../scripts/setup.py.tpl");
const STREAMER_PY: &str = include_str!("../../../../scripts/streamer.py");

/// Serve the bootstrap.sh script with server URL and default agent token baked in.
pub async fn bootstrap_script(State(state): State<AppState>) -> String {
    let default_token =
        crate::config::Config::agent_tokens().into_iter().next().unwrap_or_default();
    BOOTSTRAP_TPL
        .replace("__SERVER_URL__", &state.config.external_url)
        .replace("__TOKEN__", &default_token)
}

/// Serve the setup.py script with server URL and token baked in.
pub async fn setup(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    SETUP_TPL.replace("__SERVER_URL__", &state.config.external_url).replace("__TOKEN__", &ctx.token)
}

/// Serve the streamer.py script as-is.
pub async fn streamer() -> &'static str {
    STREAMER_PY
}
