//! Dispatcher heartbeat transitions and lifecycle audit.
use crate::events::{self, Actor, Event, kind};
use crate::state::CoreState;
use cctui_proto::models::MachineLiveness;
use uuid::Uuid;

/// Record `tier` for an enrolled `dispatcher_id` and broadcast a
/// [`cctui_proto::ws::ServerEvent::DispatcherLiveness`] iff it changed. Peer of
/// the machine lifecycle service.
pub fn record_and_broadcast_dispatcher(
    state: &CoreState,
    dispatcher_id: Uuid,
    tier: MachineLiveness,
) {
    let changed =
        state.dispatcher_liveness.insert(dispatcher_id, tier).is_none_or(|prev| prev != tier);
    if changed {
        tracing::info!(%dispatcher_id, ?tier, "dispatcher liveness changed");
        state.bus.publish_server(cctui_proto::ws::ServerEvent::DispatcherLiveness {
            dispatcher_id,
            liveness: tier,
        });
        let logged = match tier {
            MachineLiveness::Online => {
                Some((kind::SYSTEM_DISPATCHER_ONLINE, events::Severity::Info))
            }
            MachineLiveness::Offline => {
                Some((kind::SYSTEM_DISPATCHER_OFFLINE, events::Severity::Warn))
            }
            MachineLiveness::Stale => None,
        };
        if let Some((event_kind, severity)) = logged {
            events::record(
                &state.pool,
                &state.bus,
                Event::new(event_kind, Actor::System)
                    .severity(severity)
                    .detail(serde_json::json!({ "dispatcher_id": dispatcher_id })),
            );
        }
    }
}
