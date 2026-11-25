/// `CCTUI_PREVIEW_HOST` pattern split around its `{id}` placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewHost {
    prefix: String,
    suffix: String,
}

impl PreviewHost {
    pub fn parse(pattern: &str) -> anyhow::Result<Self> {
        let pattern = pattern.trim().to_ascii_lowercase();
        let Some((prefix, suffix)) = pattern.split_once("{id}") else {
            anyhow::bail!("CCTUI_PREVIEW_HOST must contain one `{{id}}` placeholder");
        };
        if suffix.contains("{id}") {
            anyhow::bail!("CCTUI_PREVIEW_HOST must contain `{{id}}` exactly once");
        }
        if suffix.is_empty() {
            anyhow::bail!("CCTUI_PREVIEW_HOST needs a suffix after `{{id}}` (a domain)");
        }
        if prefix.contains('/') || suffix.contains('/') || pattern.contains(':') {
            anyhow::bail!(
                "CCTUI_PREVIEW_HOST is a bare host pattern, without scheme or port; the port is \
                 taken from CCTUI_EXTERNAL_URL"
            );
        }
        Ok(Self { prefix: prefix.to_owned(), suffix: suffix.to_owned() })
    }

    /// The preview id embedded in a `Host` header value, ignoring any port.
    #[must_use]
    pub fn id_from_host(&self, host: &str) -> Option<String> {
        let host = host.trim().to_ascii_lowercase();
        let host = host.rsplit_once(':').map_or(host.as_str(), |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) { h } else { host.as_str() }
        });
        let id = host.strip_prefix(&self.prefix)?.strip_suffix(&self.suffix)?;
        (!id.is_empty() && is_preview_id(id)).then(|| id.to_owned())
    }

    #[must_use]
    pub fn host_for(&self, id: &str) -> String {
        format!("{}{id}{}", self.prefix, self.suffix)
    }
}

#[must_use]
pub fn is_preview_id(id: &str) -> bool {
    id.len() >= 16 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}
