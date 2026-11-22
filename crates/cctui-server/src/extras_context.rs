use std::sync::Arc;

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::adapter::RemoveInitiator;
use cctui_proto::api::ApiError;
use cctui_proto::api::auto_followup::SendFollowup;
use cctui_proto::thread::{ThreadConversationPage, ThreadCursor};
use cctui_server_extras::context::{ExtrasContext, SearchReindex};
use cctui_server_extras::hooks::{BlobsHooks, SessionsHooks};
use cctui_server_extras::langfuse::LangfuseClient;
use cctui_server_extras::plugins::PluginRegistry;
use cctui_server_extras::presence::PodIdentity;
use cctui_server_kernel::auth::AuthContext;
use cctui_server_kernel::blobs_types::StoredBlob;
use cctui_server_kernel::error::AppError;
use cctui_server_kernel::events::Actor;
use cctui_server_kernel::sessions_types::{
    ArchiveOutcome, ConversationQuery, Followed, Header, Reader, RenderableRow, Rendered,
    RenderedThread,
};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::state::AppState;

impl SearchReindex for AppState {
    async fn reindex_search(&self, pool: &PgPool, ids: &[i64]) -> Result<(), sqlx::Error> {
        cctui_server_search::reindex(pool, ids).await
    }
}

impl BlobsHooks for AppState {
    async fn store_blob(
        &self,
        pool: &PgPool,
        bytes: &[u8],
        media_type: Option<&str>,
    ) -> Result<StoredBlob, sqlx::Error> {
        cctui_server_blobs::routes::store_blob(pool, bytes, media_type).await
    }
    async fn read_blob_in(
        &self,
        conn: &mut PgConnection,
        hash: &str,
    ) -> sqlx::Result<Option<Vec<u8>>> {
        cctui_server_blobs::routes::read_blob_in(conn, hash).await
    }
    async fn delete_unreferenced(&self, pool: &PgPool, hash: &str) {
        cctui_server_blobs::blob_store::delete_unreferenced(pool, hash).await;
    }
}

impl SessionsHooks for AppState {
    async fn renderable_rows(
        &self,
        pool: &PgPool,
        session_id: &str,
        adapter_id: &str,
        params: &ConversationQuery,
    ) -> Result<Vec<RenderableRow>, sqlx::Error> {
        cctui_server_sessions::routes::sessions::renderable_rows(
            pool, session_id, adapter_id, params,
        )
        .await
    }
    async fn archive_one(
        &self,
        session_id: &str,
        force: bool,
        initiator: RemoveInitiator,
        actor: Actor,
    ) -> Result<ArchiveOutcome, sqlx::Error> {
        cctui_server_sessions::routes::sessions::archive_one(
            self, session_id, force, initiator, actor,
        )
        .await
    }
    async fn machine_user(
        &self,
        headers: &HeaderMap,
    ) -> Result<Uuid, (StatusCode, Json<ApiError>)> {
        cctui_server_sessions::routes::spawn_child::machine_user(self, headers).await
    }
    async fn route(
        &self,
        caller: Option<&AuthContext>,
        session_id: &str,
        followup: SendFollowup,
        text: &str,
        message_key: Option<&str>,
    ) -> Result<Option<Followed>, AppError> {
        cctui_server_sessions::auto_followup::route(
            self,
            caller,
            session_id,
            followup,
            text,
            message_key,
        )
        .await
    }
    async fn refuse_while_pending(&self, pool: &PgPool, session_id: &str) -> Result<(), AppError> {
        cctui_server_sessions::followup_op::refuse_while_pending(pool, session_id).await
    }
    async fn conversation(
        &self,
        reader: Reader<'_>,
        id: &str,
        cursor: Option<ThreadCursor>,
        limit: i64,
    ) -> Result<Option<ThreadConversationPage>, AppError> {
        cctui_server_sessions::followup::thread::conversation(self, reader, id, cursor, limit).await
    }
    fn render(
        &self,
        header: &Header,
        events: &[(i64, Value)],
        roles: &[String],
        budget: usize,
    ) -> Rendered {
        cctui_server_sessions::transcript_md::render(header, events, roles, budget)
    }
    fn render_thread(
        &self,
        page: &ThreadConversationPage,
        roles: &[String],
        budget: usize,
    ) -> RenderedThread {
        cctui_server_sessions::transcript_md::render_thread(page, roles, budget)
    }
}

impl ExtrasContext for AppState {
    fn langfuse(&self) -> Option<&Arc<LangfuseClient>> {
        self.langfuse.as_ref()
    }
    fn plugins(&self) -> &Arc<PluginRegistry> {
        &self.plugins
    }
    fn preview(&self) -> &Arc<cctui_server_extras::preview::Registry> {
        &self.preview
    }
    fn presence(&self) -> &Arc<PodIdentity> {
        &self.presence
    }
    fn internal_secret(&self) -> Option<&Arc<str>> {
        self.internal_secret.as_ref()
    }
}
