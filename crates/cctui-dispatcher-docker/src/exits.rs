//! `AutoRemove` deletes a container as it dies, so how it ended is kept from
//! Docker's event stream for status reads that find nothing left to inspect.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bollard::Docker;
use bollard::models::EventMessage;
use bollard::system::EventsOptions;
use cctui_dispatcher_core::HandleState;
use futures_util::StreamExt;

/// Matches the server's oldest probed handle.
const KEEP: Duration = Duration::from_secs(48 * 3600);
const RESUBSCRIBE: Duration = Duration::from_secs(5);

#[must_use]
pub fn verdict(
    oom: bool,
    exit_code: i64,
    memory_limit: Option<i64>,
) -> (HandleState, Option<String>) {
    if oom {
        return (HandleState::Failed, Some(oom_reason(memory_limit)));
    }
    if exit_code == 0 {
        (HandleState::Complete, None)
    } else {
        (HandleState::Failed, Some(format!("container exited with code {exit_code}")))
    }
}

fn oom_reason(memory_limit: Option<i64>) -> String {
    memory_limit.filter(|l| *l > 0).map_or_else(
        || "OOMKilled".to_owned(),
        |l| format!("OOMKilled (memory limit {})", quantity(l)),
    )
}

/// Kubernetes-style, so both dispatchers name a limit alike.
fn quantity(bytes: i64) -> String {
    for (unit, size) in [("Gi", 1_i64 << 30), ("Mi", 1 << 20), ("Ki", 1 << 10)] {
        if bytes % size == 0 {
            return format!("{}{unit}", bytes / size);
        }
    }
    bytes.to_string()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    oom: bool,
    code: Option<i64>,
}

impl Exit {
    #[must_use]
    pub fn state(self) -> (HandleState, Option<String>) {
        match self.code {
            Some(code) => verdict(self.oom, code, None),
            None if self.oom => verdict(true, 137, None),
            None => (HandleState::Gone, None),
        }
    }
}

#[derive(Default)]
pub struct Exits {
    seen: Mutex<HashMap<String, (Exit, Instant)>>,
}

impl Exits {
    pub fn record(&self, event: &EventMessage) {
        let Some(attributes) = event.actor.as_ref().and_then(|a| a.attributes.as_ref()) else {
            return;
        };
        let Some(name) = attributes.get("name") else { return };
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        seen.retain(|_, (_, at)| at.elapsed() < KEEP);
        if event.action.as_deref() == Some("start") {
            seen.remove(name);
            return;
        }
        let (exit, at) = seen.entry(name.clone()).or_insert((Exit::default(), Instant::now()));
        *at = Instant::now();
        match event.action.as_deref() {
            Some("oom") => exit.oom = true,
            Some("die") => exit.code = attributes.get("exitCode").and_then(|c| c.parse().ok()),
            _ => {}
        }
    }

    #[must_use]
    pub fn ended(&self, name: &str) -> Option<Exit> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).get(name).map(|(exit, _)| *exit)
    }
}

pub fn watch(docker: Docker, exits: Arc<Exits>, origin_label: String) {
    tokio::spawn(async move {
        loop {
            let options = EventsOptions::<String> {
                filters: HashMap::from([
                    ("type".to_owned(), vec!["container".to_owned()]),
                    ("event".to_owned(), ["start", "oom", "die"].map(str::to_owned).to_vec()),
                    ("label".to_owned(), vec![origin_label.clone()]),
                ]),
                ..Default::default()
            };
            let mut events = std::pin::pin!(docker.events(Some(options)));
            while let Some(event) = events.next().await {
                match event {
                    Ok(event) => exits.record(&event),
                    Err(err) => {
                        tracing::warn!(%err, "docker event stream failed");
                        break;
                    }
                }
            }
            tokio::time::sleep(RESUBSCRIBE).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use bollard::models::EventActor;

    use super::*;

    fn event(action: &str, name: &str, exit_code: Option<&str>) -> EventMessage {
        let mut attributes = HashMap::from([("name".to_owned(), name.to_owned())]);
        if let Some(code) = exit_code {
            attributes.insert("exitCode".to_owned(), code.to_owned());
        }
        EventMessage {
            action: Some(action.to_owned()),
            actor: Some(EventActor { id: None, attributes: Some(attributes) }),
            ..Default::default()
        }
    }

    #[test]
    fn an_oom_killed_container_fails_naming_its_memory_limit() {
        let oom = |limit| verdict(true, 137, limit).1;
        assert_eq!(oom(Some(8 << 30)).as_deref(), Some("OOMKilled (memory limit 8Gi)"));
        assert_eq!(oom(Some(512 << 20)).as_deref(), Some("OOMKilled (memory limit 512Mi)"));
        assert_eq!(oom(Some(1000)).as_deref(), Some("OOMKilled (memory limit 1000)"));
        assert_eq!(oom(Some(0)).as_deref(), Some("OOMKilled"), "0 means unlimited");
        assert_eq!(oom(None).as_deref(), Some("OOMKilled"));
        assert_eq!(verdict(true, 0, None).0, HandleState::Failed, "an OOM kill is never clean");
        assert_eq!(verdict(false, 0, None), (HandleState::Complete, None));
        assert_eq!(
            verdict(false, 1, None),
            (HandleState::Failed, Some("container exited with code 1".to_owned()))
        );
    }

    #[test]
    fn an_auto_removed_containers_oom_kill_outlives_its_removal() {
        let exits = Exits::default();
        exits.record(&event("start", "w-oom", None));
        exits.record(&event("oom", "w-oom", None));
        exits.record(&event("die", "w-oom", Some("137")));
        exits.record(&event("die", "w-clean", Some("0")));
        exits.record(&event("die", "w-error", Some("2")));
        let state = |name| exits.ended(name).map(Exit::state);
        assert_eq!(state("w-oom"), Some((HandleState::Failed, Some("OOMKilled".to_owned()))));
        assert_eq!(state("w-clean"), Some((HandleState::Complete, None)));
        assert_eq!(
            state("w-error"),
            Some((HandleState::Failed, Some("container exited with code 2".to_owned())))
        );
        assert_eq!(state("never-seen"), None);
        exits.record(&event("oom", "w-dying", None));
        assert_eq!(state("w-dying").map(|s| s.0), Some(HandleState::Failed));
        exits.record(&event("start", "w-oom", None));
        assert_eq!(state("w-oom"), None, "a restart under the same name forgets the old end");
    }
}
