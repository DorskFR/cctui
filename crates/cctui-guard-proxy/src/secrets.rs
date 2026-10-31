//! The proxy's secret engines come from `cctui-secrets`; every secret it can
//! inject is named by an explicit [`SecretRef`]. What stays here is the
//! per-identity templating applied before a ref is parsed.

pub use cctui_secrets::{
    Credential, Engines, K8sClient, RefResolver, SecretError, SecretRef, SecretSource, VaultClient,
};

/// Uppercases ASCII alphanumerics, maps everything else to `_` — the shared
/// identity/service → env-var-fragment convention.
fn sanitize_upper(part: &str) -> String {
    part.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}

/// Substitutes `${IDENTITY}` (sanitized-uppercase) and `${identity}` (verbatim)
/// in a secret-ref template. A template that uses a placeholder while no
/// identity is set is an error — never silently resolve to a wrong path.
#[allow(clippy::literal_string_with_formatting_args)]
pub fn render_identity(template: &str, identity: &str) -> anyhow::Result<String> {
    if !template.contains("${") {
        return Ok(template.to_owned());
    }
    anyhow::ensure!(
        !identity.is_empty(),
        "secret ref {template:?} uses an identity placeholder but no identity is set"
    );
    let out =
        template.replace("${IDENTITY}", &sanitize_upper(identity)).replace("${identity}", identity);
    anyhow::ensure!(!out.contains("${"), "unresolved placeholder in secret ref {template:?}");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_templating() {
        assert_eq!(
            render_identity("vault:c/data/w#GITHUB_TOKEN_${IDENTITY}", "acme").unwrap(),
            "vault:c/data/w#GITHUB_TOKEN_ACME"
        );
        assert_eq!(
            render_identity("vault:c/data/w/${identity}#token", "acme-corp").unwrap(),
            "vault:c/data/w/acme-corp#token"
        );
        assert_eq!(
            render_identity("env:CRED_${IDENTITY}_GH", "acme-corp").unwrap(),
            "env:CRED_ACME_CORP_GH"
        );
        // No placeholder: identity irrelevant, even empty.
        assert_eq!(render_identity("env:STATIC", "").unwrap(), "env:STATIC");
        // Placeholder with no identity must error, not resolve to a wrong path.
        assert!(render_identity("env:CRED_${IDENTITY}_GH", "").is_err());
        // Unknown placeholder must error rather than pass through.
        assert!(render_identity("env:CRED_${TYPO}", "acme").is_err());
    }

    #[test]
    fn templated_refs_parse_through_the_shared_crate() {
        let rendered = render_identity("bao:kv/data/w#GPG_KEY_${IDENTITY}", "acme").unwrap();
        let parsed = SecretRef::parse(&rendered).unwrap();
        assert_eq!(parsed.to_string(), "vault:kv/data/w#GPG_KEY_ACME");
    }
}
