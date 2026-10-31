//! Shared secret-redaction engine.
//!
//! [`redact_json`] rewrites string
//! leaves only (JSON structure is preserved), masking the matched span with
//! `[REDACTED:<category>]`; high-entropy prefixed categories add a keyed-HMAC
//! correlation suffix `[REDACTED:<category>:9f2a]`. Non-reversible, and
//! idempotent because the `[REDACTED:...]` marker matches no anchored pattern.

mod entropy;

use std::collections::{BTreeMap, BTreeSet};

use hmac::{Hmac, Mac};
use regex::{Regex, RegexSet};
use serde_json::{Map, Value};
use sha2::Sha256;

/// Per-field scan cap.
///
/// String leaves longer than this are left untouched — a
/// guard against a pathological multi-MB blob monopolising the hot path. Real
/// secrets are short and sit well within this window; a legitimate huge tool
/// output that happens to embed a token past the cap is the rare miss the
/// on-demand re-scrub (same cap) also skips, deliberately and consistently.
pub const MAX_FIELD_LEN: usize = 16 * 1024 * 1024;

/// Decides whether the match spanning `start..end` of the haystack is a secret.
type Validator = fn(&str, usize, usize) -> bool;

/// A single detector: a name/category, its regex, the capture group to mask
/// (0 = whole match), and whether it earns a correlation suffix.
struct Compiled {
    category: String,
    re: Regex,
    group: usize,
    high_entropy: bool,
    builtin: bool,
    validate: Option<Validator>,
}

/// The precompiled effective detector set (built-in defaults + enabled user
/// patterns) plus the correlation-suffix key. Build once, reuse per event.
pub struct CompiledPatterns {
    patterns: Vec<Compiled>,
    /// Prefilter over the same patterns, in the same order: one pass narrows a
    /// haystack to the handful of detectors that can match it, instead of
    /// running all ~70 full scans over every string leaf.
    prefilter: Option<RegexSet>,
    key: Vec<u8>,
    allow: BTreeSet<String>,
}

impl CompiledPatterns {
    /// An empty set — redaction is disabled, [`redact_json`] is a no-op.
    #[must_use]
    pub const fn disabled() -> Self {
        Self { patterns: Vec::new(), prefilter: None, key: Vec::new(), allow: BTreeSet::new() }
    }

    /// Values the user vetted as not secret: a match equal to one is kept as is.
    #[must_use]
    pub fn with_allowlist<I: IntoIterator<Item = String>>(mut self, values: I) -> Self {
        self.allow
            .extend(values.into_iter().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()));
        self
    }

    fn masks(&self, c: &Compiled, hay: &str, m: &regex::Match<'_>) -> bool {
        !self.allow.contains(m.as_str()) && c.validate.is_none_or(|v| v(hay, m.start(), m.end()))
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// A built-in detector definition. `group` names the capture group to mask so a
/// URL detector can redact just the password while keeping `scheme://user@host`.
struct Builtin {
    category: &'static str,
    family: &'static str,
    regex: &'static str,
    group: usize,
    high_entropy: bool,
}

use self::Builtin as B;

/// The built-in detector corpus. Patterns are distilled from the gitleaks
/// ruleset (MIT) and restricted to the prefix-anchored subset: a detector must
/// never match an emitted `[REDACTED:…]` placeholder, or the re-scrub sweep
/// would cascade instead of being idempotent. The `corpus_*` tests below
/// enforce that, plus compile / fire on a fixture / no false positive.
const BUILTINS: &[Builtin] = &[
    B {
        category: "github_token",
        family: "forge",
        regex: r"gh[pousr]_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "github_pat",
        family: "forge",
        regex: r"github_pat_[A-Za-z0-9_]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "gitlab_token",
        family: "forge",
        regex: r"gl(?:pat|rt|dt|ft|soat|oas|ptt|cbt|agent|imt|ffct)-[0-9A-Za-z_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "gitlab_runner_token",
        family: "forge",
        regex: r"GR1348941[0-9A-Za-z_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "sourcegraph_token",
        family: "forge",
        regex: r"sgp_(?:[a-fA-F0-9]{16}_|local_)?[a-fA-F0-9]{40}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "harness_key",
        family: "forge",
        regex: r"(?:pat|sat)\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9]{24}\.[A-Za-z0-9]{20}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "octopus_key",
        family: "forge",
        regex: r"API-[A-Z0-9]{26}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "postman_key",
        family: "forge",
        regex: r"PMAK-[a-f0-9]{24}-[a-f0-9]{34}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "heroku_key",
        family: "forge",
        regex: r"HRKU-AA[0-9A-Za-z_-]{58}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "artifactory_key",
        family: "forge",
        regex: r"(?:AKCp[A-Za-z0-9]{69}|cmVmd[A-Za-z0-9]{59})",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "anthropic_key",
        family: "ai",
        regex: r"sk-ant-[A-Za-z0-9_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "openai_key",
        family: "ai",
        regex: r"sk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "openai_legacy",
        family: "ai",
        regex: r"sk-[A-Za-z0-9]{48}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "openrouter_key",
        family: "ai",
        regex: r"sk-or-v1-[a-f0-9]{64}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "groq_key",
        family: "ai",
        regex: r"gsk_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "fireworks_key",
        family: "ai",
        regex: r"fw_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "huggingface_token",
        family: "ai",
        regex: r"(?:hf_|api_org_)[A-Za-z]{30,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "google_api_key",
        family: "ai",
        regex: r"AIza[A-Za-z0-9_-]{35}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "langsmith_key",
        family: "ai",
        regex: r"lsv2_(?:pt|sk)_[a-f0-9]{32}_[a-f0-9]{10}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "perplexity_key",
        family: "ai",
        regex: r"pplx-[A-Za-z0-9]{32,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "aws_access_key",
        family: "cloud",
        regex: r"(?:AKIA|ABIA|ACCA)[0-9A-Z]{16}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "vault_token",
        family: "cloud",
        regex: r"hvs\.[A-Za-z0-9_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "digitalocean_token",
        family: "cloud",
        regex: r"do[oprt]_v1_[a-f0-9]{64}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "azure_client_secret",
        family: "cloud",
        regex: r"[A-Za-z0-9_~.]{3}[0-9]Q~[A-Za-z0-9_~.-]{31,34}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "cloudflare_ca_key",
        family: "cloud",
        regex: r"v1\.0-[a-f0-9]{24}-[a-f0-9]{146}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "flyio_token",
        family: "cloud",
        regex: r"(?:fo1_[A-Za-z0-9_-]{43}|fm[12][ar]?_[A-Za-z0-9+/]{100,})",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "alibaba_key_id",
        family: "cloud",
        regex: r"LTAI[A-Za-z0-9]{20}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "hcp_terraform_token",
        family: "cloud",
        regex: r"[a-z0-9]{14}\.atlasv1\.[A-Za-z0-9_=-]{60,70}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "doppler_token",
        family: "cloud",
        regex: r"dp\.(?:pt|st|ct|sa|scim|audit)\.[A-Za-z0-9]{40,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "dynatrace_token",
        family: "cloud",
        regex: r"dt0c01\.[A-Z0-9]{24}\.[A-Z0-9]{64}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "databricks_token",
        family: "cloud",
        regex: r"dapi[a-f0-9]{32}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "grafana_token",
        family: "cloud",
        regex: r"(?:glc_[A-Za-z0-9+/=]{32,}|glsa_[A-Za-z0-9]{32}_[A-Fa-f0-9]{8})",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "sentry_token",
        family: "cloud",
        regex: r"(?:sntrys_|sntryu_)[A-Za-z0-9_=+/]{32,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "pulumi_token",
        family: "cloud",
        regex: r"pul-[a-f0-9]{40}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "planetscale_token",
        family: "cloud",
        regex: r"pscale_(?:tkn|oauth|pw)_[A-Za-z0-9_=.-]{32,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "supabase_token",
        family: "cloud",
        regex: r"sb(?:p|s|_secret)_[A-Za-z0-9_-]{40,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "tailscale_key",
        family: "cloud",
        regex: r"tskey-(?:auth|api|client)-[A-Za-z0-9-]{10,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "npm_token",
        family: "registry",
        regex: r"npm_[A-Za-z0-9]{30,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "pypi_token",
        family: "registry",
        regex: r"pypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{50,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "rubygems_token",
        family: "registry",
        regex: r"rubygems_[a-f0-9]{48}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "clojars_token",
        family: "registry",
        regex: r"CLOJARS_[A-Za-z0-9]{60}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "nuget_key",
        family: "registry",
        regex: r"oy2[a-z0-9]{43}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "onepassword_token",
        family: "registry",
        regex: r"ops_eyJ[A-Za-z0-9+/]{100,}={0,3}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "age_secret_key",
        family: "registry",
        regex: r"AGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "slack_token",
        family: "saas",
        regex: r"xox[baprse]-[0-9A-Za-z-]{10,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "slack_app_token",
        family: "saas",
        regex: r"xapp-[0-9]-[A-Z0-9]+-[0-9]+-[a-f0-9]+",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "slack_webhook",
        family: "saas",
        regex: r"https://hooks\.slack\.com/services/T[A-Za-z0-9]+/B[A-Za-z0-9]+/[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "youtrack_token",
        family: "saas",
        regex: r"perm[-:][A-Za-z0-9=._-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "bitwarden_token",
        family: "saas",
        regex: r"btr-[A-Za-z0-9._-]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "stripe_key",
        family: "saas",
        regex: r"(?:sk|rk|pk)_(?:test|live|prod)_[A-Za-z0-9]{10,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "sendgrid_key",
        family: "saas",
        regex: r"SG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "twilio_key",
        family: "saas",
        regex: r"(?:SK|AC)[0-9a-fA-F]{32}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "shopify_token",
        family: "saas",
        regex: r"shp(?:at|ca|pa|ss)_[a-fA-F0-9]{32}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "linear_key",
        family: "saas",
        regex: r"lin_api_[A-Za-z0-9]{40}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "notion_token",
        family: "saas",
        regex: r"(?:ntn_|secret_)[A-Za-z0-9]{40,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "figma_token",
        family: "saas",
        regex: r"figd_[A-Za-z0-9_-]{40,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "square_token",
        family: "saas",
        regex: r"(?:EAAA|sq0atp-|sq0csp-)[A-Za-z0-9_-]{22,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "discord_bot_token",
        family: "saas",
        regex: r"[MNO][A-Za-z0-9_-]{23}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "telegram_bot_token",
        family: "saas",
        regex: r"[0-9]{8,10}:AA[A-Za-z0-9_-]{33}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "airtable_pat",
        family: "saas",
        regex: r"pat[A-Za-z0-9]{14}\.[a-f0-9]{64}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "readme_token",
        family: "saas",
        regex: r"rdme_[a-z0-9]{70}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "cctui_token",
        family: "generic",
        regex: r"cctui_[a-z]_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "ccipat",
        family: "generic",
        regex: r"CCIPAT_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "private_key",
        family: "generic",
        regex: r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----(?:[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|(?:\s*[A-Za-z0-9+/=]{16,})+)",
        group: 0,
        high_entropy: false,
    },
    B {
        category: "jwt",
        family: "generic",
        regex: r"eyJ[A-Za-z0-9_-]{6,}\.eyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}",
        group: 0,
        high_entropy: true,
    },
    // `group: 1` on the next three: mask only the credential span so the
    // connection string / endpoint / variable name stays legible.
    B {
        category: "db_url_password",
        family: "generic",
        regex: r"[a-zA-Z][a-zA-Z0-9+.\-]*://[^:/@\s]+:([^@/\s]+)@",
        group: 1,
        high_entropy: false,
    },
    B {
        category: "url_signed_param",
        family: "generic",
        regex: r#"(?i)[?&](?:signature|sig|token|access_token|api_?key|auth|password|secret|sas|x-amz-signature)=([^&\s"'\[\]]{8,})"#,
        group: 1,
        high_entropy: true,
    },
    B {
        category: "env_assignment",
        family: "generic",
        regex: r"(?m)^\s*(?:export\s+)?[A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|APIKEY|API_KEY|CREDENTIAL)[A-Z0-9_]*\s*=\s*([^\s\[\]]{8,})",
        group: 1,
        high_entropy: true,
    },
    // Password-manager encrypted field: `<type>.<iv>|<ciphertext>[|<mac>]`.
    B {
        category: "vault_cipher_string",
        family: "generic",
        regex: r"(?-u:\b)[0-9]\.[A-Za-z0-9+/]{16,}={0,2}\|[A-Za-z0-9+/]{16,}={0,2}(?:\|[A-Za-z0-9+/]{16,}={0,2})?",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "otpauth_uri",
        family: "generic",
        regex: r#"otpauth://[A-Za-z]+/([^\s"'<>\\\[\]]+)"#,
        group: 1,
        high_entropy: true,
    },
    // The run detectors go last so a known shape claims its span first; their
    // regexes are deliberately loose, `entropy` decides.
    B {
        category: "high_entropy_hex",
        family: "generic",
        regex: r"(?-u:\b)[0-9a-fA-F]{32,}(?-u:\b)",
        group: 0,
        high_entropy: true,
    },
    B {
        category: "high_entropy_base64",
        family: "generic",
        regex: r"[A-Za-z0-9+/_-]{32,}={0,2}",
        group: 0,
        high_entropy: true,
    },
];

fn validator(category: &str) -> Option<Validator> {
    match category {
        "high_entropy_hex" => Some(entropy::hex_secret),
        "high_entropy_base64" => Some(entropy::base64_secret),
        _ => None,
    }
}

/// Category for a value masked because its key announced a secret rather than
/// because the value matched a shape.
const SECRET_FIELD: &str = "secret_field";

const SECRET_KEY_NAME: &str =
    r"(?i)(token|secret|password|passwd|api[_-]?key|credential|auth|private[_-]?key|signature)";

/// Minimum length for a key-name-matched value to be worth masking.
const MIN_SECRET_VALUE_LEN: usize = 12;

fn secret_key_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(SECRET_KEY_NAME).expect("secret key-name pattern must compile"))
}

