//! Calls the extras code makes into other domains; the host forwards each
//! method to the owner's function of the same name.
use std::future::Future;

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::adapter::RemoveInitiator;
use cctui_proto::api::ApiError;
use cctui_proto::api::auto_followup::SendFollowup;
use cctui_proto::thread::{ThreadConversationPage, ThreadCursor};
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

/// The blob store, for plugin archives.
pub trait BlobsHooks: Send + Sync + 'static {
    fn store_blob(
        &self,
        pool: &PgPool,
        bytes: &[u8],
        media_type: Option<&str>,
    ) -> impl Future<Output = Result<StoredBlob, sqlx::Error>> + Send;
    fn read_blob_in(
        &self,
        conn: &mut PgConnection,
        hash: &str,
    ) -> impl Future<Output = sqlx::Result<Option<Vec<u8>>>> + Send;
    fn delete_unreferenced(&self, pool: &PgPool, hash: &str) -> impl Future<Output = ()> + Send;
}

/// Sessions: transcripts, follow-ups, archiving and machine-key auth.
pub trait SessionsHooks: Send + Sync + 'static {
    fn renderable_rows(
        &self,
        pool: &PgPool,
        session_id: &str,
        adapter_id: &str,
        params: &ConversationQuery,
    ) -> impl Future<Output = Result<Vec<RenderableRow>, sqlx::Error>> + Send;
    fn archive_one(
        &self,
        session_id: &str,
        force: bool,
        initiator: RemoveInitiator,
        actor: Actor,
    ) -> impl Future<Output = Result<ArchiveOutcome, sqlx::Error>> + Send;
    fn machine_user(
        &self,
        headers: &HeaderMap,
    ) -> impl Future<Output = Result<Uuid, (StatusCode, Json<ApiError>)>> + Send;
    /// `auto_followup::route`.
    fn route(
        &self,
        caller: Option<&AuthContext>,
        session_id: &str,
        followup: SendFollowup,
        text: &str,
        message_key: Option<&str>,
    ) -> impl Future<Output = Result<Option<Followed>, AppError>> + Send;
    fn refuse_while_pending(
        &self,
        pool: &PgPool,
        session_id: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
    /// `followup::thread::conversation`.
    fn conversation(
        &self,
        reader: Reader<'_>,
        id: &str,
        cursor: Option<ThreadCursor>,
        limit: i64,
    ) -> impl Future<Output = Result<Option<ThreadConversationPage>, AppError>> + Send;
    /// `transcript_md::render`.
    fn render(
        &self,
        header: &Header,
        events: &[(i64, Value)],
        roles: &[String],
        budget: usize,
    ) -> Rendered;
    /// `transcript_md::render_thread`.
    fn render_thread(
        &self,
        page: &ThreadConversationPage,
        roles: &[String],
        budget: usize,
    ) -> RenderedThread;
}
