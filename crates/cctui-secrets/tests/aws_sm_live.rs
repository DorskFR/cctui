#![cfg(feature = "aws-sm")]

use cctui_secrets::{Engines, RefResolver as _, SecretError, SecretRef};

#[tokio::test]
#[ignore = "calls real AWS: needs credentials and AWS_REGION in the environment"]
async fn live_get_secret_value_passes_aws_signature_checks() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let r = SecretRef::parse("aws-sm:cctui-signature-probe/does-not-exist").unwrap();
    match Engines::new().resolve(&r).await {
        Err(SecretError::NotFound { .. }) => {}
        Err(SecretError::Backend(e)) => {
            let msg = format!("{e:#}");
            assert!(msg.contains("AccessDeniedException"), "unexpected failure: {msg}");
        }
        Ok(_) => panic!("a nonexistent secret resolved"),
    }
}
