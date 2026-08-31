//! Emoji prefix for auto-generated session names (opt-in, per user).
//!
//! cctui does not generate session titles: they come from the agent itself
//! (the claude binary writes `name` into its `state.json`, codex reports
//! `thread/name/updated`) and reach us on the `Status` event. There is
//! therefore no naming prompt to ask for an emoji — instead, when the owning
//! user has `sessionEmojiPrefix` enabled in their settings, we decorate the
//! name on ingestion, deterministically, from the words it already contains.
//!
//! Two paths produce the emoji:
//!
//! * **A small model**, when `CCTUI_EMOJI_ENDPOINT` + `CCTUI_EMOJI_MODEL` are
//!   configured — one short OpenAI-compatible call per *new* name, off the
//!   status path, so the emoji is genuinely chosen for the subject.
//! * **A deterministic keyword table** otherwise, and whenever the call fails,
//!   times out or answers with something that is not a single emoji. It costs
//!   nothing, never blocks, and keeps the feature working for adapters and
//!   deployments with no model wired up.
//!
//! The table result is written first, so the name is decorated immediately; the
//! model refines it a moment later when it has something better.

/// Keyword stems (English + French) mapped to the emoji they select. Scanned in
/// order, first hit wins, so the more specific families come first: `docker`
/// must be tested before `doc`, `migration` before the generic `mig`-less
/// database words, and so on.
const FAMILIES: &[(&str, &[&str])] = &[
    ("🐳", &["docker", "kube", "k8s", "helm", "cluster", "container", "conteneur", "pod"]),
    (
        "🔐",
        &[
            "security",
            "securit",
            "auth",
            "secret",
            "token",
            "vuln",
            "cert",
            "tls",
            "ssl",
            "crypto",
            "chiffr",
            "oauth",
            "password",
            "passwd",
            "credential",
        ],
    ),
    (
        "🪲",
        &[
            "bug",
            "bogue",
            "fix",
            "hotfix",
            "crash",
            "regression",
            "correctif",
            "corrige",
            "corrig",
            "panne",
            "broken",
            "casse",
            "debug",
            "repair",
            "repar",
        ],
    ),
    (
        "🔎",
        &[
            "research",
            "recherche",
            "investigat",
            "enquet",
            "explor",
            "search",
            "audit",
            "analys",
            "diagnostic",
            "review",
            "revue",
            "inspect",
            "compare",
            "comparai",
        ],
    ),
    (
        "🧹",
        &[
            "refactor", "refacto", "cleanup", "clean", "tidy", "refonte", "nettoy", "menage",
            "dedup",
        ],
    ),
    (
        "📝",
        &[
            "doc",
            "docs",
            "documentation",
            "readme",
            "changelog",
            "spec",
            "redaction",
            "redig",
            "note",
            "notes",
            "article",
            "blog",
            "rapport",
        ],
    ),
    ("🧪", &["test", "tests", "coverage", "e2e", "unit", "pytest", "vitest", "recette"]),
    (
        "🚀",
        &[
            "deploy",
            "deploi",
            "release",
            "ship",
            "rollout",
            "publish",
            "publie",
            "prod",
            "production",
            "livraison",
            "livre",
        ],
    ),
    (
        "🗄️",
        &[
            "database",
            "postgres",
            "sqlite",
            "sql",
            "schema",
            "migration",
            "migrate",
            "index",
            "query",
            "requete",
        ],
    ),
    (
        "🎨",
        &[
            "ui",
            "ux",
            "css",
            "style",
            "design",
            "theme",
            "front",
            "frontend",
            "layout",
            "interface",
            "maquette",
            "couleur",
            "color",
            "icon",
            "logo",
        ],
    ),
    (
        "⚡",
        &["perf", "performance", "optimi", "speed", "latency", "latence", "slow", "lent", "bench"],
    ),
    (
        "⚙️",
        &[
            "config",
            "setup",
            "install",
            "ci",
            "pipeline",
            "build",
            "workflow",
            "script",
            "cron",
            "infra",
            "provision",
            "upgrade",
            "bump",
            "update",
            "maj",
        ],
    ),
    (
        "✨",
        &[
            "feat",
            "feature",
            "implement",
            "implemente",
            "ajout",
            "ajoute",
            "nouveau",
            "nouvelle",
            "create",
            "creer",
            "cree",
            "support",
            "enable",
            "porte",
        ],
    ),
];

