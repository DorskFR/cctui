//! Bookmark and prompt-library routes.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, patch};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET, Method::POST],
        "/bookmarks",
        "List your saved messages, or save one.",
        get(cctui_server_sessions::routes::bookmarks::list_bookmarks::<crate::state::AppState>)
            .post(
                cctui_server_sessions::routes::bookmarks::create_bookmark::<crate::state::AppState>,
            ),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/bookmarks/{id}",
        "Edit a bookmark's title/note, or delete it.",
        patch(cctui_server_sessions::routes::bookmarks::update_bookmark::<crate::state::AppState>)
            .delete(
                cctui_server_sessions::routes::bookmarks::delete_bookmark::<crate::state::AppState>,
            ),
        Authn::Bearer,
        Authenticated,
    )
    // Prompts: owner_filter() filter in the handler.
    .add(
        &[GET, Method::POST],
        "/prompts",
        "List your saved prompts, or create one.",
        get(cctui_server_sessions::routes::prompts::list_prompts::<crate::state::AppState>)
            .post(cctui_server_sessions::routes::prompts::create_prompt::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/prompts/resolve",
        "Resolve a prompt by name/reference.",
        get(cctui_server_sessions::routes::prompts::resolve_prompt::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET, Method::DELETE],
        "/prompts/{id}",
        "Get or delete a saved prompt.",
        get(cctui_server_sessions::routes::prompts::get_prompt::<crate::state::AppState>).delete(
            cctui_server_sessions::routes::prompts::delete_prompt::<crate::state::AppState>,
        ),
        Authn::Bearer,
        Authenticated,
    )
}
