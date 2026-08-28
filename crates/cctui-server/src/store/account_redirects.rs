use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// One live redirect rule. Exactly one of `to_account` / `to_model` is set
/// (enforced by `account_redirects_one_target`): a rule either moves new
/// sessions to another account or flips the model they spawn with — never both.
#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AccountRedirect {
    #[ts(type = "string")]
    pub id: Uuid,
    #[ts(type = "string")]
    pub user_id: Uuid,
    #[ts(type = "string")]
    pub from_account: Uuid,
    #[ts(type = "string | null")]
    pub to_account: Option<Uuid>,
    pub family: String,
    pub match_model: Option<String>,
    pub to_model: Option<String>,
    #[ts(type = "string | null")]
    pub expires_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, user_id, from_account, to_account, family, match_model, to_model, \
                    expires_at, reason, created_at";

/// Every unexpired rule a user holds. Expired rows stay in the table for
/// history; every read path filters them out here.
pub async fn live_for_user(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<Vec<AccountRedirect>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM account_redirects \
         WHERE user_id = $1 AND (expires_at IS NULL OR expires_at > now()) \
         ORDER BY created_at DESC"
    )))
    .bind(user_id)
    .fetch_all(exec)
    .await
}

/// Every unexpired rule that can move a launch by `user_id`: rules they
/// authored plus rules authored by the owner of the rule's `from_account` — an
/// owner's rule follows the shared account to every grantee launching on it.
/// The launcher's own rules sort first so they win when both authors hold a
/// rule for the same `(from_account, family)`.
pub async fn live_for_launch(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<Vec<AccountRedirect>, sqlx::Error> {
    let cols = COLS.split(", ").map(|c| format!("r.{c}")).collect::<Vec<_>>().join(", ");
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {cols} FROM account_redirects r \
         JOIN accounts a ON a.id = r.from_account \
         WHERE (r.user_id = $1 OR r.user_id = a.user_id) \
           AND (r.expires_at IS NULL OR r.expires_at > now()) \
         ORDER BY (r.user_id = $1) DESC, r.created_at DESC"
    )))
    .bind(user_id)
    .fetch_all(exec)
    .await
}

/// Insert parameters for [`upsert`]; the XOR of `to_account`/`to_model` is
/// enforced by the table's CHECK, not here.
pub struct NewRedirect<'a> {
    pub user_id: Uuid,
    pub from_account: Uuid,
    pub to_account: Option<Uuid>,
    pub family: &'a str,
    pub match_model: Option<&'a str>,
    pub to_model: Option<&'a str>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reason: Option<&'a str>,
}

pub async fn upsert(
    exec: impl PgExecutor<'_>,
    r: NewRedirect<'_>,
) -> Result<AccountRedirect, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO account_redirects \
           (user_id, from_account, to_account, family, match_model, to_model, expires_at, reason) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         ON CONFLICT (user_id, from_account, family, COALESCE(match_model, '')) \
         DO UPDATE SET to_account = $3, to_model = $6, expires_at = $7, reason = $8, \
                       created_at = now() \
         RETURNING {COLS}"
    )))
    .bind(r.user_id)
    .bind(r.from_account)
    .bind(r.to_account)
    .bind(r.family)
    .bind(r.match_model)
    .bind(r.to_model)
    .bind(r.expires_at)
    .bind(r.reason)
    .fetch_one(exec)
    .await
}

/// Delete a rule by id. `owner` scopes the delete to that user's rules; `None`
/// (the admin token) deletes regardless of owner. Returns whether a row went
/// away.
pub async fn delete(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    owner: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM account_redirects WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Every live rule regardless of owner — the admin token's list view.
pub async fn live_all(exec: impl PgExecutor<'_>) -> Result<Vec<AccountRedirect>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM account_redirects \
         WHERE expires_at IS NULL OR expires_at > now() \
         ORDER BY created_at DESC"
    )))
    .fetch_all(exec)
    .await
}

/// Follow account-level rules (`to_account` set, `match_model` NULL) from
/// `from` transitively. The visited set ends a cycle at its last new hop and
/// the cap bounds pathological chains; both degrade to "stop here", never an
/// error — a broken rule must not fail a launch. `None` when no rule moves
/// `from` at all.
pub fn follow_account_chain(rules: &[AccountRedirect], from: Uuid, family: &str) -> Option<Uuid> {
    let mut visited = vec![from];
    let mut cur = from;
    for _ in 0..4 {
        let Some(rule) = rules
            .iter()
            .find(|r| r.from_account == cur && r.family == family && r.to_account.is_some())
        else {
            break;
        };
        let next = rule.to_account.expect("filtered on to_account.is_some()");
        if visited.contains(&next) {
            break;
        }
        visited.push(next);
        cur = next;
    }
    (cur != from).then_some(cur)
}