/// Emoji used when nothing in the name matches a known family.
const FALLBACK: &str = "💬";

/// True when `name` already opens with a pictograph — an agent title that
/// carries its own emoji, or a name we have already decorated. Keeps the
/// prefixing idempotent across the repeated `Status` events that re-send the
/// same name.
///
/// The cut-off is `U+2190` (arrows and everything above: symbols, dingbats,
/// emoji planes), which sits well above every accented Latin letter, so a
/// French title like `Étude` is not mistaken for an emoji.
#[must_use]
pub fn starts_with_emoji(name: &str) -> bool {
    name.trim_start().chars().next().is_some_and(|c| c as u32 >= 0x2190)
}

/// Lowercase and strip the common French accents so `sécurité` matches the
/// `securit` stem. Non-letters become word separators.
fn fold(name: &str) -> String {
    name.chars()
        // `to_lowercase` first: it maps `É` to `é`, which the accent table
        // below then folds to `e`. Doing it the other way round would leave
        // uppercase accents unmatched.
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'å' | 'ã' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'ì' | 'í' => 'i',
            'ô' | 'ö' | 'ò' | 'ó' | 'õ' | 'œ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ÿ' => 'y',
            'ñ' => 'n',
            // Everything that is not a plain ASCII letter/digit becomes a word
            // separator, so punctuation and emoji never glue two words.
            other if other.is_ascii_alphanumeric() => other,
            _ => ' ',
        })
        .collect()
}

/// Pick the emoji for a session name: the first family with a stem that one of
/// the name's words starts with, else [`FALLBACK`].
///
/// Matching is per word and prefix-anchored, so `fix` hits `fixes` but not
/// `prefix`, and a stem can absorb a whole inflection family (`optimi` covers
/// `optimize`, `optimisation`, `optimiser`).
#[must_use]
pub fn emoji_for(name: &str) -> &'static str {
    let folded = fold(name);
    let words: Vec<&str> = folded.split_whitespace().collect();
    for (emoji, stems) in FAMILIES {
        if words.iter().any(|w| stems.iter().any(|s| w.starts_with(s))) {
            return emoji;
        }
    }
    FALLBACK
}

/// Return `name` with an emoji prefix, or `None` when there is nothing to do:
/// an empty name, or one that already starts with a pictograph.
#[must_use]
pub fn decorate(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || starts_with_emoji(trimmed) {
        return None;
    }
    Some(format!("{} {trimmed}", emoji_for(trimmed)))
}

/// Strip a leading pictograph (and the spaces after it) from `name`.
///
/// Lets the caller compare a stored, already-decorated name against the plain
/// name the agent keeps re-reporting: equal after stripping means this exact
/// name was already handled, so nothing is re-decorated and no model call is
/// made for it twice.
#[must_use]
pub fn strip_emoji(name: &str) -> &str {
    let trimmed = name.trim();
    if !starts_with_emoji(trimmed) {
        return trimmed;
    }
    // Drop the whole leading run of non-letter symbols: one emoji may be
    // several chars (ZWJ sequences, skin tones, variation selectors).
    let rest = trimmed.trim_start_matches(|c: char| (c as u32) >= 0x2190 || c == '\u{200d}');
    rest.trim_start()
}

