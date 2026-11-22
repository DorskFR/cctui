//! Execution target liveness independent of workspace presentation.
pub use cctui_server_dispatch::targets::{
    OFFLINE_SECS, ONLINE_SECS, derive_liveness, dispatcher, dispatcher_liveness,
    dispatcher_management, enrollment, machine_status, manifest,
};

pub mod daemon;
