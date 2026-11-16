use crate::config::Config;
use crate::notify::{self, Channels, NewNotification, Routing};

/// `priority` is ntfy's 1..=5 scale.
pub struct Notification {
    pub title: String,
    pub message: String,
    pub tags: String,
    pub priority: u8,
}

/// Never blocks the caller; a no-op unless ntfy is configured.
pub fn notify(config: &Config, n: Notification) {
    let routing = Routing::from_config(config);
    if !routing.ntfy {
        return;
    }
    let Some(pool) = notify::installed_pool() else {
        tracing::warn!("ntfy ping dropped: notification outbox not started");
        return;
    };
    let channels = Channels::from_config(config);
    tokio::spawn(async move {
        let Some(owner) = notify::instance_owner(&pool).await else { return };
        notify::emit_now(&pool, routing, &channels, legacy(owner, n)).await;
    });
}

fn legacy(owner: uuid::Uuid, n: Notification) -> NewNotification {
    NewNotification {
        severity: notify::store::severity_for_priority(n.priority),
        body: Some(n.message),
        tags: Some(n.tags),
        ..NewNotification::new(owner, notify::kind::LEGACY, n.title)
    }
}

#[cfg(test)]
mod tests {
    use super::{Notification, legacy};

    #[test]
    fn a_legacy_ping_keeps_its_title_tags_and_priority() {
        let owner = uuid::Uuid::new_v4();
        let n = legacy(
            owner,
            Notification {
                title: "Daemon crashloop: m1".into(),
                message: "x".repeat(400),
                tags: "rotating_light".into(),
                priority: 4,
            },
        );
        assert_eq!((n.user_id, n.kind), (owner, "legacy"));
        assert_eq!(n.title, "Daemon crashloop: m1");
        assert_eq!(n.tags.as_deref(), Some("rotating_light"));
        assert_eq!(crate::notify::store::priority_for_severity(n.severity), 4);
        assert_eq!(n.collapse_key, None);
    }

    #[test]
    fn without_ntfy_config_nothing_is_stored_or_sent() {
        let config = crate::config::Config::for_test(vec![]);
        assert!(!crate::notify::Routing::from_config(&config).ntfy);
        let n = Notification {
            title: "t".into(),
            message: "m".into(),
            tags: String::new(),
            priority: 3,
        };
        super::notify(&config, n);
    }
}
