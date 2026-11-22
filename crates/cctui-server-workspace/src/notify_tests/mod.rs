//! Each module stands in for the notify module its tests came from, so their
//! `super::` paths resolve against the host state.

pub mod notify {
    pub use crate::notify::*;

    pub mod env_owner_tests;
}

mod notification_endpoints {
    pub use crate::routes::notification_endpoints::*;

    mod tests;
}

mod notifications {
    pub use crate::routes::notifications::*;

    mod tests;
}

mod producers {
    pub use crate::notify::producers::*;

    mod tests;
}

mod user_actions {
    use axum::Json;
    use axum::extract::{Path, State};
    use cctui_proto::api::{UserActionResolver, UserActionStatus};
    use uuid::Uuid;

    pub use crate::routes::user_actions::*;
    use crate::state::AppState;

    mod tests;
}