fn is_secret_key_name(name: &str) -> bool {
    secret_key_re().is_match(name)
}

/// Whether a value sitting under a secret-announcing key is opaque enough to be
/// a credential. Rejects the shapes that are routinely stored under such keys
/// and are not secrets: paths, URLs, booleans, and lowercase enum values
/// (`client_secret_post`, `bearer`, …).
fn looks_like_secret_value(v: &str) -> bool {
    if v.len() < MIN_SECRET_VALUE_LEN || v.len() > 4096 {
        return false;
    }
    if v.contains("[REDACTED:") || v.chars().any(char::is_whitespace) {
        return false;
    }
    if v.starts_with('/') || v.starts_with("./") || v.starts_with("~/") || v.contains("://") {
        return false;
    }
    if matches!(v.to_ascii_lowercase().as_str(), "true" | "false" | "null" | "none" | "undefined") {
        return false;
    }
    // An enum-ish token: lowercase words joined by `-`/`_`/`.`, no digits.
    !v.chars().all(|c| c.is_ascii_lowercase() || matches!(c, '-' | '_' | '.'))
}

/// Random base64 is full of token-shaped runs, and one masked run makes an
/// image undecodable: encoded attachment data is never scanned.
fn binary_field(obj: &Map<String, Value>, key: &str) -> bool {
    match obj.get("type").and_then(Value::as_str) {
        Some("cctui-blob") => true,
        Some("base64") => key == "data",
        _ => {
            key == "data"
                && ["mimeType", "mime_type", "media_type"].iter().any(|k| obj.contains_key(*k))
        }
    }
}

