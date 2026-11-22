use cctui_proto::adapter::AdapterId;
use cctui_proto::api::DaemonAdapterConfig;
use uuid::Uuid;

use crate::context::DaemonContext;

/// The per-session transcript high-water marks for `machine_id`,
///
/// handed to the daemon right after Reconcile so it resumes its tail from the
/// server's stored offset instead of replaying from zero. Only sessions with a
/// non-zero mark are returned.
pub async fn load_resume_marks(
    state: &impl DaemonContext,
    machine_id: Uuid,
) -> sqlx::Result<Vec<(String, u64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, transcript_offset FROM sessions \
         WHERE machine_uuid = $1 AND transcript_offset > 0",
    )
    .bind(machine_id)
    .fetch_all(state.daemon_pool())
    .await?;
    Ok(rows.into_iter().map(|(id, off)| (id, u64::try_from(off).unwrap_or(0))).collect())
}

/// Among the claude job `shorts` a daemon reports on disk, the sessions this
///
/// machine has archived. The daemon removes their jobs so `claude agents`
/// converges on the archive state.
pub async fn archived_jobs(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    shorts: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    cctui_telemetry::db::traced(ARCHIVED_JOBS, |sql| {
        sqlx::query_scalar(sql).bind(machine_id).bind(shorts).fetch_all(pool)
    })
    .await
}

/// Matches `idx_sessions_archived_claude_short` column for column, predicate for
/// predicate, so each reported prefix is one index probe.
pub const ARCHIVED_JOBS: &str = "SELECT id FROM sessions \
     WHERE machine_uuid = $1 AND status = 'archived' AND adapter_id = 'claude-code' \
       AND left(id, 8) = ANY($2)";

/// Rebuild and deliver current configuration to the machine's live daemon.
pub async fn push_reconcile(
    state: &impl DaemonContext,
    machine_id: Uuid,
) -> Result<(), crate::bus::BusError> {
    let adapters = load_reconcile(state, machine_id)
        .await
        .map_err(|e| crate::bus::BusError::Reconcile(e.to_string()))?;
    let secret_scrub = load_scrub_config(state, machine_id).await;
    state
        .daemon_bus()
        .command_daemon(
            machine_id,
            cctui_proto::ws::DaemonFrameDown::Reconcile { adapters, secret_scrub },
        )
        .await
}

/// Union the machine's `adapters_enabled` rows with the harnesses that run by default.
///
/// A row overrides a default-on harness's config or disables it, and is the
/// only thing that turns a default-off harness on.
fn merge_default_adapters<'a>(
    mut rows: Vec<(String, serde_json::Value, bool)>,
    defaults: impl IntoIterator<Item = &'a str>,
) -> Vec<(String, serde_json::Value, bool)> {
    for id in defaults {
        if !rows.iter().any(|(rid, _, _)| rid == id) {
            rows.push((id.to_owned(), serde_json::json!({}), true));
        }
    }
    rows
}

/// The ids of every harness a machine runs: default-on ones without a
/// disabling row, plus any the machine's rows enable.
pub async fn enabled_adapter_ids(
    state: &impl DaemonContext,
    machine_id: Uuid,
) -> sqlx::Result<Vec<String>> {
    Ok(load_reconcile(state, machine_id)
        .await?
        .into_iter()
        .filter(|a| a.enabled)
        .map(|a| a.adapter_id.0)
        .collect())
}

pub async fn load_reconcile(
    state: &impl DaemonContext,
    machine_id: Uuid,
) -> sqlx::Result<Vec<DaemonAdapterConfig>> {
    let rows: Vec<(String, serde_json::Value, bool)> = cctui_telemetry::db::traced(
        "SELECT adapter_id, config, enabled FROM adapters_enabled WHERE machine_id = $1",
        |sql| sqlx::query_as(sql).bind(machine_id).fetch_all(state.daemon_pool()),
    )
    .await?;
    let rows = merge_default_adapters(rows, cctui_proto::adapter::default_enabled_adapters());

    // Bridge the owning user's `user_settings.data.harnessMode` into each
    // claude-code adapter's `config["mode"]`. The settings blob is
    // otherwise webui-only; this is the one place the server reads it. A
    // machine-level `adapters_enabled.config.mode` (if ever set) wins, so an
    // operator can still pin a machine. Codex rows are untouched.
    let harness_mode: Option<String> = sqlx::query_scalar(
        "SELECT us.data->>'harnessMode' \
         FROM machines m JOIN user_settings us ON us.user_id = m.user_id \
         WHERE m.id = $1",
    )
    .bind(machine_id)
    .fetch_optional(state.daemon_pool())
    .await?
    .flatten();
    let adapter_mode =
        crate::execution_settings::harness_mode_to_adapter_token(harness_mode.as_deref());

    Ok(rows
        .into_iter()
        .map(|(id, mut config, enabled)| {
            if id == "claude-code" {
                // Per-machine pin wins: only inject when the row hasn't already
                // set a mode (any value — `claude-daemon`/`legacy`/bg/etc.).
                let pinned = config.get("mode").and_then(serde_json::Value::as_str).is_some();
                if !pinned {
                    if let Some(obj) = config.as_object_mut() {
                        obj.insert(
                            "mode".to_owned(),
                            serde_json::Value::String(adapter_mode.clone()),
                        );
                    } else {
                        config = serde_json::json!({ "mode": adapter_mode });
                    }
                }
            }
            DaemonAdapterConfig { adapter_id: AdapterId::new(id), config, enabled }
        })
        .collect())
}

