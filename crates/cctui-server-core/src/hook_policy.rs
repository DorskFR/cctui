//! The hook policy a launch carries (side-door rule, inherited and account
//! rules), and the record of it.

use axum::http::StatusCode;
use cctui_proto::adapter::SessionSpec;
use cctui_proto::hook_policy::{
    AddOnlyError, DISPATCH_ADDITIONS_FIELD, DISPATCH_POLICY_FIELD, HookPolicy, HookPolicyStatus,
    HookRule, SessionHookPolicy, effective_rules, no_direct_harness_rule, normalize_rules,
    supports_hook_policy,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

/// Rules of every account the launch key holds a live token for.
///
/// # Errors
/// The lookup failed: the caller must not launch without the rules.
pub async fn account_rules(pool: &PgPool, session_id: &str) -> Result<Vec<HookRule>, sqlx::Error> {
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

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Rejected(String),
}

impl From<PolicyError> for AppError {
    fn from(error: PolicyError) -> Self {
        match error {
            PolicyError::Db(error) => Self::Db(error),
            PolicyError::Rejected(reason) => Self::new(StatusCode::BAD_REQUEST, reason),
        }
    }
}

impl From<AddOnlyError> for PolicyError {
    fn from(error: AddOnlyError) -> Self {
        Self::Rejected(error.to_string())
    }
}

/// The side-door rule, then `rules`, as the launch of `adapter` carries them.
///
/// # Errors
/// The union is over the limits: the launch must not go out without it.
pub fn carry(adapter: &str, rules: Vec<HookRule>) -> Result<SessionHookPolicy, String> {
    let mut all = vec![no_direct_harness_rule()];
    all.extend(rules);
    let rules = normalize_rules(all)?;
    let status = if supports_hook_policy(adapter) {
        HookPolicyStatus::Applied
    } else {
        HookPolicyStatus::Unsupported
    };
    Ok(SessionHookPolicy { rules, status: Some(status) })
}

/// Add the account rules to what the spec already carries.
///
/// # Errors
/// See [`carry`].
pub fn attach(spec: &mut SessionSpec, rules: Vec<HookRule>) -> Result<SessionHookPolicy, String> {
    let mut all = spec.hook_policy.take().map(|p| p.rules).unwrap_or_default();
    all.extend(rules);
    let carried = carry(spec.adapter_id.as_str(), all)?;
    spec.hook_policy = Some(HookPolicy { rules: carried.rules.clone() });
    Ok(carried)
}

/// # Errors
/// The launch row could not be updated.
pub async fn record(
    pool: &PgPool,
    command: Uuid,
    carried: &SessionHookPolicy,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE machine_launches SET hook_policy = $2 WHERE command_id = $1")
        .bind(command)
        .bind(sqlx::types::Json(carried))
        .execute(pool)
        .await?;
    Ok(())
}

/// Record the policy of a launch that has no `machine_launches` row.
///
/// # Errors
/// The row could not be written.
pub async fn record_for_session(
    pool: &PgPool,
    session_id: &str,
    carried: &SessionHookPolicy,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO session_hook_policies (session_id, policy) VALUES ($1, $2) \
         ON CONFLICT (session_id) DO UPDATE SET policy = EXCLUDED.policy, created_at = now()",
    )
    .bind(session_id)
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
        "SELECT policy FROM ( \
             SELECT hook_policy AS policy, created_at FROM machine_launches \
             WHERE (session_id = $1 OR registered_session_id = $1) AND hook_policy IS NOT NULL \
             UNION ALL \
             SELECT policy, created_at FROM session_hook_policies WHERE session_id = $1 \
         ) launched ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(stored.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

/// What a session spawned from `parent` inherits: the policy the parent was
/// launched with, plus its accounts' rules in case it predates that record.
///
/// # Errors
/// A lookup failed: the caller must not launch without the parent's rules.
pub async fn inherited_rules(
    pool: &PgPool,
    parent_session_id: &str,
) -> Result<Vec<HookRule>, sqlx::Error> {
    let mut rules = vec![no_direct_harness_rule()];
    rules.extend(for_session(pool, parent_session_id).await?.rules);
    rules.extend(account_rules(pool, parent_session_id).await?);
    Ok(rules)
}

/// The parent's rules with the request's additions, for a spawned child.
///
/// # Errors
/// [`PolicyError::Rejected`] when an addition would weaken an inherited rule.
pub async fn for_child(
    pool: &PgPool,
    parent_session_id: &str,
    additions: Vec<HookRule>,
) -> Result<HookPolicy, PolicyError> {
    let inherited = inherited_rules(pool, parent_session_id).await?;
    Ok(HookPolicy { rules: effective_rules(&inherited, additions)? })
}

/// Fill a launch from `parent` that bypasses `launch_durable` (a fork) with
/// the parent's policy, and record it under the child's key.
///
/// # Errors
/// The parent's policy could not be read or recorded.
pub async fn inherit_for_launch(
    pool: &PgPool,
    parent_session_id: &str,
    child_key: &str,
    spec: &mut SessionSpec,
) -> Result<SessionHookPolicy, PolicyError> {
    spec.hook_policy = Some(for_child(pool, parent_session_id, Vec::new()).await?);
    let carried = attach(spec, Vec::new()).map_err(PolicyError::Rejected)?;
    record_for_session(pool, child_key, &carried).await?;
    Ok(carried)
}

/// The caller's `hook_rules` additions in a dispatch payload, already checked
/// against the side-door rule every worker inherits: a dispatch weakening it is
/// refused before anything is minted for the worker.
///
/// # Errors
/// [`PolicyError::Rejected`] for unreadable or weakening additions.
pub fn dispatch_additions(payload: &serde_json::Value) -> Result<Vec<HookRule>, PolicyError> {
    let additions = match payload.get(DISPATCH_ADDITIONS_FIELD) {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(raw) => serde_json::from_value::<Vec<HookRule>>(raw.clone())
            .map_err(|e| PolicyError::Rejected(format!("invalid payload.hook_rules: {e}")))?,
    };
    effective_rules(&[no_direct_harness_rule()], additions.clone())?;
    Ok(additions)
}

/// Replace whatever policy the caller put in a worker payload with the
/// effective one: the worker's account rules plus the caller's
/// `hook_rules` additions, under the same add-only rule as a child spawn.
///
/// # Errors
/// Unreadable or weakening additions, or a failed lookup or record.
pub async fn for_dispatch(
    pool: &PgPool,
    session_id: &str,
    adapter: &str,
    payload: &mut serde_json::Value,
) -> Result<(), PolicyError> {
    let additions = dispatch_additions(payload)?;
    let Some(obj) = payload.as_object_mut() else {
        return Err(PolicyError::Rejected("dispatch payload must be an object".into()));
    };
    obj.remove(DISPATCH_POLICY_FIELD);
    obj.remove(DISPATCH_ADDITIONS_FIELD);
    let mut inherited = vec![no_direct_harness_rule()];
    inherited.extend(account_rules(pool, session_id).await?);
    let rules = effective_rules(&inherited, additions)?;
    let carried = carry(adapter, rules).map_err(PolicyError::Rejected)?;
    record_for_session(pool, session_id, &carried).await?;
    let policy = HookPolicy { rules: carried.rules };
    obj.insert(
        DISPATCH_POLICY_FIELD.to_owned(),
        serde_json::to_value(policy).map_err(|e| PolicyError::Rejected(e.to_string()))?,
    );
    Ok(())
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

    fn shell(command: &str, timeout_secs: Option<u32>) -> HookRule {
        HookRule {
            event: HookEvent::PreToolUse,
            tool: ToolClass::Shell,
            command: command.into(),
            timeout_secs,
        }
    }

    fn deny_shell() -> HookRule {
        shell("deny", None)
    }

    #[test]
    fn a_launch_without_rules_still_carries_the_side_door_rule() {
        let mut s = spec("claude-code");
        let carried = attach(&mut s, Vec::new()).unwrap();
        assert_eq!(carried.rules, vec![no_direct_harness_rule()]);
        assert_eq!(s.hook_policy, Some(HookPolicy { rules: vec![no_direct_harness_rule()] }));
    }

    #[test]
    fn a_claude_launch_carries_and_records_the_rules_as_applied() {
        let mut s = spec("claude-code");
        let carried = attach(&mut s, vec![deny_shell(), deny_shell()]).unwrap();
        let want = vec![no_direct_harness_rule(), deny_shell()];
        assert_eq!(s.hook_policy, Some(HookPolicy { rules: want.clone() }));
        assert_eq!(carried.rules, want);
        assert_eq!(carried.status, Some(HookPolicyStatus::Applied));
    }

    #[test]
    fn inherited_spec_rules_come_before_the_account_rules() {
        let mut s = spec("claude-code");
        s.hook_policy = Some(HookPolicy { rules: vec![shell("parent", None)] });
        let carried = attach(&mut s, vec![shell("account", None)]).unwrap();
        assert_eq!(
            carried.rules,
            vec![no_direct_harness_rule(), shell("parent", None), shell("account", None)]
        );
    }

    #[test]
    fn a_harness_without_a_hook_seam_is_recorded_unsupported() {
        let mut s = spec("opencode");
        let carried = attach(&mut s, vec![deny_shell()]).unwrap();
        assert_eq!(carried.status, Some(HookPolicyStatus::Unsupported));
    }

    #[test]
    fn a_launch_over_the_limits_fails_instead_of_dropping_rules() {
        let many: Vec<HookRule> = (0..cctui_proto::hook_policy::MAX_RULES)
            .map(|i| shell(&format!("r{i}"), None))
            .collect();
        assert!(attach(&mut spec("claude-code"), many).is_err());
    }

    #[test]
    fn dispatch_additions_cannot_weaken_the_side_door_rule() {
        let weaker = HookRule { timeout_secs: Some(1), ..no_direct_harness_rule() };
        let payload = serde_json::json!({ "hook_rules": [weaker] });
        assert!(matches!(dispatch_additions(&payload), Err(PolicyError::Rejected(_))));
        let payload = serde_json::json!({ "hook_rules": [no_direct_harness_rule(), deny_shell()] });
        assert_eq!(dispatch_additions(&payload).unwrap().len(), 2);
        assert!(dispatch_additions(&serde_json::json!({})).unwrap().is_empty());
        assert!(dispatch_additions(&serde_json::json!({ "hook_rules": 7 })).is_err());
    }

    #[test]
    fn unreadable_stored_rules_are_skipped_not_fatal() {
        let stored = vec![
            serde_json::json!("garbage"),
            serde_json::json!([{"event": "PreToolUse", "tool": "shell", "command": "deny"}]),
        ];
        assert_eq!(merge_rules(stored), vec![deny_shell()]);
    }

    async fn fixture(pool: &PgPool) -> (Uuid, Uuid) {
        let (user, machine) = (Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'fixture', 'kh-u')")
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'fixture', 'kh-m')",
        )
        .bind(machine)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
        (user, machine)
    }

    async fn launch(pool: &PgPool, user: Uuid, machine: Uuid, adapter: &str) -> Uuid {
        let command = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO machine_launches \
                 (command_id, session_id, machine_id, user_id, acting_user_id, adapter_id, \
                  working_dir) \
             VALUES ($1, $2, $3, $4, $4, $5, '/w')",
        )
        .bind(command)
        .bind(command.to_string())
        .bind(machine)
        .bind(user)
        .bind(adapter)
        .execute(pool)
        .await
        .unwrap();
        command
    }

    async fn launched_with(
        pool: &PgPool,
        user: Uuid,
        machine: Uuid,
        adapter: &str,
        policy: Option<HookPolicy>,
    ) -> (String, SessionHookPolicy) {
        let command = launch(pool, user, machine, adapter).await;
        let mut s = spec(adapter);
        s.hook_policy = policy;
        let carried = attach(&mut s, Vec::new()).unwrap();
        record(pool, command, &carried).await.unwrap();
        (command.to_string(), carried)
    }

    #[tokio::test]
    async fn the_launch_record_round_trips_and_resolves_by_registered_id() {
        crate::test_support::with_isolated_database(
            "hook_policy_launch_record",
            |pool| async move {
                crate::schema::bootstrap(&pool).await.unwrap();
                let (user, machine) = fixture(&pool).await;
                let command = launch(&pool, user, machine, "codex").await;
                let key = command.to_string();
                let carried = attach(&mut spec("codex"), vec![deny_shell()]).unwrap();
                record(&pool, command, &carried).await.unwrap();
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
                assert_eq!(
                    for_session(&pool, "nobody").await.unwrap(),
                    SessionHookPolicy::default()
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_child_request_omitting_the_parent_rules_still_gets_them() {
        crate::test_support::with_isolated_database("hook_policy_child_omits", |pool| async move {
            crate::schema::bootstrap(&pool).await.unwrap();
            let (user, machine) = fixture(&pool).await;
            let parent_rules = HookPolicy { rules: vec![shell("deny-rm", Some(30))] };
            let (parent, _) =
                launched_with(&pool, user, machine, "claude-code", Some(parent_rules)).await;
            let got = for_child(&pool, &parent, vec![shell("audit", None)]).await.unwrap();
            assert_eq!(
                got.rules,
                vec![no_direct_harness_rule(), shell("deny-rm", Some(30)), shell("audit", None)]
            );
            let bare = for_child(&pool, &parent, Vec::new()).await.unwrap();
            assert!(bare.rules.contains(&shell("deny-rm", Some(30))));
            assert!(bare.rules.contains(&no_direct_harness_rule()));
        })
        .await;
    }

    #[tokio::test]
    async fn a_child_request_weakening_a_parent_rule_is_rejected_with_400() {
        crate::test_support::with_isolated_database(
            "hook_policy_child_weakens",
            |pool| async move {
                crate::schema::bootstrap(&pool).await.unwrap();
                let (user, machine) = fixture(&pool).await;
                let parent_rules = HookPolicy { rules: vec![shell("deny-rm", Some(30))] };
                let (parent, _) =
                    launched_with(&pool, user, machine, "claude-code", Some(parent_rules)).await;
                let side_door = HookRule { timeout_secs: Some(1), ..no_direct_harness_rule() };
                for weaker in [shell("deny-rm", Some(1)), side_door] {
                    let err = for_child(&pool, &parent, vec![weaker]).await.unwrap_err();
                    assert!(matches!(err, PolicyError::Rejected(_)), "{err}");
                    assert_eq!(AppError::from(err).status(), StatusCode::BAD_REQUEST);
                }
            },
        )
        .await;
    }

    #[tokio::test]
    async fn every_generation_keeps_the_root_rules() {
        crate::test_support::with_isolated_database("hook_policy_generations", |pool| async move {
            crate::schema::bootstrap(&pool).await.unwrap();
            let (user, machine) = fixture(&pool).await;
            let root_rules = HookPolicy { rules: vec![shell("deny-rm", Some(30))] };
            let (root, _) =
                launched_with(&pool, user, machine, "claude-code", Some(root_rules)).await;
            let child_policy = for_child(&pool, &root, Vec::new()).await.unwrap();
            let (child, _) = launched_with(&pool, user, machine, "codex", Some(child_policy)).await;
            let grandchild = for_child(&pool, &child, Vec::new()).await.unwrap();
            assert!(grandchild.rules.contains(&shell("deny-rm", Some(30))));
            assert!(grandchild.rules.contains(&no_direct_harness_rule()));
        })
        .await;
    }

    #[tokio::test]
    async fn a_relaunch_from_a_session_reapplies_its_persisted_policy() {
        crate::test_support::with_isolated_database("hook_policy_relaunch", |pool| async move {
            crate::schema::bootstrap(&pool).await.unwrap();
            let (user, machine) = fixture(&pool).await;
            let rules = HookPolicy { rules: vec![shell("deny-rm", Some(30))] };
            let (parent, carried) =
                launched_with(&pool, user, machine, "claude-code", Some(rules)).await;
            let mut fork = spec("claude-code");
            let fork_key = Uuid::new_v4().to_string();
            let forked = inherit_for_launch(&pool, &parent, &fork_key, &mut fork).await.unwrap();
            assert_eq!(forked.rules, carried.rules);
            assert_eq!(fork.hook_policy, Some(HookPolicy { rules: carried.rules.clone() }));
            assert_eq!(for_session(&pool, &fork_key).await.unwrap(), forked);
        })
        .await;
    }

    #[tokio::test]
    async fn a_dispatch_payload_gets_the_effective_policy_and_records_it() {
        crate::test_support::with_isolated_database("hook_policy_dispatch", |pool| async move {
            crate::schema::bootstrap(&pool).await.unwrap();
            let session = Uuid::new_v4().to_string();
            let mut payload = serde_json::json!({
                "prompt": "go",
                "hook_policy": {"rules": []},
                "hook_rules": [{"event": "PreToolUse", "tool": "shell", "command": "audit"}],
            });
            for_dispatch(&pool, &session, "claude-code", &mut payload).await.unwrap();
            assert!(payload.get(DISPATCH_ADDITIONS_FIELD).is_none());
            let sent = cctui_proto::hook_policy::from_dispatch_payload(&payload).unwrap();
            assert_eq!(sent.rules, vec![no_direct_harness_rule(), shell("audit", None)]);
            let stored = for_session(&pool, &session).await.unwrap();
            assert_eq!(stored.rules, sent.rules);
            assert_eq!(stored.status, Some(HookPolicyStatus::Applied));

            let mut bad = serde_json::json!({ "hook_rules": "nope" });
            assert!(matches!(
                for_dispatch(&pool, &session, "claude-code", &mut bad).await,
                Err(PolicyError::Rejected(_))
            ));
        })
        .await;
    }
}
