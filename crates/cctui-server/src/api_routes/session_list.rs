//! Session listing, stats and search routes.

use super::GET;
use crate::authz::Routes;
use axum::routing::get;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Self-scoped list/stats/search endpoints — `owner_filter()` filter in
        // the handler (admin sees all rows, others only their own).
        .add(
            &[GET],
            "/sessions",
            "List your sessions (admin: all).",
            get(cctui_server_sessions::routes::sessions::list_sessions::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/events",
            "Lifecycle event log (sessions, machines, system), keyset-paginated newest first.",
            get(cctui_server_sessions::routes::events::list_events::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats",
            "Aggregate session counts/status stats.",
            get(cctui_server_sessions::routes::stats::session_stats::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/tokens",
            "Token-usage stats across sessions.",
            get(
                cctui_server_sessions::routes::stats::session_token_stats::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/usage",
            "Overview usage analytics: tokens over time, per-model, heatmap.",
            get(cctui_server_sessions::routes::stats::session_usage_analytics::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/cache-busts",
            "Dollars lost to prompt-cache busts per day, by reason.",
            get(cctui_server_sessions::routes::cache_loss::cache_loss::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search",
            "Full-text search across your sessions.",
            get(cctui_server_sessions::routes::sessions::search_sessions::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search/values",
            "Autocomplete values for a search field.",
            get(cctui_server_sessions::routes::sessions::search_field_values::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/recent-dirs",
            "List recently used working directories.",
            get(cctui_server_sessions::routes::stats::recent_dirs::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
}
