//! Share-boundary settings and environment policy, enforced by core.

#[cfg(test)]
mod tests {
    use cctui_server_core::launch_settings::{SHARED_ENV_ALLOWLIST, SHARED_SETTINGS_ALLOWLIST};

    /// The allowlist must name real, per-account-settable keys: a typo or a key
    /// the catalog has since retagged `managed` would silently widen or narrow
    /// the share boundary.
    #[test]
    fn every_allowlisted_settings_key_is_exposable_in_some_catalog() {
        for name in SHARED_SETTINGS_ALLOWLIST {
            let claude = super::super::catalog().key(name);
            let codex = super::super::codex::catalog().key(name);
            let found = claude.or(codex).unwrap_or_else(|| {
                panic!("`{name}` is in neither settings catalog — stale share allowlist")
            });
            assert!(
                found.account_exposable(),
                "`{name}` is tagged `{}` and is not settable per-account at all",
                found.tag.as_str()
            );
        }
    }

    #[test]
    fn no_allowlisted_env_name_is_denylisted() {
        for name in SHARED_ENV_ALLOWLIST {
            assert!(super::super::valid_env_name(name), "`{name}` is not a valid env var name");
            assert!(
                !super::super::catalog().env_denylisted(name),
                "`{name}` is gateway-managed and must not be share-allowlisted"
            );
        }
    }
}