fn binary_spans(s: &str) -> Vec<(usize, usize)> {
    static RES: std::sync::OnceLock<[Regex; 2]> = std::sync::OnceLock::new();
    if !s.contains("data") {
        return Vec::new();
    }
    let [data_url, embedded] = RES.get_or_init(|| {
        [
            Regex::new(r"data:[\w.+-]+/[\w.+-]+(?:;[\w.+-]+=[\w.+-]+)*;base64,[A-Za-z0-9+/=]+")
                .expect("data url pattern must compile"),
            Regex::new(r#"\\*"data\\*"\s*:\s*\\*"([A-Za-z0-9+/]{64,}={0,2})\\*""#)
                .expect("embedded base64 pattern must compile"),
        ]
    });
    let mut spans: Vec<(usize, usize)> = data_url
        .find_iter(s)
        .map(|m| (m.start(), m.end()))
        .chain(embedded.captures_iter(s).filter_map(|c| c.get(1)).map(|m| (m.start(), m.end())))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn text_parts(s: &str) -> Vec<(&str, &str)> {
    let mut last = 0;
    let mut parts = Vec::new();
    for (start, end) in binary_spans(s).into_iter().chain(std::iter::once((s.len(), s.len()))) {
        parts.push((&s[last..start], &s[start..end]));
        last = end;
    }
    parts
}

/// Compile the effective detector set.
///
/// Built-in defaults plus each `enabled`
/// user pattern (already validated server-side; an uncompilable one is skipped
/// defensively). `key` is the vault key used for the correlation suffix — an
/// empty key drops the suffix (dev/test). Returns [`CompiledPatterns::disabled`]
/// when `enabled` is false.
#[must_use]
pub fn compile(enabled: bool, user: &[(String, String)], key: &[u8]) -> CompiledPatterns {
    if !enabled {
        return CompiledPatterns::disabled();
    }
    let mut patterns: Vec<Compiled> = BUILTINS
        .iter()
        .map(|b| Compiled {
            category: b.category.to_owned(),
            re: Regex::new(b.regex).expect("built-in redaction pattern must compile"),
            group: b.group,
            high_entropy: b.high_entropy,
            builtin: true,
            validate: validator(b.category),
        })
        .collect();
    for (name, regex) in user {
        match Regex::new(regex) {
            Ok(re) => patterns.push(Compiled {
                category: sanitize_category(name),
                re,
                group: 0,
                high_entropy: false,
                builtin: false,
                validate: None,
            }),
            Err(e) => {
                tracing::warn!(pattern = %name, error = %e, "skipping uncompilable user scrub pattern");
            }
        }
    }
    let prefilter = RegexSet::new(patterns.iter().map(|c| c.re.as_str())).ok();
    CompiledPatterns { patterns, prefilter, key: key.to_vec(), allow: BTreeSet::new() }
}

/// The built-in detector categories with their UI family, so the webui renders
/// the effective list instead of a hand-maintained copy that silently drifts.
#[must_use]
pub fn builtin_categories() -> Vec<(&'static str, &'static str)> {
    BUILTINS.iter().map(|b| (b.category, b.family)).collect()
}

/// Validate that a user-supplied pattern compiles, so the server can reject a
/// bad regex at `PUT` time instead of silently dropping it on the daemon.
pub fn validate_regex(pattern: &str) -> Result<(), String> {
    Regex::new(pattern).map(|_| ()).map_err(|e| e.to_string())
}

/// Keep category tokens greppable and placeholder-safe: lowercase, `[a-z0-9_]`,
/// so a user-named pattern can't inject `]` into `[REDACTED:...]`.
fn sanitize_category(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.is_empty() { "custom".to_owned() } else { s }
}

/// Truncated keyed-HMAC of a matched secret — the correlation suffix. Keyed so a
/// low-entropy secret isn't dictionary-brute-forceable from the suffix.
fn correlation_suffix(key: &[u8], matched: &str) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(matched.as_bytes());
    let bytes = mac.finalize().into_bytes();
    hex::encode(&bytes[..2])
}

fn placeholder(c: &Compiled, matched: &str, key: &[u8]) -> String {
    if c.high_entropy && !key.is_empty() {
        format!("[REDACTED:{}:{}]", c.category, correlation_suffix(key, matched))
    } else {
        format!("[REDACTED:{}]", c.category)
    }
}

/// Apply one detector across `input`, masking each matched group span. Returns
/// the rewritten string and the number of substitutions.
fn apply(input: &str, c: &Compiled, patterns: &CompiledPatterns) -> (String, usize) {
    let mut out = String::with_capacity(input.len());
    let mut last = 0usize;
    let mut count = 0usize;
    for caps in c.re.captures_iter(input) {
        let Some(g) = caps.get(c.group).or_else(|| caps.get(0)) else { continue };
        if !patterns.masks(c, input, &g) {
            continue;
        }
        out.push_str(&input[last..g.start()]);
        out.push_str(&placeholder(c, g.as_str(), &patterns.key));
        last = g.end();
        count += 1;
    }
    if count == 0 {
        return (input.to_owned(), 0);
    }
    out.push_str(&input[last..]);
    (out, count)
}

/// Redact a single string leaf, accumulating per-category counts. Returns the
/// rewritten string only when something changed.
fn redact_string(
    input: &str,
    patterns: &CompiledPatterns,
    stats: &mut BTreeMap<String, usize>,
) -> Option<String> {
    if input.len() > MAX_FIELD_LEN {
        return None;
    }
    let parts = text_parts(input);
    if let [(text, "")] = parts.as_slice() {
        return redact_text(text, patterns, stats);
    }
    let mut out = String::with_capacity(input.len());
    let mut changed = false;
    for (text, binary) in parts {
        match redact_text(text, patterns, stats) {
            Some(r) => {
                out.push_str(&r);
                changed = true;
            }
            None => out.push_str(text),
        }
        out.push_str(binary);
    }
    changed.then_some(out)
}

fn redact_text(
    input: &str,
    patterns: &CompiledPatterns,
    stats: &mut BTreeMap<String, usize>,
) -> Option<String> {
    let candidates: Vec<usize> = patterns.prefilter.as_ref().map_or_else(
        || (0..patterns.patterns.len()).collect(),
        |set| set.matches(input).into_iter().collect(),
    );
    if candidates.is_empty() {
        return None;
    }
    let mut current = input.to_owned();
    let mut changed = false;
    for c in candidates.into_iter().filter_map(|i| patterns.patterns.get(i)) {
        let (next, n) = apply(&current, c, patterns);
        if n > 0 {
            *stats.entry(c.category.clone()).or_insert(0) += n;
            current = next;
            changed = true;
        }
    }
    changed.then_some(current)
}

/// `secret_key` is true when the enclosing JSON key — or, for the k8s
/// `{"name": …, "value": …}` shape, the sibling name field — announced that this
/// value is a credential.
fn walk(
    value: &mut Value,
    secret_key: bool,
    patterns: &CompiledPatterns,
    stats: &mut BTreeMap<String, usize>,
) {
    match value {
        Value::String(s) => {
            if secret_key && looks_like_secret_value(s) {
                *stats.entry(SECRET_FIELD.to_owned()).or_insert(0) += 1;
                *s = secret_field_placeholder(s, &patterns.key);
            } else if let Some(replaced) = redact_string(s, patterns, stats) {
                *s = replaced;
            }
        }
        Value::Array(arr) => {
            for v in arr {
                walk(v, secret_key, patterns, stats);
            }
        }
        Value::Object(obj) => {
            let named_secret = ["name", "Name", "key", "Key"]
                .iter()
                .filter_map(|k| obj.get(*k))
                .filter_map(Value::as_str)
                .any(is_secret_key_name);
            let skipped: Vec<String> =
                obj.keys().filter(|k| binary_field(obj, k)).cloned().collect();
            for (k, v) in obj.iter_mut() {
                if skipped.contains(k) {
                    continue;
                }
                let announced = is_secret_key_name(k)
                    || (named_secret && matches!(k.as_str(), "value" | "Value"));
                walk(v, announced, patterns, stats);
            }
        }
        _ => {}
    }
}

fn secret_field_placeholder(matched: &str, key: &[u8]) -> String {
    if key.is_empty() {
        format!("[REDACTED:{SECRET_FIELD}]")
    } else {
        format!("[REDACTED:{SECRET_FIELD}:{}]", correlation_suffix(key, matched))
    }
}

/// Redact `value` in place, rewriting string leaves only. Returns the total
/// number of substitutions. No-op (returns 0) when `patterns` is disabled/empty.
pub fn redact_json(value: &mut Value, patterns: &CompiledPatterns) -> usize {
    if patterns.is_empty() {
        return 0;
    }
    let mut stats = BTreeMap::new();
    walk(value, false, patterns, &mut stats);
    stats.values().sum()
}

/// Like [`redact_json`] but returns per-category substitution counts (for the
/// re-scrub dry-run report). The `value` is still mutated in place.
pub fn redact_json_stats(
    value: &mut Value,
    patterns: &CompiledPatterns,
) -> BTreeMap<String, usize> {
    let mut stats = BTreeMap::new();
    if !patterns.is_empty() {
        walk(value, false, patterns, &mut stats);
    }
    stats
}

/// How many example matches are kept per category for the scan preview.
pub const SAMPLES_PER_CATEGORY: usize = 5;
/// How many matches per category are inspected for the identifier heuristic
/// before the collector stops looking; the ratio is stable well before this.
pub const SAMPLE_EXAMINE_CAP: usize = 200;
/// Characters of surrounding text kept on each side of a sampled match.
const SAMPLE_CONTEXT: usize = 40;

/// One example match, as shown in the scan preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// The matched text with any value it carries masked. A bare match is kept
    /// whole only when it cannot itself be a secret, so a user can see their
    /// pattern hit `access_token` without the sample becoming a credential.
    pub text: String,
    /// Surrounding text, itself redacted so a neighbouring secret cannot ride
    /// along into the preview.
    pub context: String,
    /// A `=` or `:` with a value follows the match.
    pub value_follows: bool,
}

/// Example matches for one category plus the tallies behind
/// [`CategorySamples::looks_like_identifiers`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CategorySamples {
    pub samples: Vec<Sample>,
    pub examined: usize,
    pub with_value: usize,
}

impl CategorySamples {
    /// Most inspected matches were bare names with no value after them — the
    /// signature of a `\w+_token`-style pattern hitting identifiers.
    #[must_use]
    pub const fn looks_like_identifiers(&self) -> bool {
        self.examined >= 3 && self.with_value * 2 < self.examined
    }

    const fn done(&self) -> bool {
        self.examined >= SAMPLE_EXAMINE_CAP
    }
}

/// Collect example matches per category from `value`.
///
/// Accumulates into `out` so a caller can sample across many rows under one
/// budget. Read-only: unlike [`redact_json_stats`] it never rewrites the
/// document.
pub fn collect_samples(
    value: &Value,
    patterns: &CompiledPatterns,
    out: &mut BTreeMap<String, CategorySamples>,
) {
    if patterns.is_empty() {
        return;
    }
    sample_walk(value, patterns, out);
}

fn sample_walk(
    value: &Value,
    patterns: &CompiledPatterns,
    out: &mut BTreeMap<String, CategorySamples>,
) {
    match value {
        Value::String(s) => sample_string(s, patterns, out),
        Value::Array(arr) => {
            for v in arr {
                sample_walk(v, patterns, out);
            }
        }
        Value::Object(obj) => {
            for (k, v) in obj {
                if !binary_field(obj, k) {
                    sample_walk(v, patterns, out);
                }
            }
        }
        _ => {}
    }
}

fn sample_string(
    input: &str,
    patterns: &CompiledPatterns,
    out: &mut BTreeMap<String, CategorySamples>,
) {
    if input.len() > MAX_FIELD_LEN {
        return;
    }
    for (text, _) in text_parts(input) {
        sample_text_part(text, patterns, out);
    }
}

