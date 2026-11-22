use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::ApiError;
use cctui_server_notify::hooks::{NotifyDispatch, NotifySessions};
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;

impl NotifySessions for AppState {
    fn wake_lane(&self, lane_id: Uuid) {
        crate::inbox::deliver::wake_lane(self, lane_id);
    }
    async fn machine_user(
        &self,
        headers: &HeaderMap,
    ) -> Result<Uuid, (StatusCode, Json<ApiError>)> {
        crate::routes::spawn_child::machine_user(self, headers).await
    }
}

impl NotifyDispatch for AppState {
    async fn dismiss_dispatch_record(
        &self,
        pool: &PgPool,
        viewer: Uuid,
        owner: Option<Uuid>,
        session_id: &str,
    ) -> Result<bool, sqlx::Error> {
        cctui_server_dispatch::store::dispatch_records::dismiss(pool, viewer, owner, session_id)
            .await
    }
}
