//! Session listing, stats and search routes.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::routing::get;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Self-scoped list/stats/search endpoints — `owner_filter()` filter in
        // the handler (admin sees all rows, others only their own).
        .add(
            &[GET],
            "/sessions",
            "List your sessions (admin: all).",
            get(routes::sessions::list_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats",
            "Aggregate session counts/status stats.",
            get(routes::stats::session_stats),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/tokens",
            "Token-usage stats across sessions.",
            get(routes::stats::session_token_stats),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/usage",
            "Overview usage analytics: tokens over time, per-model, heatmap.",
            get(routes::stats::session_usage_analytics),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/cache-busts",
            "Dollars lost to prompt-cache busts per day, by reason.",
            get(routes::cache_loss::cache_loss),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search",
            "Full-text search across your sessions.",
            get(routes::sessions::search_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search/values",
            "Autocomplete values for a search field.",
            get(routes::sessions::search_field_values),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/recent-dirs",
            "List recently used working directories.",
            get(routes::stats::recent_dirs),
            Authn::Bearer,
            Authenticated,
        )
}
