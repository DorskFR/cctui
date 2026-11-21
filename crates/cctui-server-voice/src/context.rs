use std::future::Future;

use cctui_server_kernel::auth::AuthContext;
use cctui_server_kernel::bus::BusError;
use cctui_server_kernel::context::KernelContext;
use cctui_server_kernel::error::AppError;
use uuid::Uuid;

use crate::speech::{SpeechClient, SpeechConfig};

/// The session and settings paths voice shares with the rest of the host.
pub trait VoiceContext: KernelContext {
    fn authorize_reply(
        &self,
        ctx: &AuthContext,
        session_id: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
    fn dispatch_reply(
        &self,
        session_id: &str,
        content: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        turn_id: Option<Uuid>,
    ) -> impl Future<Output = (Uuid, Result<(), BusError>)> + Send;
    fn dispatch_interrupt(
        &self,
        session_id: &str,
        user_id: Uuid,
    ) -> impl Future<Output = Result<Uuid, AppError>> + Send;
    fn set_daemon_text_watch(
        &self,
        session_id: &str,
        watch: bool,
    ) -> impl Future<Output = ()> + Send;
    fn read_speech(
        &self,
    ) -> impl Future<Output = Result<(SpeechConfig, Option<String>), AppError>> + Send;
    fn speech_client(&self) -> impl Future<Output = Result<SpeechClient, AppError>> + Send;
}
