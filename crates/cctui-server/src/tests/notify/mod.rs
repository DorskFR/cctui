//! Each module stands in for the notify module its tests came from, so their
//! `super::` paths resolve against the host state.

pub mod notify {
    pub use cctui_server_notify::notify::*;

    pub mod env_owner_tests;
}

mod notification_endpoints {
    pub use cctui_server_notify::routes::notification_endpoints::*;

    mod tests;
}

mod notifications {
    pub use cctui_server_notify::routes::notifications::*;

    mod tests;
}

mod producers {
    pub use cctui_server_notify::notify::producers::*;

    mod tests;
}

mod user_actions {
    use axum::Json;
    use axum::extract::{Path, State};
    use cctui_proto::api::{UserActionResolver, UserActionStatus};
    use uuid::Uuid;

    use crate::state::AppState;
    pub use cctui_server_notify::routes::user_actions::*;

    mod tests;
}