/// Accept a model's answer only when it is exactly one emoji.
///
/// Anything else — a word, a sentence, several emoji, an ASCII smiley — is
/// rejected so a chatty model can never write prose into a session name; the
/// caller then keeps the table's pick.
#[must_use]
pub fn single_emoji(reply: &str) -> Option<String> {
    let s = reply.trim();
    if s.is_empty() || !starts_with_emoji(s) {
        return None;
    }
    let mut bases = 0usize;
    let mut joiners = 0usize;
    for c in s.chars() {
        let u = c as u32;
        match u {
            // Zero-width joiner: glues two bases into one glyph (👨‍💻).
            0x200d => joiners += 1,
            // Variation selectors and skin-tone modifiers ride on a base.
            0xFE00..=0xFE0F | 0x1F3FB..=0x1F3FF => {}
            // Any other pictograph is a base of its own.
            u if u >= 0x2190 => bases += 1,
            // A letter, digit, space or punctuation means this is not a lone emoji.
            _ => return None,
        }
    }
    // One base, or several fused by a joiner each — anything else is two emoji
    // side by side, which we refuse rather than paste into a name.
    (bases >= 1 && joiners + 1 == bases || bases == 1).then(|| s.to_owned())
}

/// A configured small-model emoji picker (see `Config::emoji_picker`).
#[derive(Clone, Copy)]
pub struct Picker<'a> {
    pub endpoint: &'a str,
    pub model: &'a str,
    pub token: Option<&'a str>,
}

/// What we ask the model. Kept blunt on purpose: the reply is validated by
/// [`single_emoji`], so an instruction-following slip costs a fallback, not a
/// broken name.
const PROMPT: &str = "You label work sessions with a single emoji. \
Reply with exactly one emoji character that fits the subject of the title, and nothing else. \
No words, no punctuation, no explanation.";

