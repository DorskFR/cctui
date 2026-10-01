//! Account-pool membership arithmetic and redirect chips. Must agree with the
//! webui's `accounts/pools.logic.ts` and `queries/accounts.ts`, which
//! `fixtures/parity/accounts.json` enforces on both sides.

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct MemberRef {
    pub account_id: String,
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PoolRef {
    pub id: String,
    pub user_id: String,
    pub members: Vec<MemberRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct AccountRef {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub pool_eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct RedirectRef {
    pub id: String,
    pub from_account: String,
    pub to_account: Option<String>,
    pub family: String,
    pub expires_at: Option<String>,
}

/// One account's redirect rule, its target resolved to a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectChip {
    pub id: String,
    pub family: String,
    pub target_name: String,
    pub until: Option<String>,
}

/// The membership a pool should be `PATCH`ed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipChange {
    pub pool_id: String,
    pub accounts: Vec<String>,
}

/// The pool an account sits in. An account belongs to at most one; the first
/// match wins should the server ever disagree.
#[must_use]
pub fn pool_of<'a>(pools: &'a [PoolRef], account_id: &str) -> Option<&'a PoolRef> {
    pools.iter().find(|p| p.members.iter().any(|m| m.account_id == account_id))
}

/// Members in election order.
#[must_use]
pub fn ordered_members(pool: &PoolRef) -> Vec<String> {
    let mut members: Vec<&MemberRef> = pool.members.iter().collect();
    members.sort_by_key(|m| m.position);
    members.iter().map(|m| m.account_id.clone()).collect()
}

/// Whether an account may join this pool: it must exist, not be a member
/// already, and be the pool owner's own or not withheld from pools.
#[must_use]
pub fn accepts_member(pool: &PoolRef, account_id: &str, accounts: &[AccountRef]) -> bool {
    let Some(account) = accounts.iter().find(|a| a.id == account_id) else { return false };
    if pool.members.iter().any(|m| m.account_id == account_id) {
        return false;
    }
    account.user_id == pool.user_id || account.pool_eligible
}

/// The membership `PATCH`es a move implies: the account leaves its current pool
/// (if any) and joins `to` (`None` ⇒ it only leaves). Order is kept.
#[must_use]
pub fn membership_after_move(
    pools: &[PoolRef],
    account_id: &str,
    to: Option<&str>,
) -> Vec<MembershipChange> {
    let from = pool_of(pools, account_id);
    if from.map(|p| p.id.as_str()) == to {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(from) = from {
        let accounts =
            ordered_members(from).into_iter().filter(|id| id != account_id).collect::<Vec<_>>();
        out.push(MembershipChange { pool_id: from.id.clone(), accounts });
    }
    if let Some(to) = to.and_then(|id| pools.iter().find(|p| p.id == id)) {
        let mut accounts = ordered_members(to);
        accounts.push(account_id.to_owned());
        out.push(MembershipChange { pool_id: to.id.clone(), accounts });
    }
    out
}

/// Redirect rules for one account, `to_account` resolved to a name. A rule
/// without an account target is a model redirect, not one of these, and is
/// dropped.
#[must_use]
pub fn redirect_chips(
    rules: &[RedirectRef],
    accounts: &[AccountRef],
    account_id: &str,
) -> Vec<RedirectChip> {
    rules
        .iter()
        .filter(|r| r.to_account.is_some() && r.from_account == account_id)
        .map(|r| RedirectChip {
            id: r.id.clone(),
            family: r.family.clone(),
            target_name: accounts
                .iter()
                .find(|a| Some(&a.id) == r.to_account.as_ref())
                .map_or_else(|| "…".to_owned(), |a| a.name.clone()),
            until: r.expires_at.clone(),
        })
        .collect()
}
