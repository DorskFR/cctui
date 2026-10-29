//! Execution session addressing, safe peer envelopes and turn delivery.
use cctui_proto::adapter::AdapterCommand;
use cctui_proto::injected::PEER_MESSAGE;
use uuid::Uuid;

/// The session columns the policy and the peer surfaces both need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNode {
    pub id: String,
    pub user_id: Option<Uuid>,
    pub parent_id: Option<String>,
    pub machine_uuid: Option<Uuid>,
    pub machine_name: Option<String>,
    pub adapter_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    /// The one room this session is in, if any.
    pub room_id: Option<Uuid>,
}

/// `live` / `ended` / `archived`, the three states the roster reports.
/// An idle-but-revivable session is `live`: a message wakes it.
#[must_use]
pub fn state_of(status: Option<&str>) -> &'static str {
    match status {
        Some("archived") => "archived",
        Some("ended" | "failed") => "ended",
        _ => "live",
    }
}

impl SessionNode {
    #[must_use]
    pub fn state(&self) -> &'static str {
        state_of(self.status.as_deref())
    }

    /// Whether a message can still be delivered into this session.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.state() == "live"
    }

    /// `name (adapter on machine)` — the sender label the envelope carries.
    #[must_use]
    pub fn label(&self) -> String {
        let name =
            self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&self.id);
        let adapter = self.adapter_id.as_deref().unwrap_or("unknown");
        let machine = self.machine_name.as_deref().unwrap_or("unknown machine");
        format!("{name} ({adapter} on {machine})")
    }
}

/// `$1` is the session id.
pub const NODE_SQL: &str = "\
SELECT s.id, s.user_id, s.parent_id, s.machine_uuid, m.name, s.adapter_id, s.session_name, \
       s.status, s.room_id \
  FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
 WHERE s.id = $1";

/// Columns of [`NODE_SQL`], in order.
pub type NodeRow = (
    String,
    Option<Uuid>,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Uuid>,
);

impl From<NodeRow> for SessionNode {
    fn from(r: NodeRow) -> Self {
        let (id, user_id, parent_id, machine_uuid, machine_name, adapter_id, name, status, room_id) =
            r;
        Self {
            id,
            user_id,
            parent_id,
            machine_uuid,
            machine_name,
            adapter_id,
            name,
            status,
            room_id,
        }
    }
}

pub async fn load_node(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Option<SessionNode>, sqlx::Error> {
    let row: Option<NodeRow> =
        sqlx::query_as(NODE_SQL).bind(session_id).fetch_optional(pool).await?;
    Ok(row.map(SessionNode::from))
}
/// A markdown-attribute-safe rendering of a label.
fn attr(raw: &str) -> String {
    raw.replace(['"', '<', '>'], "")
}

/// Wrap a peer turn in the cctui message envelope.
///
/// The webui and transcript readers recognize the same wrapper.
#[must_use]
pub fn envelope(sender: &SessionNode, body: &str) -> String {
    format!(
        "<{PEER_MESSAGE} from=\"{}\" from-name=\"{}\">\n{}\n</{PEER_MESSAGE}>",
        attr(&sender.id),
        attr(&sender.label()),
        body.trim(),
    )
}

/// What one delivery attempt did. `Offline` covers every reason the bus could not
/// place the turn: no daemon for the machine, no adapter, a closed channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    Archived,
    Ended,
    Offline,
}

impl Delivery {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Archived => "archived",
            Self::Ended => "ended",
            Self::Offline => "offline",
        }
    }
}

pub async fn deliver(
    state: &crate::state::CoreState,
    target_id: &str,
    target_state: &str,
    text: String,
) -> Delivery {
    match target_state {
        "archived" => return Delivery::Archived,
        "ended" => return Delivery::Ended,
        _ => {}
    }
    match crate::bus::dispatch_session(
        &state.pool,
        &state.bus,
        target_id,
        AdapterCommand::SendMessage { local_id: target_id.to_owned(), text },
    )
    .await
    {
        Ok(()) => Delivery::Delivered,
        Err(err) => {
            tracing::info!(target = %target_id, %err, "peer turn not delivered");
            Delivery::Offline
        }
    }
}
