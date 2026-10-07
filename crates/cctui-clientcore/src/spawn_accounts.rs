//! Which account or pool may back a spawn, and on which harness.
//!
//! Port of the web UI's `spawn/options.ts`.
//!
//! The provider families mirror the server's `Family`: offering an account the
//! server will refuse is the bug this logic exists to prevent.
//!
//! The picker's value space — the sentinels and the pool prefix — belongs to
//! [`crate::spawn`], which is where the rule that reads it lives.

use crate::spawn::{NO_ACCOUNT, pool_name};

use cctui_proto::adapter::KNOWN_ADAPTERS;

/// The harness cards, in table order.
pub const ALL_ADAPTERS: [&str; KNOWN_ADAPTERS.len()] = KNOWN_ADAPTERS;

/// The harness a provider credential runs: the first harness-table row of the
/// provider's family. The table covers every family, so this never misses.
#[must_use]
pub fn adapter_for_provider(provider: &str) -> &'static str {
    cctui_proto::adapter::harness_for_provider(provider).unwrap_or(KNOWN_ADAPTERS[0])
}

/// The harnesses an account's credentials can run, in stable order.
#[must_use]
pub fn account_adapters<S: AsRef<str>>(providers: &[S]) -> Vec<&'static str> {
    KNOWN_ADAPTERS
        .into_iter()
        .filter(|adapter| providers.iter().any(|p| adapter_for_provider(p.as_ref()) == *adapter))
        .collect()
}

/// Whether the account can back this harness. No account always can: Auto and
/// the machine login run anything.
#[must_use]
pub fn account_backs_adapter<S: AsRef<str>>(providers: Option<&[S]>, adapter: &str) -> bool {
    providers.is_none_or(|p| account_adapters(p).contains(&adapter))
}

/// The harness in effect: the user's pick when no account is chosen or the
/// account offers it, else the first harness the account can run.
///
/// This is what the dialog *shows*; what it submits is gated by
/// [`account_backs_adapter`] instead, so a mismatch is an error rather than a
/// silently swapped harness.
#[must_use]
pub fn effective_adapter_for<S: AsRef<str>>(providers: Option<&[S]>, adapter: &str) -> String {
    let Some(providers) = providers else { return adapter.to_owned() };
    let allowed = account_adapters(providers);
    if allowed.contains(&adapter) {
        return adapter.to_owned();
    }
    allowed.first().map_or_else(|| adapter.to_owned(), |first| (*first).to_owned())
}

/// Whether a pick names an account that is gone, so the form should fall back
/// to Auto. Auto, the no-account sentinel and pools are never stale: none of
/// them is matched against account names.
#[must_use]
pub fn stale_account_pick<S: AsRef<str>>(value: &str, account_names: &[S]) -> bool {
    !value.is_empty()
        && value != NO_ACCOUNT
        && pool_name(value).is_none()
        && !account_names.iter().any(|n| n.as_ref() == value)
}

/// One pool's membership as the filter sees it.
pub struct PoolMembers<'a> {
    /// Accounts in the pool, by the key `accounts` is indexed on.
    pub members: Vec<&'a str>,
}

/// Indices of the pools that can back this harness.
///
/// A pool is dropped only when every member it can see lacks the harness's
/// family — the server would refuse each of them. An empty pool, or one with a
/// member owned by somebody else, is given the benefit of the doubt.
#[must_use]
pub fn compatible_pools<S: AsRef<str>>(
    pools: &[PoolMembers<'_>],
    account_providers: &dyn Fn(&str) -> Option<Vec<S>>,
    harness: &str,
) -> Vec<usize> {
    pools
        .iter()
        .enumerate()
        .filter(|(_, pool)| {
            pool.members.is_empty()
                || pool.members.iter().any(|member| {
                    account_providers(member).is_none_or(|providers| {
                        account_backs_adapter(Some(providers.as_slice()), harness)
                    })
                })
        })
        .map(|(i, _)| i)
        .collect()
}

/// The credential backing a harness on this account.
#[must_use]
pub fn provider_for_adapter<'a, S: AsRef<str>>(
    providers: &'a [S],
    adapter: &str,
) -> Option<&'a str> {
    providers.iter().map(AsRef::as_ref).find(|provider| adapter_for_provider(provider) == adapter)
}

/// One usage window as the headline percentage reads it.
pub struct UsageWindow {
    pub key: String,
    pub utilization: f64,
}

/// One headline percentage for a credential: its session window, else the
/// fullest window it reported, clamped to 0..=100.
#[must_use]
pub fn headline_pct(windows: &[UsageWindow]) -> Option<u32> {
    let session = windows.iter().find(|w| w.key == "session");
    let worst = session.or_else(|| {
        windows
            .iter()
            .filter(|w| w.utilization.is_finite())
            .max_by(|a, b| a.utilization.total_cmp(&b.utilization))
    })?;
    if !worst.utilization.is_finite() {
        return None;
    }
    Some(worst.utilization.round().clamp(0.0, 100.0) as u32)
}

/// Env keys are shell variable names; the server rejects anything else.
#[must_use]
pub fn env_key_valid(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_uppercase() || first == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn providers(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn env_keys_follow_the_shell_variable_shape() {
        for ok in ["A", "_", "GH_TOKEN", "A1", "_A_1"] {
            assert!(env_key_valid(ok), "{ok} should be valid");
        }
        for bad in ["", "1A", "a", "gh_token", "GH-TOKEN", "GH TOKEN", "GH.TOKEN"] {
            assert!(!env_key_valid(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn an_account_with_no_providers_backs_nothing_offerable() {
        let none: Vec<String> = Vec::new();
        assert!(account_adapters(&none).is_empty());
        assert!(!account_backs_adapter(Some(none.as_slice()), "claude-code"));
        assert!(account_backs_adapter(None::<&[String]>, "claude-code"));
    }

    #[test]
    fn the_first_family_is_the_fallback_harness() {
        let p = providers(&["openai-compatible"]);
        assert_eq!(effective_adapter_for(Some(p.as_slice()), "claude-code"), "codex");
        assert_eq!(effective_adapter_for(None::<&[String]>, "codex"), "codex");
    }

    #[test]
    fn the_provider_backing_a_harness_is_the_one_in_its_family() {
        let p = providers(&["anthropic", "openai"]);
        assert_eq!(provider_for_adapter(&p, "claude-code"), Some("anthropic"));
        assert_eq!(provider_for_adapter(&p, "codex"), Some("openai"));
        assert_eq!(provider_for_adapter(&p, "opencode"), None);
    }

    #[test]
    fn a_pool_of_members_we_cannot_see_is_kept() {
        let pools = [PoolMembers { members: vec!["unknown"] }, PoolMembers { members: vec![] }];
        let lookup = |_: &str| -> Option<Vec<String>> { None };
        assert_eq!(compatible_pools(&pools, &lookup, "claude-code"), [0, 1]);
    }
}
