//! Which accounts a running session may be rebound to, and in what order.
//!
//! The server only rebinds inside a provider family, so a picker row is only
//! offered when its credential's family matches the binding's.

use cctui_proto::provider::{ProviderFamily, provider_family};

const fn family_label(family: ProviderFamily) -> &'static str {
    match family {
        ProviderFamily::Anthropic => "anthropic",
        ProviderFamily::Openai => "openai",
        ProviderFamily::Fireworks => "fireworks",
    }
}

/// A credential at or above this utilization is treated as spent: it is still
/// listed, but never recommended and always sorts last.
pub const LIMITED_PCT: f64 = 95.0;

/// One quota window of a credential, reduced to what the ordering reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub pct: f64,
    /// Seconds until the window resets, `None` when upstream did not say.
    pub resets_in_secs: Option<i64>,
}

/// A provider credential the caller owns, as `GET /accounts/usage` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    /// The identity id, which is what the switch body posts alongside `family`.
    pub account_id: String,
    pub account_name: String,
    pub provider: String,
    pub windows: Vec<Window>,
}

/// The binding being rebound, from `GET /sessions/{id}/bindings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub family: String,
    pub account_id: String,
    pub account_name: String,
}

/// One picker row.
#[derive(Debug, Clone, PartialEq)]
pub struct Option_ {
    pub account_id: String,
    pub account_name: String,
    pub provider: String,
    /// Highest utilization across the credential's windows; `None` when it
    /// reports no windows at all (distinct from a measured zero).
    pub pct: Option<f64>,
    /// Reset of the window `pct` came from.
    pub resets_in_secs: Option<i64>,
    /// The binding's current account — listed, but switching to it is a no-op.
    pub current: bool,
    pub limited: bool,
}

/// The credential's worst window: an unknown utilization cannot be "fine".
fn worst(windows: &[Window]) -> (Option<f64>, Option<i64>) {
    windows
        .iter()
        .max_by(|a, b| a.pct.total_cmp(&b.pct))
        .map_or((None, None), |w| (Some(w.pct), w.resets_in_secs))
}

