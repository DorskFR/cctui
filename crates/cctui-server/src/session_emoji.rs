//! Emoji prefix for auto-generated session names (opt-in, per user).
//!
//! cctui does not generate session titles: they come from the agent itself
//! (the claude binary writes `name` into its `state.json`, codex reports
//! `thread/name/updated`) and reach us on the `Status` event. There is
//! therefore no naming prompt to ask for an emoji — instead, when the owning
//! user has `sessionEmojiPrefix` enabled in their settings, we decorate the
//! name on ingestion, deterministically, from the words it already contains.
//!
//! Deterministic beats a model call here: no token cost, no latency on the
//! status path, no account to bill, and the mapping is unit-testable.

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
    fn empty_and_whitespace_names_are_left_alone() {
        assert_eq!(decorate(""), None);
        assert_eq!(decorate("   "), None);
    }
}
