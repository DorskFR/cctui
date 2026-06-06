//! User-defined named dispatchers persisted in the `dispatchers` table
//! (CCT-235). A stored row carries a `kind` and a type-specific `config` JSON
//! blob; this module owns the typed shapes, the secret-redaction discipline,
//! and the construction of a live [`Dispatcher`] impl from a row — the same
//! trait/registry the global env-configured dispatchers use.
//!
//! Secrets (the http bearer token, etc.) are encrypted in-place with
//! [`crate::crypto`] before the blob is stored, and stripped out of every
//! list/get/notification response. The wire/API form never exposes a stored
//! secret back to the client — a create/update carries the cleartext secret in;
//! reads only ever report whether one is set.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{DispatchError, Dispatcher, http::HttpDispatcher, kube::KubeDispatcherConfig};

/// Sentinel JSON value standing in for an encrypted/withheld secret in API
/// responses. Distinct from `null` (no secret set) so the UI can show
/// "configured" vs "not set" without ever seeing the cleartext.
pub const SECRET_REDACTED: &str = "<redacted>";

/// Typed view over the stored `config` blob, tagged by the row's `kind`.
/// `Deserialize` reads the *decrypted* blob; the secret fields are cleartext at
/// that point. `redacted()` produces the API-safe form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredConfig {
    Http(HttpConfig),
    Kubernetes(KubeConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    pub url: String,
    /// Bearer token forwarded as `Authorization: Bearer …`. Secret — encrypted
    /// at rest, never echoed back to the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeConfig {
    pub namespace: String,
    pub source_cronjob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cctui_url: Option<String>,
}

impl StoredConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            StoredConfig::Http(_) => "http",
            StoredConfig::Kubernetes(_) => "kubernetes",
        }
    }

    /// Encrypt secret fields in-place with the vault key, yielding the form that
    /// is safe to persist in `config` JSONB.
    pub fn encrypt_secrets(&mut self, key: &[u8]) {
        if let StoredConfig::Http(c) = self
            && let Some(tok) = &c.token
        {
            c.token = Some(crate::crypto::obfuscate(tok, key));
        }
    }

    /// Decrypt secret fields in-place (inverse of [`Self::encrypt_secrets`]).
    /// A field that fails to decrypt is dropped (treated as unset) rather than
    /// leaking ciphertext into a live dispatcher.
    pub fn decrypt_secrets(&mut self, key: &[u8]) {
        if let StoredConfig::Http(c) = self
            && let Some(enc) = &c.token
        {
            c.token = crate::crypto::deobfuscate(enc, key);
        }
    }

    /// API-safe JSON: secret fields collapse to a redaction sentinel when set,
    /// or are omitted when unset. Operates on the *encrypted-at-rest* form (we
    /// never decrypt for a read), so this is safe to call on a raw DB blob.
    pub fn redacted_json(&self) -> serde_json::Value {
        match self {
            StoredConfig::Http(c) => serde_json::json!({
                "url": c.url,
                "token": c.token.as_ref().map(|_| SECRET_REDACTED),
            }),
            StoredConfig::Kubernetes(c) => serde_json::json!({
                "namespace": c.namespace,
                "source_cronjob": c.source_cronjob,
                "cctui_url": c.cctui_url,
            }),
        }
    }

    /// Construct a live [`Dispatcher`] for this definition. `id` is the
    /// per-request dispatcher id surfaced to the caller (we use the row's
    /// `name`). Expects secrets already decrypted (call [`Self::decrypt_secrets`]
    /// first). Kubernetes construction does a live connect probe and may fail
    /// off-cluster.
    pub async fn build(&self, id: &str) -> Result<Arc<dyn Dispatcher>, DispatchError> {
        match self {
            StoredConfig::Http(c) => {
                Ok(Arc::new(HttpDispatcher::new(id.to_owned(), c.url.clone(), c.token.clone())))
            }
            StoredConfig::Kubernetes(c) => {
                let cfg = KubeDispatcherConfig {
                    id: id.to_owned(),
                    namespace: c.namespace.clone(),
                    source_cronjob: c.source_cronjob.clone(),
                    cctui_url: c.cctui_url.clone(),
                };
                let d = super::kube::KubeDispatcher::try_new(&cfg)
                    .await
                    .map_err(|e| DispatchError::Backend(format!("kube dispatcher init: {e}")))?;
                Ok(Arc::new(d))
            }
        }
    }
}
