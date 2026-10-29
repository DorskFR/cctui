//! Automation host assembled exclusively from shared execution services.
pub mod host;
pub mod router;

pub mod daemon;

pub mod runs;

pub mod launch_credentials;

pub mod accounts;

pub mod spawn;

pub mod background;

pub mod children;

mod session_limits;

pub mod termination;
