//! The cctui client: typed REST over the shared route table, plus a
//! reconnecting websocket stream.
//!
//! Every URL is built from `cctui_proto::api::routes::ROUTES`; the crate
//! contains no path literals, so a renamed server route is a compile-or-test
//! failure rather than a 404 at runtime.

pub mod device_auth;
pub mod error;
pub mod rest;
pub mod ws;

use std::sync::Arc;

pub use cctui_proto::api::machine_resources::MachineResourcesRow;
pub use error::ClientError;
pub use rest::{
    Client, ConversationFetch, ConversationRow, Dispatcher, EnrollDispatcher, EnrolledDispatcher,
    FileRead, FileRefusal, LinkedFileOwner, Page, PendingPermissionItem, UpdateDispatcher,
    UploadFile,
};
pub use ws::state::{Ack, AckHandle, AckRegistry, Health, SubscriptionState, Watchdog, backoff};
pub use ws::transport::{Connector, Frame, HttpConnector, StreamTransport, Transport};
pub use ws::{ACK_TIMEOUT, Incoming, WsClient, decode_frame};

impl Client {
    /// A websocket connector for this client's server and credential.
    #[must_use]
    pub fn ws_connector(&self) -> Arc<dyn Connector> {
        Arc::new(HttpConnector::new(self.ws_url(), self.token()))
    }

    /// Starts the reconnecting event stream for this server.
    #[must_use]
    pub fn connect_ws(&self) -> (WsClient, tokio::sync::mpsc::Receiver<Incoming>) {
        WsClient::start(self.ws_connector())
    }
}
