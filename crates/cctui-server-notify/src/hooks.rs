//! Calls the notify code makes into other domains; the host forwards each
//! method to the owner's function of the same name.
use std::future::Future;

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::ApiError;
use sqlx::PgPool;
use uuid::Uuid;

/// Sessions: inbox delivery and machine-key auth.
pub trait NotifySessions: Send + Sync + 'static {
    /// `inbox::deliver::wake_lane`.
    fn wake_lane(&self, lane_id: Uuid);
    /// `routes::spawn_child::machine_user`.
    fn machine_user(
        &self,
        headers: &HeaderMap,
    ) -> impl Future<Output = Result<Uuid, (StatusCode, Json<ApiError>)>> + Send;
}

/// Dispatch: failure records.
pub trait NotifyDispatch: Send + Sync + 'static {
    /// `store::dispatch_records::dismiss`.
    fn dismiss_dispatch_record(
        &self,
        pool: &PgPool,
        viewer: Uuid,
        owner: Option<Uuid>,
        session_id: &str,
    ) -> impl Future<Output = Result<bool, sqlx::Error>> + Send;
}