fn sample_text_part(
    input: &str,
    patterns: &CompiledPatterns,
    out: &mut BTreeMap<String, CategorySamples>,
) {
    let candidates: Vec<usize> = patterns.prefilter.as_ref().map_or_else(
        || (0..patterns.patterns.len()).collect(),
        |set| set.matches(input).into_iter().collect(),
    );
    // Progressive like `redact_text`: a span an earlier detector claims is
    // neither re-sampled by a later one nor left raw in its context.
    let mut current = input.to_owned();
    for c in candidates.into_iter().filter_map(|i| patterns.patterns.get(i)) {
        if !out.get(&c.category).is_some_and(CategorySamples::done) {
            sample_matches(&current, c, patterns, out);
        }
        let (next, n) = apply(&current, c, patterns);
        if n > 0 {
            current = next;
        }
    }
}

fn sample_matches(
    input: &str,
    c: &Compiled,
    patterns: &CompiledPatterns,
    out: &mut BTreeMap<String, CategorySamples>,
) {
    for caps in c.re.captures_iter(input) {
        let Some(g) = caps.get(c.group).or_else(|| caps.get(0)) else { continue };
        if !patterns.masks(c, input, &g) {
            continue;
        }
        let entry = out.entry(c.category.clone()).or_default();
        let has_value = value_follows(&input[g.end()..]);
        entry.examined += 1;
        if has_value {
            entry.with_value += 1;
        }
        if entry.samples.len() < SAMPLES_PER_CATEGORY {
            entry.samples.push(Sample {
                text: sample_text(c, g.as_str()),
                context: sample_context(input, g.start(), g.end(), patterns),
                value_follows: has_value,
            });
        }
        if entry.done() {
            break;
        }
    }
}

/// A sample is stored and returned over the API, so it must never carry a
/// credential: whatever part of a match is a *value* is masked, whatever part
/// names it is kept. A match shaped `name=value` keeps `name=` and masks the
/// rest; a bare match is kept whole only when it cannot itself be a secret —
/// which is what makes an `input_token` identifier hit legible in the preview.
fn sample_text(c: &Compiled, matched: &str) -> String {
    if c.builtin {
        return if c.high_entropy { mask_token(matched) } else { mask_value(matched) };
    }
    if let Some((start, end)) = inner_value_span(matched) {
        let (name, value, rest) = (&matched[..start], &matched[start..end], &matched[end..]);
        return format!("{name}{}{rest}", mask_value(value));
    }
    if c.high_entropy || looks_like_secret_value(matched) {
        return mask_token(matched);
    }
    matched.to_owned()
}

fn mask_token(v: &str) -> String {
    let head: String = v.chars().take(4).collect();
    format!("{head}… ({} chars)", v.chars().count())
}

fn mask_value(v: &str) -> String {
    let n = v.chars().count();
    if n <= 6 {
        return format!("… ({n} chars)");
    }
    let head: String = v.chars().take(2).collect();
    format!("{head}… ({n} chars)")
}

fn sample_context(input: &str, start: usize, end: usize, patterns: &CompiledPatterns) -> String {
    let before = &input[floor_boundary(input, start.saturating_sub(SAMPLE_CONTEXT))..start];
    let tail = &input[end..];
    let after = &tail[..ceil_boundary(tail, SAMPLE_CONTEXT.min(tail.len()))];
    // A detector that matched only the key (`\w+_token`) leaves the credential
    // itself sitting in the context window; mask it there too.
    let after = match value_span(tail) {
        Some((s, e)) if s < after.len() => {
            let rest = if e < after.len() { &after[e..] } else { "" };
            format!("{}{}{}", &after[..s], mask_value(&tail[s..e]), rest)
        }
        _ => after.to_owned(),
    };
    let mut ctx = String::with_capacity(before.len() + after.len() + 3);
    ctx.push_str(before);
    ctx.push('…');
    ctx.push_str(&after);
    let mut sink = BTreeMap::new();
    let ctx = redact_string(&ctx, patterns, &mut sink).unwrap_or(ctx);
    ctx.split_whitespace().collect::<Vec<_>>().join(" ")
}

const fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

const fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

const SEP_PADDING: [char; 5] = [' ', '\t', '"', '\'', '\\'];
const VALUE_END: [char; 9] = [' ', '\t', '\n', '\r', '"', '\'', ',', '}', ']'];

fn padding_len(s: &str) -> usize {
    s.len() - s.trim_start_matches(&SEP_PADDING[..]).len()
}

/// The span of the value `after` assigns, if it assigns one (`token: "x"`,
/// `TOKEN=x`). A bare identifier in prose, or a key with nothing after it, has
/// no such span.
fn value_span(after: &str) -> Option<(usize, usize)> {
    let lead = padding_len(after);
    let rest = after.get(lead..)?.strip_prefix(['=', ':'])?;
    let start = lead + 1 + padding_len(rest);
    let tail = after.get(start..).filter(|t| !t.is_empty())?;
    let end = start + tail.find(&VALUE_END[..]).unwrap_or(tail.len());
    (end > start).then_some((start, end))
}

/// The same span, found inside a match that carries its own `name=value`.
fn inner_value_span(matched: &str) -> Option<(usize, usize)> {
    let sep = matched.find(['=', ':']).filter(|&i| i > 0)?;
    let (start, end) = value_span(matched.get(sep..)?)?;
    Some((sep + start, sep + end))
}

