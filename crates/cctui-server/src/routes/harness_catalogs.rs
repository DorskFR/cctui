//! Model catalogs the daemons report per harness, keyed by machine.
//!
//! Codex keeps its persisted, account-merged cache in `codex_models.rs`;
//! every other harness (the ACP agents) is cached here in memory only, and
//! a daemon re-reports it on each new session, so a server restart loses
//! nothing a session cannot refresh.

use std::sync::LazyLock;

use cctui_proto::codex_catalog::CodexModelCatalog;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use uuid::Uuid;

use crate::state::AppState;

struct Cached {
    catalog: CodexModelCatalog,
    fetched_at: DateTime<Utc>,
}

static CATALOGS: LazyLock<DashMap<(Uuid, String), Cached>> = LazyLock::new(DashMap::new);

/// Record what one machine's harness advertises. An empty catalog is
/// ignored: an agent that could not list its models must not blank the
/// picker.
pub async fn store(state: &AppState, machine_id: Uuid, harness: &str, catalog: CodexModelCatalog) {
    if harness == "codex" {
        crate::routes::codex_models::store_catalog(state, machine_id, catalog).await;
        return;
    }
    if catalog.models.is_empty() {
        tracing::debug!(%machine_id, harness, "ignoring empty model catalog report");
        return;
    }
    tracing::info!(%machine_id, harness, models = catalog.models.len(), "harness model catalog reported");
    CATALOGS.insert((machine_id, harness.to_owned()), Cached { catalog, fetched_at: Utc::now() });
}

/// The catalog a picker for `harness` should be driven by: the named
/// machine's own, else the most recently reported one across machines.
/// `None` when no machine has reported this harness.
#[must_use]
pub fn effective(harness: &str, machine: Option<Uuid>) -> Option<CodexModelCatalog> {
    if let Some(machine) = machine
        && let Some(cached) = CATALOGS.get(&(machine, harness.to_owned()))
    {
        return Some(cached.catalog.clone());
    }
    CATALOGS
        .iter()
        .filter(|entry| entry.key().1 == harness)
        .max_by_key(|entry| entry.fetched_at)
        .map(|entry| entry.catalog.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::codex_catalog::CodexModel;

    fn catalog(ids: &[&str]) -> CodexModelCatalog {
        CodexModelCatalog {
            models: ids
                .iter()
                .map(|id| CodexModel {
                    id: (*id).to_owned(),
                    model: (*id).to_owned(),
                    display_name: id.to_uppercase(),
                    description: String::new(),
                    hidden: false,
                    is_default: false,
                    supported_efforts: vec![],
                    default_effort: String::new(),
                    input_modalities: vec![],
                    upgrade: None,
                    minimal_client_version: None,
                })
                .collect(),
            client_version: None,
        }
    }

    fn insert(machine: Uuid, harness: &str, ids: &[&str], at: DateTime<Utc>) {
        CATALOGS.insert(
            (machine, harness.to_owned()),
            Cached { catalog: catalog(ids), fetched_at: at },
        );
    }

    #[test]
    fn the_named_machine_wins_else_the_freshest_report() {
        let harness = format!("test-harness-{}", Uuid::new_v4());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let t0 = Utc::now();
        insert(a, &harness, &["old"], t0 - chrono::Duration::hours(1));
        insert(b, &harness, &["fresh"], t0);
        assert_eq!(effective(&harness, Some(a)).unwrap().models[0].id, "old");
        assert_eq!(effective(&harness, None).unwrap().models[0].id, "fresh");
        assert_eq!(effective(&harness, Some(Uuid::new_v4())).unwrap().models[0].id, "fresh");
        assert!(effective("nobody-reported-this", None).is_none());
    }
}
