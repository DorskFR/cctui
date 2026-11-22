use cctui_server_voice::context::VoiceContext;
use cctui_server_voice::speech::{SpeechClient, SpeechConfig};
use uuid::Uuid;

use crate::state::AppState;
use cctui_server_kernel::auth::AuthContext;
use cctui_server_kernel::bus::BusError;
use cctui_server_kernel::error::AppError;

impl VoiceContext for AppState {
    async fn authorize_reply(&self, ctx: &AuthContext, session_id: &str) -> Result<(), AppError> {
        cctui_server_sessions::routes::sessions::authorize_reply(self, ctx, session_id).await
    }

    async fn dispatch_reply(
        &self,
        session_id: &str,
        content: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        turn_id: Option<Uuid>,
    ) -> (Uuid, Result<(), BusError>) {
        cctui_server_sessions::ws::dispatch_reply(self, session_id, content, ask_picks, turn_id)
            .await
    }

    async fn dispatch_interrupt(&self, session_id: &str, user_id: Uuid) -> Result<Uuid, AppError> {
        cctui_server_sessions::routes::sessions::dispatch_interrupt(self, session_id, user_id).await
    }

    async fn set_daemon_text_watch(&self, session_id: &str, watch: bool) {
        cctui_server_sessions::ws::set_daemon_text_watch(self, session_id, watch).await;
    }

    async fn read_speech(&self) -> Result<(SpeechConfig, Option<String>), AppError> {
        crate::routes::server_settings::read_speech(&self.pool).await
    }

    async fn speech_client(&self) -> Result<SpeechClient, AppError> {
        crate::routes::server_settings::speech_client(self).await
    }
}
