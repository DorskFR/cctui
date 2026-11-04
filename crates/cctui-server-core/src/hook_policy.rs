//! Account hook rules attached to a launch, and the record of what it carried.

use cctui_proto::adapter::SessionSpec;
use cctui_proto::hook_policy::{
    HookPolicy, HookPolicyStatus, HookRule, SessionHookPolicy, normalize_rules,
    supports_hook_policy,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Rules of every account the launch key holds a live token for.
///
/// # Errors
/// The lookup failed: the caller must not launch without the rules.
pub async fn account_rules(
    pool: &PgPool,
    session_id: &str,
) -> Result<Vec<HookRule>, sqlx::Error> {
    let stored: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT DISTINCT ON (p.account_id) p.hooks \
         FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN account_tool_policies p ON p.account_id = ap.account_id \
         WHERE st.session_id = $1 AND st.revoked_at IS NULL \
         ORDER BY p.account_id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(merge_rules(stored))
}

fn merge_rules(stored: Vec<serde_json::Value>) -> Vec<HookRule> {
    let mut rules = Vec::new();
    for value in stored {
        match serde_json::from_value::<Vec<HookRule>>(value) {
            Ok(more) => rules.extend(more),
            Err(error) => tracing::warn!(%error, "unreadable account hook rules skipped"),
        }
    }
    normalize_rules(rules).unwrap_or_else(|error| {
        tracing::warn!(%error, "account hook rules over the limits; launching without them");
        Vec::new()
    })
}

#[must_use]
pub fn attach(spec: &mut SessionSpec, rules: Vec<HookRule>) -> Option<SessionHookPolicy> {
    let mut all = spec.hook_policy.take().map(|p| p.rules).unwrap_or_default();
    all.extend(rules);
    let all = normalize_rules(all).unwrap_or_default();
    if all.is_empty() {
        return None;
    }
    let status = if supports_hook_policy(spec.adapter_id.as_str()) {
        HookPolicyStatus::Applied
    } else {
        HookPolicyStatus::Unsupported
    };
    spec.hook_policy = Some(HookPolicy { rules: all.clone() });
    Some(SessionHookPolicy { rules: all, status: Some(status) })
}

/// # Errors
/// The launch row could not be updated.
pub async fn record(
    pool: &PgPool,
    command: Uuid,
    carried: Option<&SessionHookPolicy>,
) -> Result<(), sqlx::Error> {
    let Some(carried) = carried else { return Ok(()) };
    sqlx::query("UPDATE machine_launches SET hook_policy = $2 WHERE command_id = $1")
        .bind(command)
        .bind(sqlx::types::Json(carried))
        .execute(pool)
        .await?;
    Ok(())
}

/// The policy the session's newest launch carried; empty when none did.
///
/// # Errors
/// The lookup failed.
pub async fn for_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<SessionHookPolicy, sqlx::Error> {
    let stored: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT hook_policy FROM machine_launches \
         WHERE (session_id = $1 OR registered_session_id = $1) AND hook_policy IS NOT NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(stored.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::adapter::AdapterId;
    use cctui_proto::hook_policy::{HookEvent, ToolClass};

    fn spec(adapter: &str) -> SessionSpec {
        SessionSpec {
            adapter_id: AdapterId::new(adapter),
            working_dir: None,
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            service_tier: None,
            env: std::collections::BTreeMap::new(),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
            hook_policy: None,
        }
    }

    fn deny_shell() -> HookRule {
        HookRule {
            event: HookEvent::PreToolUse,
            tool: ToolClass::Shell,
            command: "deny".into(),
            timeout_secs: None,
        }
    }

    #[test]
    fn no_rules_leave_the_spec_untouched_and_record_nothing() {
        let mut s = spec("claude-code");
        assert_eq!(attach(&mut s, Vec::new()), None);
        assert_eq!(s.hook_policy, None);
    }

    #[test]
    fn a_claude_launch_carries_and_records_the_rules_as_applied() {
        let mut s = spec("claude-code");
        let carried = attach(&mut s, vec![deny_shell(), deny_shell()]).unwrap();
        assert_eq!(s.hook_policy, Some(HookPolicy { rules: vec![deny_shell()] }));
        assert_eq!(carried.rules, vec![deny_shell()]);
        assert_eq!(carried.status, Some(HookPolicyStatus::Applied));
    }

    #[test]
    fn a_harness_without_a_hook_seam_is_recorded_unsupported() {
        let mut s = spec("codex");
        let carried = attach(&mut s, vec![deny_shell()]).unwrap();
        assert_eq!(carried.status, Some(HookPolicyStatus::Unsupported));
    }

    #[test]
    fn unreadable_stored_rules_are_skipped_not_fatal() {
        let stored = vec![
            serde_json::json!("garbage"),
            serde_json::json!([{"event": "PreToolUse", "tool": "shell", "command": "deny"}]),
        ];
        assert_eq!(merge_rules(stored), vec![deny_shell()]);
    }

    #[tokio::test]
    async fn the_launch_record_round_trips_and_resolves_by_registered_id() {
        crate::test_support::with_isolated_database("hook_policy_launch_record", |pool| async move {
            crate::schema::bootstrap(&pool).await.unwrap();
            let (user, machine, command) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
            let key = command.to_string();
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'fixture', 'kh-u')")
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) \
                 VALUES ($1, $2, 'fixture', 'kh-m')",
            )
            .bind(machine)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO machine_launches \
                     (command_id, session_id, machine_id, user_id, acting_user_id, adapter_id, \
                      working_dir) \
                 VALUES ($1, $2, $3, $4, $4, 'codex', '/w')",
            )
            .bind(command)
            .bind(&key)
            .bind(machine)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
            let carried = attach(&mut spec("codex"), vec![deny_shell()]).unwrap();
            record(&pool, command, Some(&carried)).await.unwrap();
            let real = format!("thread-{command}");
            sqlx::query(
                "UPDATE machine_launches SET registered_session_id = $2 WHERE command_id = $1",
            )
            .bind(command)
            .bind(&real)
            .execute(&pool)
            .await
            .unwrap();
            assert_eq!(for_session(&pool, &real).await.unwrap(), carried);
            assert_eq!(for_session(&pool, &key).await.unwrap(), carried);
            assert_eq!(for_session(&pool, "nobody").await.unwrap(), SessionHookPolicy::default());
        })
        .await;
    }
}
