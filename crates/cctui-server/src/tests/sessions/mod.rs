//! Each module stands in for the sessions module its tests came from, so their
//! `super::` paths resolve against the host state.

mod auto_archive {
    pub use cctui_server_sessions::auto_archive::*;

    mod tests;
}

mod auto_followup {
    pub use cctui_server_sessions::auto_followup::*;

    mod tests;
}

mod auto_resume {
    pub use cctui_server_sessions::auto_resume::*;

    mod tests;
}

mod followup {
    pub use cctui_server_sessions::followup::*;

    mod prompt_tests;

    mod thread {
        pub use cctui_server_sessions::followup::thread::*;

        use cctui_server_kernel::sessions_types::Reader;

        mod tests;
    }
}

mod followup_op {
    pub use cctui_server_sessions::followup_op::*;

    mod tests;
}

mod guard_step {
    pub use cctui_server_sessions::guard_step::*;

    use cctui_proto::guard_step::GuardStepState;

    use chrono::Utc;

    use sqlx::PgPool;

    mod tests;
}

mod inbox {
    pub use cctui_server_sessions::inbox::*;

    use cctui_server_kernel::sessions_types::Source;

    mod guards_tests;
    mod preset_tests;

    mod timer {
        pub use cctui_server_sessions::inbox::timer::*;

        mod tests;
    }
}

mod keepalive {
    pub use cctui_server_sessions::keepalive::*;

    mod tests;
}

mod outcome {
    pub use cctui_server_sessions::outcome::*;

    mod tests;
}

mod presets {
    mod from_draft {

        mod tests;
    }
}

mod routes {
    mod attachments {
        pub use cctui_server_sessions::routes::attachments::*;

        use axum::http::StatusCode;

        use cctui_server_sessions::uploads::RawUpload;

        mod tests;
    }

    mod capabilities {

        use cctui_proto::api::capabilities::{CapabilitiesResponse, LangfuseCapability};

        mod tests;
    }

    mod codex_models {
        pub use cctui_server_sessions::routes::codex_models::*;

        use chrono::Utc;

        use serde_json::Value;

        mod tests;
    }

    mod daemon {
        mod daemon_lost {
            pub use cctui_server_sessions::routes::daemon::daemon_lost::*;

            mod tests;
        }

        mod events {
            mod prompts {
                use crate::state::AppState;
                use cctui_proto::adapter::AdapterEvent;
                use cctui_proto::features::Feature;
                use cctui_server_sessions::routes::daemon::bumps::Bumps;
                pub use cctui_server_sessions::routes::daemon::events::prompts::*;

                mod tests;
            }

            mod relay {
                use crate::state::AppState;
                use cctui_proto::adapter::AdapterEvent;
                pub use cctui_server_sessions::routes::daemon::events::relay::*;

                mod tests;
            }

            mod session {
                use crate::state::AppState;
                use cctui_proto::adapter::{AdapterEvent, EndReason};
                pub use cctui_server_sessions::routes::daemon::events::session::*;

                use serde_json::json;
                use uuid::Uuid;

                mod tests;
            }

            mod status {
                pub use cctui_server_sessions::routes::daemon::events::status::*;

                mod tests;
            }

            mod stream {

                use cctui_server_accounts::daemon_usage::{
                    gateway_metered_adapter_patterns, insert_token_usage,
                };

                mod tests;
            }
        }

        mod gateway_env {
            use crate::state::AppState;
            pub use cctui_server_sessions::routes::daemon::gateway_env::*;

            use uuid::Uuid;

            use cctui_server_dispatch::launch_credentials::{
                gateway_env_allowed, spawn_capability_for,
            };

            mod tests;
        }

        mod ingest {
            pub use cctui_server_sessions::routes::daemon::ingest::*;

            use uuid::Uuid;

            mod tests;
        }

        mod ownership {
            pub use cctui_server_sessions::routes::daemon::ownership::*;

            mod tests;
        }

        mod registration {
            use crate::state::AppState;
            pub use cctui_server_sessions::routes::daemon::registration::*;

            use uuid::Uuid;

            mod tests;
        }
    }

    mod drafts {
        use crate::state::AppState;
        use axum::extract::{Path, State};
        use axum::http::StatusCode;
        use axum::{Extension, Json};
        use cctui_proto::drafts::{DRAFT_CAP_PER_USER, PutDraftRequest};
        use cctui_server_kernel::auth::AuthContext;
        pub use cctui_server_sessions::routes::drafts::*;

        mod tests;
    }

    mod events {
        use crate::state::AppState;
        use axum::extract::{Path, Query, State};
        pub use cctui_server_sessions::routes::events::*;

        use axum::{Extension, Json};

        use cctui_server_kernel::auth::AuthContext;
        use uuid::Uuid;

        mod tests;
    }

    mod fs {
        use crate::state::AppState;
        use axum::extract::{Path, Query, State};
        use axum::http::StatusCode;
        pub use cctui_server_sessions::routes::fs::*;

        use uuid::Uuid;

        use cctui_server_kernel::error::AppError;

        mod tests;
    }

    mod hidden_owners {
        pub use cctui_server_sessions::routes::hidden_owners::*;

        mod tests;
    }

    mod hooks {
        pub use cctui_server_sessions::routes::hooks::*;

        mod tests;
    }

    mod images {

        mod authz_tests;
    }

