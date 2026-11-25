//! Session lifecycle, daemon transport, spawn, follow-ups, presets, inbox and
//! the session-scoped routes. Depends only on the kernel: calls into other
//! domains go through the [`ports`] traits the host implements.

pub mod auto_archive;
pub mod auto_followup;
pub mod auto_resume;
pub mod bandwidth_watch;
pub mod brief;
pub mod cache_bust;
pub mod context;
pub mod daemon_checkpoints;
pub mod followup;
pub mod followup_op;
pub mod guard_step;
pub mod hooks;
pub mod inbox;
pub mod keepalive;
pub mod lifecycle;
pub mod live_sessions;
pub mod outbox;
pub mod outcome;
pub mod ports;
pub mod presets;
pub mod redteam_runs;
pub mod results;
pub mod routes;
pub mod scheduled_messages;
pub mod scheduled_spawns;
pub mod sealed_env;
pub mod session_emoji;
pub mod session_viewers;
pub mod spawn_labels;
pub mod store;
pub mod targets;
pub mod transcript_md;
pub mod turn_dedupe;
pub mod undo_window;
pub mod uploads;
pub mod ws;

use cctui_proto::api::soft_limit;
use cctui_server_kernel::{
    auth, authz, bus, commands, cost, crypto, ended_signals, error, execution_settings, http_cache,
    live_sessions_predicate, normalize, registry, state_events as events,
};
