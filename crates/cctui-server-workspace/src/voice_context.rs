use cctui_server_voice::context::VoiceContext;
use cctui_server_voice::speech::{SpeechClient, SpeechConfig};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::bus::BusError;
use crate::error::AppError;
use crate::state::AppState;

impl VoiceContext for AppState {
    async fn authorize_reply(&self, ctx: &AuthContext, session_id: &str) -> Result<(), AppError> {
        crate::routes::sessions::authorize_reply(self, ctx, session_id).await
    }

    async fn dispatch_reply(
        &self,
        session_id: &str,
        content: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        turn_id: Option<Uuid>,
    ) -> (Uuid, Result<(), BusError>) {
        crate::ws::dispatch_reply(self, session_id, content, ask_picks, turn_id).await
    }

    async fn dispatch_interrupt(&self, session_id: &str, user_id: Uuid) -> Result<Uuid, AppError> {
        crate::routes::sessions::dispatch_interrupt(self, session_id, user_id).await
    }

    async fn set_daemon_text_watch(&self, session_id: &str, watch: bool) {
        crate::ws::set_daemon_text_watch(self, session_id, watch).await;
    }

    async fn read_speech(&self) -> Result<(SpeechConfig, Option<String>), AppError> {
        crate::routes::server_settings::read_speech(&self.pool).await
    }

    async fn speech_client(&self) -> Result<SpeechClient, AppError> {
        crate::routes::server_settings::speech_client(self).await
    }
}
