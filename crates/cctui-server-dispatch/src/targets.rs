//! Execution target liveness independent of workspace presentation.
pub use cctui_server_kernel::dispatch_types::{OFFLINE_SECS, ONLINE_SECS, derive_liveness};

pub mod enrollment;

pub mod dispatcher;
pub mod dispatcher_liveness;
pub mod dispatcher_management;

pub mod machine_status;
pub mod manifest;