/// The model a session on `account` should spawn with instead of `requested`.
/// A rule matching the exact model beats a catch-all (`match_model` NULL).
pub fn model_flip<'r>(
    rules: &'r [AccountRedirect],
    account: Uuid,
    family: &str,
    requested: &str,
) -> Option<&'r str> {
    let candidates = || {
        rules
            .iter()
            .filter(|r| r.from_account == account && r.family == family && r.to_model.is_some())
    };
    candidates()
        .find(|r| r.match_model.as_deref() == Some(requested))
        .or_else(|| candidates().find(|r| r.match_model.is_none()))
        .and_then(|r| r.to_model.as_deref())
        .filter(|m| *m != requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        from: Uuid,
        to: Option<Uuid>,
        family: &str,
        matches: Option<&str>,
        model: Option<&str>,
    ) -> AccountRedirect {
        AccountRedirect {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            from_account: from,
            to_account: to,
            family: family.to_owned(),
            match_model: matches.map(str::to_owned),
            to_model: model.map(str::to_owned),
            expires_at: None,
            reason: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn no_rules_no_redirect() {
        assert_eq!(follow_account_chain(&[], Uuid::new_v4(), "anthropic"), None);
    }

    #[test]
    fn single_hop() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let rules = [rule(a, Some(b), "anthropic", None, None)];
        assert_eq!(follow_account_chain(&rules, a, "anthropic"), Some(b));
    }

    #[test]
    fn family_scoped() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let rules = [rule(a, Some(b), "anthropic", None, None)];
        assert_eq!(follow_account_chain(&rules, a, "openai"), None);
    }

    #[test]
    fn transitive_two_hops() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rules =
            [rule(a, Some(b), "anthropic", None, None), rule(b, Some(c), "anthropic", None, None)];
        assert_eq!(follow_account_chain(&rules, a, "anthropic"), Some(c));
    }

    #[test]
    fn cycle_stops_at_last_new_hop() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let rules =
            [rule(a, Some(b), "anthropic", None, None), rule(b, Some(a), "anthropic", None, None)];
        assert_eq!(follow_account_chain(&rules, a, "anthropic"), Some(b));
    }

    #[test]
    fn depth_capped() {
        let ids: Vec<Uuid> = (0..8).map(|_| Uuid::new_v4()).collect();
        let rules: Vec<AccountRedirect> =
            ids.windows(2).map(|w| rule(w[0], Some(w[1]), "anthropic", None, None)).collect();
        assert_eq!(follow_account_chain(&rules, ids[0], "anthropic"), Some(ids[4]));
    }

    #[test]
    fn model_rules_do_not_move_accounts() {
        let a = Uuid::new_v4();
        let rules = [rule(a, None, "anthropic", Some("fable"), Some("opus"))];
        assert_eq!(follow_account_chain(&rules, a, "anthropic"), None);
    }

    #[test]
    fn model_flip_exact_match_beats_catch_all() {
        let a = Uuid::new_v4();
        let rules = [
            rule(a, None, "anthropic", None, Some("sonnet")),
            rule(a, None, "anthropic", Some("fable"), Some("opus")),
        ];
        assert_eq!(model_flip(&rules, a, "anthropic", "fable"), Some("opus"));
        assert_eq!(model_flip(&rules, a, "anthropic", "haiku"), Some("sonnet"));
    }

    #[test]
    fn model_flip_misses() {
        let a = Uuid::new_v4();
        let rules = [rule(a, None, "anthropic", Some("fable"), Some("opus"))];
        assert_eq!(model_flip(&rules, a, "anthropic", "opus"), None);
        assert_eq!(model_flip(&rules, a, "openai", "fable"), None);
        assert_eq!(model_flip(&rules, Uuid::new_v4(), "anthropic", "fable"), None);
    }

    #[test]
    fn model_flip_to_same_model_is_a_noop() {
        let a = Uuid::new_v4();
        let rules = [rule(a, None, "anthropic", None, Some("opus"))];
        assert_eq!(model_flip(&rules, a, "anthropic", "opus"), None);
    }
}
