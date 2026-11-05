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

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const KIND_REFS: [&str; 4] =
        ["file:/run/secrets/gh", "k8s:dev/creds#gh", "bao:kv/data/creds#gh", "aws-sm:creds/gh"];

    /// Serves one value for a ref of every kind; `None` is a backend outage.
    /// The engines themselves are tested in `cctui-secrets`.
    struct Rotating(Mutex<Option<String>>);

    #[async_trait::async_trait]
    impl RefResolver for Rotating {
        async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError> {
            let value = self.0.lock().unwrap().clone();
            value
                .map(Credential::new)
                .ok_or_else(|| SecretError::Backend(anyhow::anyhow!("{r}: backend down")))
        }
    }

    struct Shared(Arc<Rotating>);

    #[async_trait::async_trait]
    impl RefResolver for Shared {
        async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError> {
            self.0.resolve(r).await
        }
    }

    fn source(value: &str) -> (Arc<Rotating>, Arc<SecretSource>) {
        let backend = Arc::new(Rotating(Mutex::new(Some(value.to_owned()))));
        let source = SecretSource::new(Box::new(Shared(backend.clone())), Duration::from_hours(1));
        (backend, Arc::new(source))
    }

    fn refs() -> Vec<SecretRef> {
        KIND_REFS.iter().map(|r| SecretRef::parse(r).unwrap()).collect()
    }

    async fn all_equal(source: &SecretSource, want: &str) -> bool {
        for r in refs() {
            if source.fetch(&r).await.unwrap().expose() != want {
                return false;
            }
        }
        true
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sighup_refreshes_cached_secrets_of_every_kind_before_the_ttl() {
        let (backend, source) = source("gh-v1");
        assert!(all_equal(&source, "gh-v1").await);
        *backend.0.lock().unwrap() = Some("gh-v2".to_owned());
        assert!(all_equal(&source, "gh-v1").await, "cached until the TTL or a SIGHUP");

        let task = source.spawn_sighup_refresh();
        let status = std::process::Command::new("kill")
            .args(["-HUP", &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !all_equal(&source, "gh-v2").await {
            assert!(tokio::time::Instant::now() < deadline, "SIGHUP never refreshed the cache");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        task.abort();
    }

    #[derive(Clone, Default)]
    struct LogBuf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuf {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_refresh_keeps_serving_the_old_secret_and_logs_only_the_ref() {
        let (backend, source) = source("gh-old-value");
        assert!(all_equal(&source, "gh-old-value").await);
        *backend.0.lock().unwrap() = None;

        let buf = LogBuf::default();
        let writer = buf.clone();
        let subscriber =
            tracing_subscriber::fmt().with_writer(move || writer.clone()).with_ansi(false).finish();
        let report = {
            let _guard = tracing::subscriber::set_default(subscriber);
            source.refresh_all().await
        };
        assert_eq!(report.failed, KIND_REFS.len());
        assert!(all_equal(&source, "gh-old-value").await);
        let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        for r in refs() {
            assert!(logs.contains(&r.to_string()), "{logs}");
        }
        assert!(!logs.contains("gh-old-value"), "{logs}");

        *backend.0.lock().unwrap() = Some("gh-new-value".to_owned());
        assert_eq!(source.refresh_all().await.changed, KIND_REFS.len());
        assert!(all_equal(&source, "gh-new-value").await);
    }

    #[test]
    fn templated_refs_parse_through_the_shared_crate() {
        let rendered = render_identity("bao:kv/data/w#GPG_KEY_${IDENTITY}", "acme").unwrap();
        let parsed = SecretRef::parse(&rendered).unwrap();
        assert_eq!(parsed.to_string(), "vault:kv/data/w#GPG_KEY_ACME");
    }
}