/// The effective secret-scrub config for `machine_id`'s owner: the
///
/// `secretScrubEnabled` flag plus the clamped `secretScrubPatterns` list from
/// `user_settings.data`, carried in every Reconcile so a running daemon applies
/// the current list without a restart. Best-effort — a DB error scrubs nothing.
pub async fn load_scrub_config(
    state: &impl DaemonContext,
    machine_id: Uuid,
) -> cctui_proto::ws::SecretScrubConfig {
    let data: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT us.data FROM machines m JOIN user_settings us ON us.user_id = m.user_id \
         WHERE m.id = $1",
    )
    .bind(machine_id)
    .fetch_optional(state.daemon_pool())
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("db error loading scrub config: {e}");
        None
    })
    .flatten();
    data.as_ref().map(crate::execution_settings::secret_scrub_of).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use cctui_proto::ws::DaemonFrameDown;
    use serde_json::json;

    use super::*;

    fn defaults() -> Vec<&'static str> {
        cctui_proto::adapter::default_enabled_adapters()
    }

    #[test]
    fn merge_defaults_every_default_on_adapter() {
        let got = merge_default_adapters(Vec::new(), defaults());
        let mut ids: Vec<&str> = got.iter().map(|(id, _, _)| id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["claude-code", "codex", "opencode"]);
        assert!(got.iter().all(|(_, config, enabled)| *enabled && config == &json!({})));
    }

    #[test]
    fn a_default_off_adapter_is_skipped_without_a_row() {
        let got = merge_default_adapters(Vec::new(), ["claude-code"]);
        assert!(got.iter().any(|(id, _, enabled)| id == "claude-code" && *enabled));
        assert!(!got.iter().any(|(id, _, _)| id == "gemini"));
    }

    #[test]
    fn a_default_off_adapter_is_included_when_a_row_enables_it() {
        let rows = vec![("gemini".to_owned(), json!({"bin": "/opt/gemini"}), true)];
        let got = merge_default_adapters(rows, ["claude-code"]);
        let gemini = got.iter().find(|(id, _, _)| id == "gemini").expect("row kept");
        assert!(gemini.2);
        assert_eq!(gemini.1, json!({"bin": "/opt/gemini"}));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn merge_defaults_keeps_row_overrides() {
        let rows = vec![
            ("opencode".to_owned(), json!({"bin": "/opt/opencode"}), false),
            ("legacy-harness".to_owned(), json!({}), true),
        ];
        let got = merge_default_adapters(rows, defaults());
        assert_eq!(got.len(), 4);
        let opencode = got.iter().find(|(id, _, _)| id == "opencode").unwrap();
        assert_eq!(opencode.1, json!({"bin": "/opt/opencode"}));
        assert!(!opencode.2);
        assert!(got.iter().any(|(id, _, _)| id == "legacy-harness"));
        assert!(got.iter().any(|(id, _, enabled)| id == "claude-code" && *enabled));
        assert!(got.iter().any(|(id, _, enabled)| id == "codex" && *enabled));
    }

    #[test]
    fn resume_marks_frame_carries_stored_offsets() {
        let rows: Vec<(String, u64)> = vec![("sess-a".into(), 4096), ("sess-b".into(), 12)];
        let frame = DaemonFrameDown::ResumeMarks { session_marks: rows.clone(), archived: vec![] };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains(r#""type":"resume_marks""#));
        assert!(!json.contains("archived"), "{json}");
        match serde_json::from_str::<DaemonFrameDown>(&json).unwrap() {
            DaemonFrameDown::ResumeMarks { session_marks, archived } => {
                assert_eq!(session_marks, rows);
                assert!(archived.is_empty());
            }
            _ => panic!("expected ResumeMarks"),
        }
    }
}