fn value_follows(after: &str) -> bool {
    value_span(after).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY: &[u8] = b"test-key-32-bytes-test-key-32byt";

    fn p() -> CompiledPatterns {
        compile(true, &[], KEY)
    }

    fn redact_str(s: &str) -> String {
        let mut v = json!(s);
        redact_json(&mut v, &p());
        v.as_str().unwrap().to_owned()
    }

    // "gl" + "pat-" split so GitHub push protection doesn't flag the fixture
    fn gitlab_fixture() -> String {
        format!("{}{}abcdef0123456789ABCD end", "gl", "pat-")
    }

    #[test]
    fn masks_each_default_category_and_keeps_surrounding_text() {
        let gitlab = gitlab_fixture();
        let cases = [
            ("gh", "curl -H \"Authorization: Bearer ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123\""),
            ("github_pat", "github_pat_11ABCDEFG0123456789abcdefg"),
            ("npm", "//registry: npm_abcdefghijklmnopqrstuvwxyz0123456789"),
            ("anthropic", "ANTHROPIC_API_KEY=sk-ant-api03-abcDEF0123456789xyz"),
            ("aws", "AKIAIOSFODNN7EXAMPLE here"),
            ("vault", "token hvs.CAESIJ0123456789abcdefghij done"),
            ("gitlab", gitlab.as_str()),
            ("slack", "xoxb-1234567890-abcdefghijkl"),
            ("cctui", "cctui_m_ABCDEFGHIJKLMNOPQRSTUV"),
            ("ccipat", "CCIPAT_ABCDEFGHIJKLMNOPQRSTUV"),
        ];
        for (label, input) in cases {
            let out = redact_str(input);
            assert!(out.contains("[REDACTED:"), "{label}: no placeholder in {out}");
            assert!(!out.contains("0123456789ab"), "{label}: secret leaked: {out}");
        }
    }

    #[test]
    fn db_url_masks_only_the_password() {
        let out = redact_str("postgres://admin:s3cr3tPass@db.example.com:5432/app");
        assert!(
            out.starts_with("postgres://admin:[REDACTED:db_url_password]@db.example.com"),
            "{out}"
        );
        assert!(!out.contains("s3cr3tPass"));
    }

    #[test]
    fn private_key_block_is_masked_whole() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIabc123\nZZ==\n-----END RSA PRIVATE KEY-----";
        let out = redact_str(&format!("key:\n{pem}\ndone"));
        assert!(out.contains("[REDACTED:private_key]"), "{out}");
        assert!(!out.contains("MIIabc123"));
        assert!(out.ends_with("done"));
    }

    #[test]
    fn jwt_is_masked() {
        let out = redact_str("eyJhbGciOi.eyJzdWI6MTIz.SflKxwRJSM_abc123");
        assert!(out.contains("[REDACTED:jwt:"), "{out}");
    }

    #[test]
    fn high_entropy_gets_keyed_suffix_low_entropy_does_not() {
        let gh = redact_str("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123");
        assert!(gh.starts_with("[REDACTED:github_token:") && gh.ends_with(']'), "{gh}");
        let db = redact_str("postgres://u:pw123456@h/db");
        assert_eq!(db, "postgres://u:[REDACTED:db_url_password]@h/db");
    }

    #[test]
    fn suffix_is_stable_and_secret_specific() {
        let a = redact_str("ghp_AAAAAAAAAAAAAAAAAAAAAAA0000");
        let b = redact_str("ghp_AAAAAAAAAAAAAAAAAAAAAAA0000");
        let c = redact_str("ghp_BBBBBBBBBBBBBBBBBBBBBBB1111");
        assert_eq!(a, b, "same secret -> same suffix");
        assert_ne!(a, c, "different secret -> different suffix");
    }

    #[test]
    fn idempotent_rescrub_is_a_noop() {
        let once =
            redact_str("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123 and hvs.CAESIJ0123456789abcdefghij");
        let twice = redact_str(&once);
        assert_eq!(once, twice);
        let mut v = json!(once);
        assert_eq!(redact_json(&mut v, &p()), 0);
    }

    #[test]
    fn walks_nested_structure_only_string_leaves() {
        let mut v = json!({
            "cmd": "export TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123",
            "n": 42,
            "arr": ["clean", "hvs.CAESIJ0123456789abcdefghij"],
            "nested": { "k": "sk-ant-api03-abcDEF0123456789xyz" }
        });
        let n = redact_json(&mut v, &p());
        assert_eq!(n, 3);
        assert_eq!(v["n"], json!(42));
        assert!(v["cmd"].as_str().unwrap().contains("[REDACTED:github_token"));
        assert!(v["arr"][1].as_str().unwrap().contains("[REDACTED:vault_token"));
        assert!(v["nested"]["k"].as_str().unwrap().contains("[REDACTED:anthropic_key"));
    }

    #[test]
    fn gateway_session_token_is_masked() {
        let out = redact_str("cctui_s_e143d90d82244ae99b945fe74ff501c55bfa5afe52bf4761a3c5a271a9");
        assert!(out.starts_with("[REDACTED:cctui_token:"), "{out}");
        assert!(!out.contains("e143d90d"));
    }

    #[test]
    fn signed_callback_url_keeps_the_endpoint_and_masks_the_signature() {
        let out = redact_str(
            "https://n8n.dorsk.dev/webhook-waiting/181134?signature=5ce8ef8a82455572f3e483e6b67a25ca",
        );
        assert!(
            out.starts_with("https://n8n.dorsk.dev/webhook-waiting/181134?signature="),
            "{out}"
        );
        assert!(out.contains("[REDACTED:url_signed_param:"), "{out}");
        assert!(!out.contains("5ce8ef8a"));
    }

    #[test]
    fn env_assignment_in_plain_text_is_masked() {
        let out = redact_str("export FIREWORKS_API_KEY=zX9qLmNb2v8Kd4Rt\nnext line");
        assert!(out.starts_with("export FIREWORKS_API_KEY=[REDACTED:env_assignment:"), "{out}");
        assert!(out.ends_with("next line"));
        assert!(!out.contains("zX9qLmNb2v8Kd4Rt"));
    }

    #[test]
    fn key_name_masks_an_opaque_value_of_no_known_shape() {
        let mut v = json!({ "authToken": "Zq7NmVbXc2Ld9Kt4Rw8P", "note": "Zq7NmVbXc2Ld9Kt4Rw8P" });
        assert_eq!(redact_json(&mut v, &p()), 1);
        assert!(v["authToken"].as_str().unwrap().starts_with("[REDACTED:secret_field:"));
        assert_eq!(v["note"], json!("Zq7NmVbXc2Ld9Kt4Rw8P"));
    }

    #[test]
    fn key_name_masking_spares_paths_urls_and_enum_values() {
        let mut v = json!({
            "tokenPath": "/var/run/secrets/token",
            "authUrl": "https://auth.example.com/oauth/authorize",
            "token_endpoint_auth_method": "client_secret_post",
            "secretEnabled": true,
        });
        assert_eq!(redact_json(&mut v, &p()), 0, "{v}");
    }

    /// The shape that leaked: a `kubectl` dry-run dump of a worker pod spec.
    #[test]
    fn leaked_pod_spec_shape_is_fully_masked() {
        let mut v = json!({
            "spec": { "containers": [{
                "name": "worker",
                "image": "harbor.dorsk.dev/homelab/cctui-worker:0.8.14",
                "env": [
                    { "name": "CCTUI_MACHINE_KEY", "value": "cctui_m_QRSTUVWXYZ0123456789abcdefgh" },
                    { "name": "ANTHROPIC_AUTH_TOKEN", "value": "cctui_s_e143d90d82244ae99b945fe74ff5" },
                    { "name": "OPENAI_API_KEY", "value": "cctui_s_7b19aa04c1f34c0d8e2a6b5f0d3e11" },
                    { "name": "FIREWORKS_API_KEY", "value": "Wm4TzPq9Lx2Kd7Rb5Nv8Hc3Ju6Ye1Gs" },
                    { "name": "REPLY_URL", "value": "https://n8n.dorsk.dev/webhook-waiting/181134?signature=5ce8ef8a82455572f3e483e6b67a25ca" },
                    { "name": "CCTUI_SERVER_URL", "value": "https://cctui.dorsk.dev" },
                ],
            }]},
        });
        assert!(redact_json(&mut v, &p()) > 0);
        let dumped = v.to_string();
        for secret in [
            "QRSTUVWXYZ0123456789abcdefgh",
            "e143d90d",
            "7b19aa04",
            "Wm4TzPq9Lx2Kd7Rb5Nv8Hc3Ju6Ye1Gs",
            "5ce8ef8a",
        ] {
            assert!(!dumped.contains(secret), "leaked {secret}: {dumped}");
        }
        let env = &v["spec"]["containers"][0]["env"];
        assert_eq!(env[5]["value"], json!("https://cctui.dorsk.dev"), "benign env rewritten");
        assert_eq!(
            v["spec"]["containers"][0]["image"],
            json!("harbor.dorsk.dev/homelab/cctui-worker:0.8.14"),
        );
        let once = v.clone();
        assert_eq!(redact_json(&mut v, &p()), 0, "rescrub must be idempotent");
        assert_eq!(v, once);
    }

    #[test]
    fn disabled_is_a_noop() {
        let mut v = json!("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123");
        assert_eq!(redact_json(&mut v, &CompiledPatterns::disabled()), 0);
        assert_eq!(v, json!("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"));
    }

    #[test]
    fn user_pattern_is_applied() {
        let pats = compile(true, &[("acme".to_owned(), r"ACME-[0-9]{6}".to_owned())], KEY);
        let mut v = json!("id ACME-123456 end");
        assert_eq!(redact_json(&mut v, &pats), 1);
        assert_eq!(v.as_str().unwrap(), "id [REDACTED:acme] end");
    }

    /// One fixture per built-in that MUST match. `n(c)` keeps the long
    /// fixed-length keys readable.
    fn fixtures() -> Vec<(&'static str, String)> {
        fn n(c: char, len: usize) -> String {
            std::iter::repeat_n(c, len).collect()
        }
        vec![
            ("github_token", "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123".into()),
            ("github_pat", "github_pat_11ABCDEFG0123456789abcdefg".into()),
            ("gitlab_token", format!("{}{}abcdef0123456789ABCD", "gl", "rt-")),
            ("gitlab_runner_token", format!("{}{}1348941abcdef0123456789ABCD", "G", "R")),
            ("sourcegraph_token", format!("sgp_{}", n('a', 40))),
            ("harness_key", format!("pat.{}.{}.{}", n('a', 22), n('b', 24), n('c', 20))),
            ("octopus_key", format!("{}{}-ABCDEFGHIJ0123456789ABCDEF", "AP", "I")),
            ("postman_key", format!("PMAK-{}-{}", n('a', 24), n('b', 34))),
            ("heroku_key", format!("HRKU-AA{}", n('a', 58))),
            ("artifactory_key", format!("AKCp{}", n('a', 69))),
            ("anthropic_key", "sk-ant-api03-abcDEF0123456789xyz".into()),
            ("openai_key", "sk-proj-abcdefghijklmnopqrstuvwx0123456789".into()),
            ("openai_legacy", format!("sk-{}", n('a', 48))),
            ("openrouter_key", format!("sk-or-v1-{}", n('a', 64))),
            ("groq_key", "gsk_abcdefghijklmnopqrstuvwxyz0123".into()),
            ("fireworks_key", "fw_3ZabcdefghijklmnopqrstuvW".into()),
            ("huggingface_token", format!("hf_{}", n('a', 32))),
            ("google_api_key", format!("AIza{}", n('a', 35))),
            ("langsmith_key", format!("lsv2_pt_{}_{}", n('a', 32), n('b', 10))),
            ("perplexity_key", format!("pplx-{}", n('a', 32))),
            ("aws_access_key", "AKIAIOSFODNN7EXAMPLE".into()),
            ("vault_token", "hvs.CAESIJ0123456789abcdefghij".into()),
            ("digitalocean_token", format!("dop_v1_{}", n('a', 64))),
            ("azure_client_secret", format!("abc8Q~{}", n('a', 33))),
            ("cloudflare_ca_key", format!("v1.0-{}-{}", n('a', 24), n('b', 146))),
            ("flyio_token", format!("fo1_{}", n('a', 43))),
            ("alibaba_key_id", format!("LTAI{}", n('a', 20))),
            ("hcp_terraform_token", format!("abcdefghijklmn.atlasv1.{}", n('a', 64))),
            ("doppler_token", format!("dp.pt.{}", n('a', 43))),
            ("dynatrace_token", format!("dt0c01.{}.{}", n('A', 24), n('B', 64))),
            ("databricks_token", format!("dapi{}", n('a', 32))),
            ("grafana_token", format!("glsa_{}_{}", n('a', 32), n('0', 8))),
            ("sentry_token", format!("sntryu_{}", n('a', 64))),
            ("pulumi_token", format!("pul-{}", n('a', 40))),
            ("planetscale_token", format!("pscale_tkn_{}", n('a', 40))),
            ("supabase_token", format!("sbp_{}", n('a', 40))),
            ("tailscale_key", "tskey-auth-abcdefghijklmnop".into()),
            ("npm_token", "npm_abcdefghijklmnopqrstuvwxyz0123456789".into()),
            ("pypi_token", format!("pypi-AgEIcHlwaS5vcmc{}", n('a', 50))),
            ("rubygems_token", format!("rubygems_{}", n('a', 48))),
            ("clojars_token", format!("CLOJARS_{}", n('a', 60))),
            ("nuget_key", format!("oy2{}", n('a', 43))),
            ("onepassword_token", format!("ops_eyJ{}", n('a', 120))),
            ("age_secret_key", format!("AGE-SECRET-KEY-1{}", n('Q', 58))),
            ("slack_token", "xoxe-1234567890-abcdefghijkl".into()),
            ("slack_app_token", "xapp-1-A01234ABCDE-1234567890123-abcdef0123456789".into()),
            (
                "slack_webhook",
                format!("https://hooks.slack.com/services/T01234ABC/B01234ABC/{}", n('a', 24)),
            ),
            ("youtrack_token", "perm:abcdefghij.0123456789.abcdefghij".into()),
            ("bitwarden_token", "btr-abcdefghij0123456789.abcd".into()),
            ("stripe_key", format!("{}_live_abcdefghij0123456789", "sk")),
            ("sendgrid_key", format!("SG.{}.{}", n('a', 22), n('b', 43))),
            ("twilio_key", format!("SK{}", n('a', 32))),
            ("shopify_token", format!("shpat_{}", n('a', 32))),
            ("linear_key", format!("lin_api_{}", n('a', 40))),
            ("notion_token", format!("ntn_{}", n('a', 44))),
            ("figma_token", format!("figd_{}", n('a', 40))),
            ("square_token", format!("sq0atp-{}", n('a', 22))),
            ("discord_bot_token", format!("M{}.{}.{}", n('a', 23), n('b', 6), n('c', 27))),
            ("telegram_bot_token", format!("123456789:AA{}", n('a', 33))),
            ("airtable_pat", format!("pat{}.{}", n('a', 14), n('b', 64))),
            ("readme_token", format!("rdme_{}", n('a', 70))),
            ("cctui_token", "cctui_s_e143d90d82244ae99b945fe74ff5".into()),
            ("ccipat", "CCIPAT_ABCDEFGHIJKLMNOPQRSTUV".into()),
            (
                "private_key",
                "-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----".into(),
            ),
            ("jwt", "eyJhbGciOi.eyJzdWI6MTIz.SflKxwRJSM_abc123".into()),
            ("db_url_password", "postgres://admin:s3cr3tPass@db.example.com/app".into()),
            ("url_signed_param", "https://n8n.example/wait/1?signature=5ce8ef8a8245".into()),
            ("env_assignment", "export FIREWORKS_API_KEY=zX9qLmNb2v8Kd4Rt".into()),
            ("vault_cipher_string", cipher_string("fixture")),
            ("otpauth_uri", otpauth_fixture()),
            ("high_entropy_hex", hex::encode(rand_bytes("hex-fixture", 24))),
            ("high_entropy_base64", b64(&rand_bytes("b64-fixture", 45))),
        ]
    }

    #[test]
    fn corpus_every_builtin_has_a_fixture_that_fires() {
        let fx = fixtures();
        let named: std::collections::BTreeSet<_> = fx.iter().map(|(c, _)| *c).collect();
        for b in BUILTINS {
            assert!(named.contains(b.category), "no fixture for {}", b.category);
        }
        assert_eq!(named.len(), BUILTINS.len(), "fixture/builtin count drift");
        for (cat, input) in &fx {
            let out = redact_str(input);
            assert!(
                out.contains(&format!("[REDACTED:{cat}")),
                "{cat} did not fire: {input} -> {out}"
            );
        }
    }

    /// The invariant the whole re-scrub design rests on: an emitted placeholder
    /// must not itself look like a secret, or a second sweep would cascade.
    #[test]
    fn corpus_no_builtin_matches_a_placeholder() {
        for b in BUILTINS {
            let re = Regex::new(b.regex).unwrap();
            for other in BUILTINS {
                for ph in [
                    format!("[REDACTED:{}]", other.category),
                    format!("[REDACTED:{}:9f2a]", other.category),
                ] {
                    assert!(!re.is_match(&ph), "{} matches placeholder {ph}", b.category);
                }
            }
        }
    }

    /// The settings UI renders a checked-in copy of this table. Regenerate with
    /// `node webui/scripts/gen-scrub-detectors.mjs` when this fails.
    #[test]
    fn builtin_list_matches_the_webui_copy() {
        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../webui/src/lib/scrubDetectors.generated.ts");
        let generated = std::fs::read_to_string(path).expect("generated detector list is present");
        for (category, family) in builtin_categories() {
            let line = format!("{{ category: '{category}', family: '{family}' }}");
            assert!(generated.contains(&line), "webui detector list is stale: missing {line}");
        }
        assert_eq!(
            generated.matches("{ category:").count(),
            BUILTINS.len(),
            "webui detector list has entries the engine does not"
        );
    }

    #[test]
    fn corpus_no_builtin_fires_on_benign_text() {
        let benign = [
            "the quick brown fox jumps over the lazy dog",
            "https://github.com/DorskFR/cctui/pull/340",
            "cargo test -p cctui-crypto redact",
            "let x = 42; // see AGENTS.md for details",
            "image: harbor.dorsk.dev/homelab/cctui-worker:0.8.14",
            "GET /api/v1/sessions?limit=50&order=desc",
        ];
        for b in BUILTINS {
            let re = Regex::new(b.regex).unwrap();
            for text in benign {
                assert!(!re.is_match(text), "{} false-positives on {text:?}", b.category);
            }
        }
    }

    #[test]
    fn corpus_fixtures_survive_a_second_pass_unchanged() {
        for (cat, input) in fixtures() {
            let once = redact_str(&input);
            assert_eq!(redact_str(&once), once, "{cat} is not idempotent");
        }
    }

    #[test]
    fn multi_mb_field_is_fast_and_correct() {
        let mut s = "a".repeat(4 * 1024 * 1024);
        s.push_str("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123");
        let mut v = json!(s);
        let start = std::time::Instant::now();
        let n = redact_json(&mut v, &p());
        assert!(start.elapsed().as_secs() < 5, "redaction too slow");
        assert_eq!(n, 1);
        assert!(v.as_str().unwrap().contains("[REDACTED:github_token"));
    }

    fn custom(regex: &str) -> CompiledPatterns {
        compile(true, &[("mytoken".to_owned(), regex.to_owned())], KEY)
    }

    #[test]
    fn samples_keep_context_and_abbreviate_high_entropy_matches() {
        let doc = json!({
            "cmd": "curl -H \"Authorization: Bearer ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123\" https://api"
        });
        let mut out = BTreeMap::new();
        collect_samples(&doc, &p(), &mut out);
        let gh = out.get("github_token").expect("github_token sampled");
        assert_eq!(gh.samples.len(), 1);
        let s = &gh.samples[0];
        assert!(!s.text.contains("MNOPQRSTUVWX"), "{}", s.text);
        assert!(s.text.starts_with("ghp_"), "{}", s.text);
        assert!(s.context.contains("Authorization"), "{}", s.context);
        assert!(!s.context.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"), "{}", s.context);
    }

    #[test]
    fn samples_are_capped_per_category_but_keep_counting_for_the_heuristic() {
        let line = (0..40).map(|i| format!("input_token{i} here")).collect::<Vec<_>>().join(" ");
        let mut out = BTreeMap::new();
        collect_samples(&json!(line), &custom(r"\w+_token\d+"), &mut out);
        let c = out.get("mytoken").unwrap();
        assert_eq!(c.samples.len(), SAMPLES_PER_CATEGORY);
        assert_eq!(c.examined, 40);
        assert_eq!(c.with_value, 0);
        assert!(c.looks_like_identifiers());
    }

    #[test]
    fn identifier_heuristic_clears_when_matches_carry_values() {
        let doc = json!({
            "a": "export MY_token=s3cret-value",
            "b": "{\"other_token\": \"s3cret-value\"}",
            "c": "set app_token = s3cret-value"
        });
        let mut out = BTreeMap::new();
        collect_samples(&doc, &custom(r"\w+_token"), &mut out);
        let c = out.get("mytoken").unwrap();
        assert_eq!((c.examined, c.with_value), (3, 3));
        assert!(!c.looks_like_identifiers());
        assert!(c.samples.iter().all(|s| s.value_follows));
    }

    #[test]
    fn identifier_heuristic_needs_a_few_matches_before_it_warns() {
        let mut out = BTreeMap::new();
        collect_samples(&json!("mentions input_token once"), &custom(r"\w+_token"), &mut out);
        let c = out.get("mytoken").unwrap();
        assert_eq!((c.examined, c.with_value), (1, 0));
        assert!(!c.looks_like_identifiers());
    }

    const SAMPLE_SECRET: &str = "s3cretvalue123XY";

    #[test]
    fn a_user_pattern_never_keeps_the_value_it_matched() {
        let doc = json!({ "a": format!("MY_token={SAMPLE_SECRET}") });

        let mut key_only = BTreeMap::new();
        collect_samples(&doc, &custom(r"\w+_token"), &mut key_only);
        let c = key_only.get("mytoken").expect("sampled");
        assert_eq!(c.samples[0].text, "MY_token");
        assert!(c.samples[0].value_follows);
        assert!(!format!("{key_only:?}").contains(SAMPLE_SECRET), "{key_only:?}");

        let mut with_value = BTreeMap::new();
        collect_samples(&doc, &custom(r"\w+_token=\S+"), &mut with_value);
        let c = with_value.get("mytoken").expect("sampled");
        assert_eq!(c.samples[0].text, "MY_token=s3… (16 chars)");
        assert!(!format!("{with_value:?}").contains(SAMPLE_SECRET), "{with_value:?}");
    }

    #[test]
    fn a_bare_match_is_kept_whole_only_when_it_cannot_be_a_secret() {
        let mut high = BTreeMap::new();
        collect_samples(&json!("token ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123 sent"), &p(), &mut high);
        assert_eq!(high["github_token"].samples[0].text, "ghp_… (32 chars)");

        let mut opaque = BTreeMap::new();
        let opaque_doc = json!("value Ax9Kd7Rb5Nv8Hc3Ju6Ye here");
        collect_samples(&opaque_doc, &custom("[A-Za-z0-9]{20}"), &mut opaque);
        assert_eq!(opaque["mytoken"].samples[0].text, "Ax9K… (20 chars)");

        let mut names = BTreeMap::new();
        collect_samples(
            &json!("the input_token count and the access_token list"),
            &custom(r"\w+_token"),
            &mut names,
        );
        let texts: Vec<&str> = names["mytoken"].samples.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["input_token", "access_token"]);
    }

    #[test]
    fn collect_samples_does_not_mutate_and_is_a_noop_when_disabled() {
        let doc = json!({ "text": "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123" });
        let mut out = BTreeMap::new();
        collect_samples(&doc, &CompiledPatterns::disabled(), &mut out);
        assert!(out.is_empty());
        assert_eq!(doc["text"], json!("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"));
    }

    /// Base64 with a token-shaped run, as found in prod screenshots.
    fn image_b64() -> String {
        let run = "EAAAqT7bZ3kLm9pW2xYv8sN4rD1fH6jC0";
        format!("iVBORw0KGgoAAAANSUhEUgAA{}x9+/{run}+/Qw{}", "Ab+/".repeat(40), "Zz09".repeat(40))
    }

    const GH: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123";

    fn image_result(data: &str) -> Value {
        json!({
            "kind": "tool_result",
            "tool_use_id": "toolu_01",
            "content": [
                {"type": "text", "text": format!("saved, token {GH}")},
                {"type": "image", "source": {
                    "type": "base64", "media_type": "image/png", "data": data,
                }},
            ],
        })
    }

    #[test]
    fn image_source_data_is_never_scanned_and_its_text_still_is() {
        let data = image_b64();
        assert!(redact_str(&data).contains("[REDACTED:square_token"), "token-shaped fixture");
        let mut v = image_result(&data);
        assert!(redact_json(&mut v, &p()) > 0);
        assert_eq!(v["content"][1]["source"]["data"], json!(data));
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("saved, token [REDACTED:github_token"), "{text}");
    }

    #[test]
    fn stringified_image_blocks_and_data_urls_keep_their_base64() {
        let data = image_b64();
        let content = image_result(&data)["content"].to_string();
        let mut v = json!({ "output_summary": content });
        redact_json(&mut v, &p());
        let blocks: Value = serde_json::from_str(v["output_summary"].as_str().unwrap()).unwrap();
        assert_eq!(blocks[1]["source"]["data"], json!(data));
        assert!(blocks[0]["text"].as_str().unwrap().contains("[REDACTED:github_token"));

        let url = format!("data:image/png;base64,{data}");
        let out = redact_str(&format!("![shot]({url}) and {GH}"));
        assert!(out.starts_with(&format!("![shot]({url}) and [REDACTED:github_token")), "{out}");
    }

    #[test]
    fn blob_refs_and_inline_data_parts_are_left_alone() {
        let data = image_b64();
        let doc = json!({
            "ref": {"type": "cctui-blob", "blob_id": data, "size": 3, "encoding": "base64"},
            "part": {"inlineData": {"mimeType": "image/png", "data": data}},
            "data": GH,
        });
        let mut v = doc.clone();
        redact_json(&mut v, &p());
        assert_eq!(v["ref"], doc["ref"]);
        assert_eq!(v["part"], doc["part"]);
        assert!(v["data"].as_str().unwrap().starts_with("[REDACTED:github_token"), "plain data");
    }

    fn gh() -> String {
        format!("{}{}", "gh", "p_ABCDEFGHIJKLMNOPQRSTUVWX0123")
    }

    fn image_block(data: &str) -> Value {
        json!({"type": "image", "source": {
            "type": "base64", "media_type": "image/png", "data": data,
        }})
    }

    #[test]
    fn stream_event_images_keep_base64_byte_identical_while_sibling_text_is_redacted() {
        let data = image_b64();
        let secret = gh();
        let mut v = json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "text", "text": format!("look {secret}")},
                image_block(&data),
                {"type": "tool_result", "tool_use_id": "toolu_02", "content": [
                    {"type": "text", "text": format!("shot taken {secret}")},
                    image_block(&data),
                ]},
            ]},
        });
        assert!(redact_json(&mut v, &p()) >= 2);
        let content = &v["message"]["content"];
        assert_eq!(content[1]["source"]["data"].as_str(), Some(data.as_str()));
        assert_eq!(content[2]["content"][1]["source"]["data"].as_str(), Some(data.as_str()));
        let top = content[0]["text"].as_str().unwrap();
        assert!(top.starts_with("look [REDACTED:github_token"), "{top}");
        let nested = content[2]["content"][0]["text"].as_str().unwrap();
        assert!(nested.starts_with("shot taken [REDACTED:github_token"), "{nested}");
    }

    #[test]
    fn untagged_image_source_keeps_base64() {
        let data = image_b64();
        let mut v = json!({"type": "image", "source": {"media_type": "image/png", "data": data}});
        redact_json(&mut v, &p());
        assert_eq!(v["source"]["data"].as_str(), Some(data.as_str()));
    }

    #[test]
    fn stringified_mcp_image_without_base64_marker_keeps_data() {
        let data = image_b64();
        let secret = gh();
        let blocks = json!([
            {"type": "text", "text": format!("ok {secret}")},
            {"type": "image", "data": data, "mimeType": "image/png"},
        ])
        .to_string();
        assert!(!blocks.contains("base64"), "fixture must not carry the base64 marker");
        let mut v = json!({ "output_summary": blocks });
        redact_json(&mut v, &p());
        let out: Value = serde_json::from_str(v["output_summary"].as_str().unwrap()).unwrap();
        assert_eq!(out[1]["data"].as_str(), Some(data.as_str()));
        assert!(out[0]["text"].as_str().unwrap().starts_with("ok [REDACTED:github_token"));
    }

    #[test]
    fn samples_skip_image_data() {
        let data = image_b64();
        let mut out = BTreeMap::new();
        collect_samples(&image_result(&data), &p(), &mut out);
        let s = json!({"s": format!("data:image/jpeg;base64,{data}")});
        collect_samples(&s, &p(), &mut out);
        assert!(!out.contains_key("square_token"), "{out:?}");
        assert!(out.contains_key("github_token"));
    }

    /// Deterministic pseudo-random bytes, so no secret-shaped literal sits in the source.
    fn rand_bytes(seed: &str, len: usize) -> Vec<u8> {
        use sha2::Digest;
        let mut out = Vec::with_capacity(len + 32);
        let mut block = Sha256::digest(seed.as_bytes());
        while out.len() < len {
            out.extend_from_slice(&block[..]);
            block = Sha256::digest(&block[..]);
        }
        out.truncate(len);
        out
    }

    fn b64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, &b)| acc | (u32::from(b) << (16 - 8 * i)));
            for i in 0..=chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            }
            for _ in chunk.len()..3 {
                out.push('=');
            }
        }
        out
    }

    fn cipher_string(seed: &str) -> String {
        let part = |s: &str, n| b64(&rand_bytes(&format!("{seed}-{s}"), n));
        format!("2.{}|{}|{}", part("iv", 16), part("ct", 32), part("mac", 32))
    }

    fn otpauth_fixture() -> String {
        let secret = "JBSWY3DP".repeat(4);
        format!("{}://totp/Acme:alice@example.com?secret={secret}&issuer=Acme", "otpauth")
    }

    fn vault_item_json() -> String {
        json!({
            "object": "item",
            "id": "3f2a8c1e-9b4d-4e7a-8f6b-2c1d0e9a7b3c",
            "type": 1,
            "name": cipher_string("name"),
            "login": {
                "username": cipher_string("user"),
                "password": cipher_string("pass"),
                "totp": cipher_string("totp"),
            },
            "notes": null,
        })
        .to_string()
    }

    #[test]
    fn base64_password_manager_payload_is_flagged() {
        let payload = b64(vault_item_json().as_bytes());
        let out = redact_str(&format!("Error: apply failed for item payload {payload}\nexit 1"));
        assert!(out.contains("[REDACTED:high_entropy_base64:"), "{out}");
        assert!(out.starts_with("Error: apply failed for item payload ["), "{out}");
        assert!(out.ends_with("\nexit 1"), "{out}");
        for chunk in payload.as_bytes().chunks(16) {
            let chunk = std::str::from_utf8(chunk).unwrap();
            assert!(chunk.len() < 16 || !out.contains(chunk), "leaked {chunk}: {out}");
        }
    }

    #[test]
    fn plain_password_manager_item_masks_every_encrypted_field() {
        let mut v: Value = serde_json::from_str(&vault_item_json()).unwrap();
        redact_json(&mut v, &p());
        assert!(v["name"].as_str().unwrap().starts_with("[REDACTED:vault_cipher_string:"), "{v}");
        assert!(v["login"]["username"].as_str().unwrap().starts_with("[REDACTED:"), "{v}");
        assert!(v["login"]["totp"].as_str().unwrap().starts_with("[REDACTED:"), "{v}");
        assert_eq!(v["id"], json!("3f2a8c1e-9b4d-4e7a-8f6b-2c1d0e9a7b3c"), "uuid rewritten");
        assert!(!v.to_string().contains('|'), "{v}");
    }

    #[test]
    fn hex_secret_run_is_flagged() {
        let secret = hex::encode(rand_bytes("hex-secret", 24));
        let out = redact_str(&format!("decrypt with key {secret} failed"));
        assert!(out.starts_with("decrypt with key [REDACTED:high_entropy_hex:"), "{out}");
        assert!(out.ends_with(" failed"), "{out}");
        assert!(!out.contains(&secret[..16]), "{out}");
    }

    #[test]
    fn otpauth_uri_secret_is_flagged() {
        let uri = otpauth_fixture();
        let out = redact_str(&format!("scan this: {uri} then confirm"));
        assert!(out.starts_with("scan this: otpauth://totp/[REDACTED:otpauth_uri:"), "{out}");
        assert!(out.ends_with(" then confirm"), "{out}");
        assert!(!out.contains("JBSWY3DP"), "{out}");
        assert_eq!(redact_str(&out), out, "rescrub must be idempotent");
    }

    fn pem_body(seed: &str) -> String {
        (0..3).map(|i| b64(&rand_bytes(&format!("{seed}{i}"), 48))).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn pem_private_key_blocks_are_flagged_including_pgp_and_truncated() {
        let body = pem_body("pem");
        let k = ["PRIVATE", " KEY BLOCK"].concat();
        let pgp = format!("-----BEGIN PGP {k}-----\n\n{body}\n=AbCd\n-----END PGP {k}-----");
        let truncated = format!("-----BEGIN OPENSSH {} KEY-----\n{body}", "PRIVATE");
        for pem in [pgp, truncated] {
            let out = redact_str(&format!("cat key:\n{pem}\n... (output truncated)"));
            assert!(out.contains("[REDACTED:private_key]"), "{out}");
            assert!(out.ends_with("... (output truncated)"), "{out}");
            assert!(!out.contains(&body[..24]), "{out}");
        }
    }

    #[test]
    fn private_key_sample_does_not_leak_the_body() {
        let body = pem_body("sample");
        let k = ["PRIVATE", " KEY"].concat();
        let pem = format!("-----BEGIN RSA {k}-----\n{body}==\n-----END RSA {k}-----");
        let mut out = BTreeMap::new();
        collect_samples(&json!({ "out": pem }), &p(), &mut out);
        assert!(out.contains_key("private_key"), "{out:?}");
        let dumped = format!("{out:?}");
        for line in body.lines() {
            assert!(!dumped.contains(&line[..16]), "leaked {line}: {dumped}");
        }
    }

    #[test]
    fn digests_ids_paths_and_public_base64_are_not_flagged() {
        let sha1 = hex::encode(rand_bytes("git", 20));
        let sha256 = hex::encode(rand_bytes("sha256", 32));
        let sha512 = hex::encode(rand_bytes("sha512", 64));
        let labelled_hex = hex::encode(rand_bytes("labelled", 30));
        let sri384 = b64(&rand_bytes("sri384", 48));
        let sri512 = b64(&rand_bytes("sri512", 64));
        let cert = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
            pem_body("cert")
        );
        let benign = [
            "session 3f2a8c1e-9b4d-4e7a-8f6b-2c1d0e9a7b3c resumed".to_owned(),
            format!("commit {sha1}\nAuthor: someone"),
            format!("{sha256}  target/release/cctui-server"),
            format!("image@sha256:{sha256}"),
            format!("sha512sum: {sha512}"),
            format!("md5={labelled_hex}"),
            "/home/user/Documents/projects/cctui/crates/cctui-crypto/src/redact/entropy.rs:42"
                .to_owned(),
            "/nix/store/0c5a9f2kq8zx1m3n7b4v6d2h8j1l0p9r-openssl-3.0.13/lib".to_owned(),
            format!(r#"<script src="app.js" integrity="sha384-{sri384}" crossorigin></script>"#),
            format!(r#""integrity": "sha512-{sri512}","#),
            "const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';"
                .to_owned(),
            "fn digests_ids_paths_and_public_base64_are_not_flagged_by_the_entropy_rules()"
                .to_owned(),
            "impl ConversationComposerVoiceControlsHarnessBuilder for X {}".to_owned(),
            cert,
        ];
        for text in &benign {
            assert_eq!(&redact_str(text), text, "false positive");
        }
    }

    #[test]
    fn image_block_base64_stays_byte_identical_with_entropy_detectors_on() {
        let data = b64(&rand_bytes("png", 3000));
        assert!(
            redact_str(&data).contains("[REDACTED:high_entropy_base64"),
            "fixture must be something the detector flags as plain text"
        );
        let mut v = image_result(&data);
        redact_json(&mut v, &p());
        assert_eq!(v["content"][1]["source"]["data"].as_str(), Some(data.as_str()));

        let summary = image_result(&data)["content"].to_string();
        let mut stringified = json!({ "output_summary": summary });
        redact_json(&mut stringified, &p());
        let blocks: Value =
            serde_json::from_str(stringified["output_summary"].as_str().unwrap()).unwrap();
        assert_eq!(blocks[1]["source"]["data"].as_str(), Some(data.as_str()));

        let mcp_blocks = json!([{"type": "image", "data": data, "mimeType": "image/png"}]);
        let mut mcp = json!({ "output_summary": mcp_blocks.to_string() });
        redact_json(&mut mcp, &p());
        let blocks: Value = serde_json::from_str(mcp["output_summary"].as_str().unwrap()).unwrap();
        assert_eq!(blocks[0]["data"].as_str(), Some(data.as_str()));

        let url = format!("data:image/png;base64,{data}");
        assert_eq!(redact_str(&format!("![shot]({url})")), format!("![shot]({url})"));

        let mut out = BTreeMap::new();
        collect_samples(&image_result(&data), &p(), &mut out);
        assert!(!out.contains_key("high_entropy_base64"), "{out:?}");
    }

    #[test]
    fn allowlisted_value_is_left_alone() {
        let vetted = hex::encode(rand_bytes("vetted", 24));
        let other = hex::encode(rand_bytes("other", 24));
        let pats = compile(true, &[], KEY).with_allowlist([format!(" {vetted} ")]);
        let mut v = json!(format!("{vetted} {other}"));
        assert_eq!(redact_json(&mut v, &pats), 1);
        let out = v.as_str().unwrap();
        assert!(out.starts_with(&format!("{vetted} [REDACTED:high_entropy_hex:")), "{out}");
        let mut samples = BTreeMap::new();
        collect_samples(&json!(vetted), &pats, &mut samples);
        assert!(samples.is_empty(), "{samples:?}");
    }

    #[test]
    fn dry_run_reports_masked_samples_per_new_category() {
        let payload = b64(vault_item_json().as_bytes());
        let hex_secret = hex::encode(rand_bytes("dry-run-hex", 24));
        let doc = json!({
            "a": format!("apply failed: {payload}"),
            "b": format!("key {hex_secret} rejected"),
            "c": format!("enroll {}", otpauth_fixture()),
            "d": cipher_string("dry-run"),
        });
        let mut samples = BTreeMap::new();
        collect_samples(&doc, &p(), &mut samples);
        let mut v = doc.clone();
        let stats = redact_json_stats(&mut v, &p());
        let cats =
            ["high_entropy_base64", "high_entropy_hex", "otpauth_uri", "vault_cipher_string"];
        for cat in cats {
            assert_eq!(stats.get(cat), Some(&1), "{cat}: {stats:?}");
            assert_eq!(samples.get(cat).map(|s| s.samples.len()), Some(1), "{cat}: {samples:?}");
        }
        let dumped = format!("{samples:?}");
        for secret in [&payload[8..40], &hex_secret[8..40], "JBSWY3DP"] {
            assert!(!dumped.contains(secret), "sample leaked {secret}: {dumped}");
        }
    }
}
