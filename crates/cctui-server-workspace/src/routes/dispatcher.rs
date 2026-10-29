//! Shared dispatcher transport with the full host's replica-presence adapter.
use crate::state::AppState;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response;
use cctui_server_core::targets::dispatcher::DispatcherContext;
pub use cctui_server_core::targets::dispatcher::{auth, enroll};

impl DispatcherContext for AppState {
    fn dispatcher_core(&self) -> &cctui_server_core::state::CoreState {
        self
    }
    async fn register_dispatcher_presence(&self, id: uuid::Uuid) {
        crate::presence::register(self, crate::presence::Kind::Dispatcher, id).await;
    }
    async fn unregister_dispatcher_presence(&self, id: uuid::Uuid) {
        crate::presence::unregister(self, crate::presence::Kind::Dispatcher, id).await;
    }
}
pub async fn ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<response::Response, StatusCode> {
    cctui_server_core::targets::dispatcher::ws(ws, State(state), headers).await
}