    mod inbox {
        pub use cctui_server_sessions::routes::inbox::*;

        mod tests;
    }

    mod labels {
        use crate::state::AppState;
        use axum::extract::{Path, State};
        use axum::http::StatusCode;
        use axum::{Extension, Json};
        use cctui_proto::api::{AttachLabelRequest, CreateLabelRequest, UpdateLabelRequest};
        use cctui_server_kernel::auth::AuthContext;
        pub use cctui_server_sessions::routes::labels::*;

        mod tests;
    }

    mod presets {
        pub use cctui_server_sessions::routes::presets::*;

        mod tests;
    }

    mod privacy_scan {
        use crate::state::AppState;
        pub use cctui_server_sessions::routes::privacy_scan::*;

        use sqlx::PgPool;

        use cctui_proto::api::privacy_scan::PrivacyScanJob;

        mod tests;
    }

    mod redteam {
        pub use cctui_server_sessions::routes::redteam::*;

        mod tests;
    }

    mod sessions {
        pub use cctui_server_sessions::routes::sessions::*;

        use cctui_proto::adapter::RemoveInitiator;

        use cctui_server_kernel::sessions_types::ArchiveOutcome;

        mod switch_account_tests;
        mod tests;
    }

    mod settings {
        use crate::state::AppState;
        use axum::extract::State;
        use axum::http::StatusCode;
        use axum::{Extension, Json};
        pub use cctui_server_sessions::routes::settings::*;

        use cctui_proto::api::settings::SettingsPayload;
        use cctui_server_kernel::auth::AuthContext;

        use cctui_server_kernel::execution_settings::{
            clamp_secret_scrub, harness_mode_to_adapter_token, secret_scrub_of,
        };

        mod tests;
    }

    mod spawn {
        pub use cctui_server_sessions::routes::spawn::*;

        use cctui_server_dispatch::machine_policy::{
            authorize_launch, claim_spawner, fill_spawner, remember_spawner, resolve_owned_machine,
            resolve_spawn_machine, spawn_adapter, spawnable_adapters,
        };
        use cctui_server_dispatch::machine_spawn::{
            AccountDecision, AutoAccountRow, auto_account_candidates, decide_account,
            resolve_default_account,
        };

        mod draft_seal_tests;
        mod launch_gate_tests;
        mod missing_cwd_tests;
        mod queue_tests;
        mod spawn_share_tests;
        mod tests;
    }

    mod spawn_child {
        use crate::state::AppState;
        pub use cctui_server_sessions::routes::spawn_child::*;

        use axum::http::StatusCode;
        use cctui_proto::api::{ArchiveChildResponse, SpawnChildRequest};
        use cctui_server_kernel::dispatch_types::Parent;
        use cctui_server_kernel::error::AppError;
        use uuid::Uuid;

        use cctui_proto::api::SpawnCapability;
        use cctui_server_dispatch::child_exec::{
            MAX_COMPLETION_BYTES, child_account_env, child_spawner, resolve_child_adapter,
        };
        pub use cctui_server_dispatch::child_policy::{
            Authorized, Denied, Usage, authorize, release_child, tree_granted_usd,
        };
        use cctui_server_kernel::dispatch_types::ChildSlots;

        async fn reserve_child(
            pool: &sqlx::PgPool,
            parent_id: &str,
            cap: Option<&SpawnCapability>,
            req: &SpawnChildRequest,
            usage: Usage,
            child_key: &str,
        ) -> Result<(Authorized, SpawnCapability), AppError> {
            cctui_server_dispatch::child_policy::reserve_child(
                &crate::sessions_context::WorkspaceChildCount,
                pool,
                parent_id,
                cap,
                req,
                usage,
                child_key,
            )
            .await
        }

        mod tests;
    }

    mod spawn_memory {
        use crate::state::AppState;
        use axum::extract::State;
        use axum::{Extension, Json};
        use cctui_proto::drafts::{SpawnMemoryEntry, SpawnMemoryPayload};
        use cctui_server_kernel::auth::AuthContext;
        pub use cctui_server_sessions::routes::spawn_memory::*;
        use serde_json::Value;
        use std::collections::BTreeMap;

        mod tests;
    }
}

mod scheduled_messages {
    pub use cctui_server_sessions::scheduled_messages::*;

    mod fold_tests;
    mod reply_gate_tests;
}

mod scheduled_spawns {
    use crate::state::AppState;
    use cctui_proto::ws::{ScheduledLaunchState, ServerEvent};
    pub use cctui_server_sessions::scheduled_spawns::*;
    use chrono::{DateTime, Utc};
    use uuid::Uuid;

    use cctui_server_kernel::bus::Bus;
    use cctui_server_sessions::inbox::claim::BatchEvent;
    use cctui_server_sessions::inbox::deliver::Outcome;

    use cctui_server_sessions::store::sessions::SessionRowStatus;

    mod tests;
}

mod session_viewers {
    use crate::state::AppState;
    pub use cctui_server_sessions::session_viewers::*;
    use sqlx::PgPool;

    mod tests;
}

mod targets {
    mod daemon {
        mod reconcile {
            pub use cctui_server_sessions::targets::daemon::reconcile::*;

            use uuid::Uuid;

            mod tests;
        }
    }
}

mod ws {

    use cctui_server_kernel::config::origin_permitted;

    mod tests;
}