/// Ask the configured model for the emoji that fits `title`.
///
/// Returns `None` on any problem at all (no endpoint, HTTP error, timeout, a
/// reply that is not a lone emoji) — every one of which simply leaves the
/// table's emoji in place.
pub async fn pick_with_model(
    client: &reqwest::Client,
    picker: Picker<'_>,
    title: &str,
) -> Option<String> {
    let body = serde_json::json!({
        "model": picker.model,
        "max_tokens": 8,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": PROMPT },
            { "role": "user", "content": title },
        ],
    });
    let mut req = client
        .post(format!("{}/chat/completions", picker.endpoint.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(10))
        .json(&body);
    if let Some(token) = picker.token {
        req = req.bearer_auth(token);
    }
    let resp = match req.send().await {
        Ok(resp) => resp,
        Err(e) => {
            // Worth a warn: a misconfigured endpoint is otherwise invisible,
            // the name just silently keeps the table's emoji.
            tracing::warn!(error = %e, "emoji picker: request failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "emoji picker: non-success reply");
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    emoji_from_reply(&json)
}

/// Pull the emoji out of an OpenAI-compatible chat completion, validating it.
/// Split out from the request so the reply handling is unit-testable without a
/// server. Any shape we do not recognize yields `None` (table keeps its pick).
#[must_use]
fn emoji_from_reply(json: &serde_json::Value) -> Option<String> {
    let choice = json.get("choices")?.as_array()?.first()?;
    let content = choice.get("message")?.get("content")?;
    // Most servers answer with a plain string; some send the content-parts array.
    let text = match content.as_str() {
        Some(t) => t.to_owned(),
        None => content
            .as_array()?
            .iter()
            .filter_map(|p| p.get("text").and_then(serde_json::Value::as_str))
            .collect::<String>(),
    };
    single_emoji(&text)
}

#[cfg(test)]
mod tests {
    use super::{decorate, emoji_for, starts_with_emoji};

    #[test]
    fn picks_the_family_emoji_in_both_languages() {
        assert_eq!(emoji_for("Bug fix projet XY"), "🪲");
        assert_eq!(emoji_for("Recherche bidule machin"), "🔎");
        assert_eq!(emoji_for("Rollout cctui 0.7.279"), "🚀");
        assert_eq!(emoji_for("Refactor the dispatcher"), "🧹");
        assert_eq!(emoji_for("Écrire la documentation"), "📝");
        assert_eq!(emoji_for("Sécurité du proxy"), "🔐");
        assert_eq!(emoji_for("Optimisation du rendu"), "⚡");
        assert_eq!(emoji_for("Nouvelle feature spawn"), "✨");
    }

    #[test]
    fn matches_whole_words_only() {
        // `prefix` must not trip the `fix` stem, nor `docker` the `doc` one.
        assert_eq!(emoji_for("Prefix handling"), "💬");
        assert_eq!(emoji_for("Docker image build"), "🐳");
    }

    #[test]
    fn unknown_subject_falls_back() {
        assert_eq!(emoji_for("Bidule machin chose"), "💬");
        assert_eq!(emoji_for(""), "💬");
    }

    #[test]
    fn decorate_is_idempotent() {
        let once = decorate("Bug fix projet XY").unwrap();
        assert_eq!(once, "🪲 Bug fix projet XY");
        assert_eq!(decorate(&once), None);
    }

    #[test]
    fn leaves_an_agent_supplied_emoji_alone() {
        assert!(starts_with_emoji("🐛 already tagged"));
        assert_eq!(decorate("🐛 already tagged"), None);
        // An accented capital is a letter, not an emoji.
        assert!(!starts_with_emoji("Étude des performances"));
        assert_eq!(
            decorate("Étude des performances").as_deref(),
            Some("⚡ Étude des performances")
        );
    }

    #[test]
    fn reply_parsing_handles_both_content_shapes_and_junk() {
        use serde_json::json;
        let plain = json!({"choices":[{"message":{"content":"🪲"}}]});
        assert_eq!(super::emoji_from_reply(&plain).as_deref(), Some("🪲"));

        let parts = json!({"choices":[{"message":{"content":[{"type":"text","text":" 🔎 "}]}}]});
        assert_eq!(super::emoji_from_reply(&parts).as_deref(), Some("🔎"));

        // A chatty model, an error body and an empty choice list all fall back.
        let chatty = json!({"choices":[{"message":{"content":"Sure! 🪲"}}]});
        assert_eq!(super::emoji_from_reply(&chatty), None);
        assert_eq!(super::emoji_from_reply(&json!({"error":"nope"})), None);
        assert_eq!(super::emoji_from_reply(&json!({"choices":[]})), None);
    }

    #[test]
    fn strip_emoji_recovers_the_plain_name() {
        assert_eq!(super::strip_emoji("🪲 Bug fix projet XY"), "Bug fix projet XY");
        assert_eq!(super::strip_emoji("Bug fix projet XY"), "Bug fix projet XY");
        // A ZWJ sequence is one emoji, not three.
        assert_eq!(super::strip_emoji("👨‍💻 Refactor"), "Refactor");
        // An accented capital must not be eaten as a pictograph.
        assert_eq!(super::strip_emoji("Étude des performances"), "Étude des performances");
    }

    #[test]
    fn only_a_lone_emoji_is_accepted_from_the_model() {
        assert_eq!(super::single_emoji(" 🪲 ").as_deref(), Some("🪲"));
        assert_eq!(super::single_emoji("👨‍💻").as_deref(), Some("👨‍💻"));
        assert_eq!(super::single_emoji("🪲 bug"), None);
        assert_eq!(super::single_emoji("bug"), None);
        assert_eq!(super::single_emoji("🪲🔎"), None);
        assert_eq!(super::single_emoji(":-)"), None);
        assert_eq!(super::single_emoji(""), None);
    }

    #[test]
    fn empty_and_whitespace_names_are_left_alone() {
        assert_eq!(decorate(""), None);
        assert_eq!(decorate("   "), None);
    }
}