/// The rows to offer for `binding`, already ordered.
#[must_use]
pub fn switch_options(
    binding: &Binding,
    credentials: &[Credential],
    limited_pct: f64,
) -> Vec<Option_> {
    let family = &binding.family;
    let mut rows: Vec<Option_> = credentials
        .iter()
        .filter(|c| family_label(provider_family(&c.provider)) == family)
        .map(|c| {
            let (pct, resets_in_secs) = worst(&c.windows);
            Option_ {
                account_id: c.account_id.clone(),
                account_name: c.account_name.clone(),
                provider: c.provider.clone(),
                pct,
                resets_in_secs,
                current: c.account_id == binding.account_id,
                limited: pct.is_some_and(|p| p >= limited_pct),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.current
            .cmp(&b.current)
            .reverse()
            .then(a.limited.cmp(&b.limited))
            .then(a.pct.unwrap_or(f64::MAX).total_cmp(&b.pct.unwrap_or(f64::MAX)))
            .then(a.resets_in_secs.unwrap_or(i64::MAX).cmp(&b.resets_in_secs.unwrap_or(i64::MAX)))
            .then_with(|| a.account_name.cmp(&b.account_name))
    });
    rows
}

/// The row the picker opens on: the best switchable candidate, else the
/// first row that is not the current account, else nothing to switch to.
#[must_use]
pub fn recommended(options: &[Option_]) -> std::option::Option<usize> {
    options
        .iter()
        .position(|o| !o.current && !o.limited)
        .or_else(|| options.iter().position(|o| !o.current))
}

#[cfg(test)]
mod tests {
    use super::{Binding, Credential, LIMITED_PCT, Option_, Window, recommended, switch_options};

    fn cred(name: &str, provider: &str, pct: f64, resets: i64) -> Credential {
        Credential {
            account_id: format!("{name}-id"),
            account_name: name.to_owned(),
            provider: provider.to_owned(),
            windows: vec![Window { pct, resets_in_secs: Some(resets) }],
        }
    }

    fn binding(account: &str, family: &str) -> Binding {
        Binding {
            family: family.to_owned(),
            account_id: format!("{account}-id"),
            account_name: account.to_owned(),
        }
    }

    fn names(rows: &[Option_]) -> Vec<&str> {
        rows.iter().map(|r| r.account_name.as_str()).collect()
    }

    #[test]
    fn the_ticket_mockup_is_the_ordering() {
        let creds = vec![
            cred("bob", "anthropic", 99.0, 60),
            cred("alice", "anthropic", 91.0, 300),
            cred("carol", "anthropic", 12.0, 300),
        ];
        let rows = switch_options(&binding("alice", "anthropic"), &creds, LIMITED_PCT);
        assert_eq!(names(&rows), vec!["alice", "carol", "bob"]);
        assert!(rows[0].current);
        assert!(rows[2].limited);
        assert_eq!(recommended(&rows), Some(1), "carol is the recommendation");
    }

    #[test]
    fn only_credentials_in_the_bindings_family_are_offered() {
        let creds = vec![
            cred("alice", "anthropic", 10.0, 60),
            cred("oai", "openai", 1.0, 60),
            cred("oaic", "openai-compatible", 2.0, 60),
            cred("fw", "fireworks", 3.0, 60),
            cred("compat", "anthropic-compatible", 4.0, 60),
        ];
        assert_eq!(
            names(&switch_options(&binding("alice", "anthropic"), &creds, LIMITED_PCT)),
            vec!["alice", "compat"]
        );
        assert_eq!(
            names(&switch_options(&binding("oai", "openai"), &creds, LIMITED_PCT)),
            vec!["oai", "oaic"]
        );
        assert_eq!(
            names(&switch_options(&binding("fw", "fireworks"), &creds, LIMITED_PCT)),
            vec!["fw"]
        );
    }

    #[test]
    fn equal_utilization_breaks_on_the_soonest_reset() {
        let creds = vec![
            cred("me", "anthropic", 80.0, 10),
            cred("late", "anthropic", 20.0, 9_000),
            cred("soon", "anthropic", 20.0, 60),
        ];
        let rows = switch_options(&binding("me", "anthropic"), &creds, LIMITED_PCT);
        assert_eq!(names(&rows), vec!["me", "soon", "late"]);
    }

    #[test]
    fn a_credential_reporting_no_windows_sorts_last_and_is_never_limited() {
        let creds = vec![
            Credential {
                account_id: "quiet-id".to_owned(),
                account_name: "quiet".to_owned(),
                provider: "anthropic".to_owned(),
                windows: Vec::new(),
            },
            cred("me", "anthropic", 50.0, 60),
            cred("busy", "anthropic", 70.0, 60),
        ];
        let rows = switch_options(&binding("me", "anthropic"), &creds, LIMITED_PCT);
        assert_eq!(names(&rows), vec!["me", "busy", "quiet"]);
        assert_eq!(rows[2].pct, None);
        assert!(!rows[2].limited);
        assert_eq!(recommended(&rows), Some(1));
    }

    #[test]
    fn the_worst_window_decides_not_the_first() {
        let creds = vec![
            Credential {
                account_id: "mixed-id".to_owned(),
                account_name: "mixed".to_owned(),
                provider: "anthropic".to_owned(),
                windows: vec![
                    Window { pct: 3.0, resets_in_secs: Some(10) },
                    Window { pct: 97.0, resets_in_secs: Some(9_000) },
                ],
            },
            cred("me", "anthropic", 50.0, 60),
        ];
        let rows = switch_options(&binding("me", "anthropic"), &creds, LIMITED_PCT);
        assert!(rows[1].limited, "a spent weekly window makes the credential spent");
        assert_eq!(rows[1].resets_in_secs, Some(9_000));
    }

    #[test]
    fn with_everything_spent_the_recommendation_is_still_a_switch() {
        let creds = vec![cred("me", "anthropic", 99.0, 60), cred("other", "anthropic", 99.0, 60)];
        let rows = switch_options(&binding("me", "anthropic"), &creds, LIMITED_PCT);
        assert_eq!(recommended(&rows), Some(1));
    }

    #[test]
    fn with_nowhere_to_go_there_is_no_recommendation() {
        let creds = vec![cred("me", "anthropic", 99.0, 60)];
        let rows = switch_options(&binding("me", "anthropic"), &creds, LIMITED_PCT);
        assert_eq!(recommended(&rows), None);
    }
}
