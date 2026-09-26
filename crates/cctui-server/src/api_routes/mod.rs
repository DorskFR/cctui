//! The `/api/v1` route table, registered area by area.

mod account_usage;
mod accounts;
mod admin_instance;
mod admin_users;
mod daemon;
mod dispatch;
mod keys;
mod labels;
mod machines;
mod me;
mod passkeys;
mod profiles;
mod prompts;
mod session_bulk;
mod session_control;
mod session_lifecycle;
mod session_list;
mod session_messages;
mod session_state;
mod session_view;
mod shares;
mod skills;
mod users;
mod version;

use axum::http::Method;

use crate::authz::{Action, Authz, IdFrom, ResourceKind, Routes};

const GET: Method = Method::GET;

// Per-session ownership guard: `machine_uuid -> machines.user_id`,
// id sourced from the `{id}` path param. `read`/`write` differ only in the
// recorded `Action` (for RBAC); the owner rule is identical today.
const fn sess_read() -> Authz {
    Authz::Resource(ResourceKind::Session, Action::Read, IdFrom::Path("id"))
}

const fn sess_write() -> Authz {
    Authz::Resource(ResourceKind::Session, Action::Write, IdFrom::Path("id"))
}

pub fn register(r: Routes) -> Routes {
    let r = passkeys::register(r);
    let r = version::register(r);
    let r = session_lifecycle::register(r);
    let r = dispatch::register(r);
    let r = session_bulk::register(r);
    let r = session_list::register(r);
    let r = session_view::register(r);
    let r = session_messages::register(r);
    let r = session_control::register(r);
    let r = session_state::register(r);
    let r = labels::register(r);
    let r = daemon::register(r);
    let r = prompts::register(r);
    let r = keys::register(r);
    let r = accounts::register(r);
    let r = profiles::register(r);
    let r = account_usage::register(r);
    let r = shares::register(r);
    let r = machines::register(r);
    let r = me::register(r);
    let r = admin_instance::register(r);
    let r = admin_users::register(r);
    let r = skills::register(r);
    users::register(r)
}
