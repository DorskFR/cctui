/// `toFixed`, not `{:.N}`.
///
/// `{:.N}` rounds a tie to even and `Number.prototype.toFixed` rounds it away
/// from zero, so every Rust mirror of a `toFixed` has to come through here:
/// `compact(1250)` is "1.3k" on the web and must not be "1.2k" in the TUI.
/// Rounds the f64's exact decimal expansion, as `toFixed` does, so a value only
/// *near* a tie still rounds the way its binary value leans. Values at or above
/// 1e21, where `toFixed` switches to exponential form, are not mirrored — no
/// token count or price reaches them.
#[must_use]
pub fn js_to_fixed(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return format!("{v:.digits$}");
    }
    let negative = v < 0.0;
    // 25 extra digits reach past any tie a double can represent here, so the
    // digit that decides the rounding is exact rather than itself rounded.
    let exact = format!("{:.*}", digits + 25, v.abs());
    let (whole, frac) = exact.split_once('.').unwrap_or((exact.as_str(), ""));
    let mut kept: Vec<u8> =
        whole.bytes().chain(frac.bytes().take(digits)).map(|b| b - b'0').collect();
    let mut int_len = whole.len();
    if frac.as_bytes().get(digits).is_some_and(|b| *b >= b'5') {
        let mut at = kept.len();
        loop {
            if at == 0 {
                kept.insert(0, 1);
                int_len += 1;
                break;
            }
            at -= 1;
            if kept[at] == 9 {
                kept[at] = 0;
            } else {
                kept[at] += 1;
                break;
            }
        }
    }
    let text: String = kept.iter().map(|d| char::from(b'0' + d)).collect();
    let sign = if negative { "-" } else { "" };
    if digits == 0 {
        return format!("{sign}{}", &text[..int_len]);
    }
    format!("{sign}{}.{}", &text[..int_len], &text[int_len..])
}

const FAMILIES: [&str; 10] =
    ["opus", "sonnet", "haiku", "fable", "gpt", "o1", "o3", "o4", "gemini", "grok"];

const CODENAMES: [&str; 4] = ["sol", "terra", "luna", "astra"];

struct Tier {
    div: f64,
    suffix: char,
}

const TIERS: [Tier; 3] = [
    Tier { div: 1e9, suffix: 'B' },
    Tier { div: 1e6, suffix: 'M' },
    Tier { div: 1e3, suffix: 'k' },
];

const fn tier_digits(index: usize, value: f64) -> usize {
    if index == 2 && value >= 10.0 { 0 } else { 1 }
}

/// A value that rounds up to 1000 in its own tier is promoted to the next one,
/// so no unit ever displays "1000".
#[must_use]
pub fn compact(n: f64) -> String {
    if n < 1000.0 {
        return trim_number(n);
    }
    let mut i = TIERS.iter().position(|t| n >= t.div).unwrap_or(TIERS.len() - 1);
    let mut v = n / TIERS[i].div;
    let rounded: f64 = js_to_fixed(v, tier_digits(i, v)).parse().unwrap_or(v);
    if rounded >= 1000.0 && i > 0 {
        i -= 1;
        v = n / TIERS[i].div;
    }
    format!("{}{}", js_to_fixed(v, tier_digits(i, v)), TIERS[i].suffix)
}

#[must_use]
pub fn uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        return format!("{d}d {h}h");
    }
    if h > 0 {
        return format!("{h}h {m}m");
    }
    format!("{m}m")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeTone {
    Ok,
    Info,
    Danger,
    Neutral,
}

impl BadgeTone {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Danger => "danger",
            Self::Neutral => "neutral",
        }
    }
}

#[must_use]
pub fn status_badge_tone(status: &str) -> BadgeTone {
    match status {
        "active" => BadgeTone::Ok,
        "new" => BadgeTone::Info,
        "archived" => BadgeTone::Danger,
        _ => BadgeTone::Neutral,
    }
}

#[must_use]
pub fn model_short(model: &str) -> String {
    let leaf = model.split('/').rfind(|s| !s.is_empty()).unwrap_or(model);
    let lower = leaf.to_lowercase();
    for prefix in ["claude-", "anthropic-"] {
        if lower.starts_with(prefix) {
            return leaf[prefix.len()..].to_string();
        }
    }
    leaf.to_string()
}

#[must_use]
pub fn model_family(model: &str) -> String {
    let m = model.to_lowercase();
    if let Some(fam) = FAMILIES.iter().copied().find(|f| m.contains(f)) {
        return fam.to_string();
    }
    let short = model_short(model);
    let head = short.split(['-', ' ', '\t', '\n']).next().unwrap_or("");
    if head.is_empty() { model.to_string() } else { head.to_string() }
}

/// Shortest prefix, two letters up, unique across the family + codename
/// vocabulary: Sonnet and Sol both start "so" and would otherwise collide.
fn distinct_prefix(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let vocabulary: Vec<&str> = FAMILIES.iter().chain(CODENAMES.iter()).copied().collect();
    let mut n = 2;
    while n < chars.len() {
        let head: String = chars[..n].iter().collect();
        let clash = vocabulary.iter().any(|w| *w != word && w.starts_with(&head));
        if !clash {
            break;
        }
        n += 1;
    }
    chars[..n.min(chars.len())].iter().collect()
}

/// A codename only counts when it is a whole word; "solaris" is not Sol.
fn standalone_word(haystack: &str, needle: &str) -> bool {
    let bytes: Vec<char> = haystack.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    if pat.len() > bytes.len() {
        return false;
    }
    (0..=bytes.len() - pat.len()).any(|i| {
        bytes[i..i + pat.len()] == pat[..]
            && (i == 0 || !bytes[i - 1].is_ascii_lowercase())
            && (i + pat.len() == bytes.len() || !bytes[i + pat.len()].is_ascii_lowercase())
    })
}

#[must_use]
pub fn model_abbrev(model: &str) -> String {
    let m = model.to_lowercase();
    let word = CODENAMES
        .iter()
        .copied()
        .find(|c| standalone_word(&m, c))
        .map_or_else(|| model_family(model), str::to_string);
    if word == "gpt" {
        return "GPT".to_string();
    }
    let prefix = distinct_prefix(&word);
    let mut out = String::new();
    for (i, c) in prefix.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out.push('.');
    out
}

/// The machine badge's smallest legible form: first alphanumeric, plus any
/// trailing number (unpadded) so a numbered fleet stays distinguishable.
#[must_use]
pub fn machine_initial(label: &str) -> String {
    let Some(head) = label.chars().find(char::is_ascii_alphanumeric) else {
        return "?".to_string();
    };
    let trailing: Vec<char> =
        label.trim_end().chars().rev().take_while(char::is_ascii_digit).collect();
    let digits: String = trailing.iter().rev().collect();
    let head: String = head.to_uppercase().collect();
    if digits.is_empty() {
        return head;
    }
    let n = digits.trim_start_matches('0');
    format!("{head}{}", if n.is_empty() { "0" } else { n })
}

#[must_use]
pub fn usd(n: f64) -> String {
    if !n.is_finite() || n <= 0.0 {
        return "$0.00".to_string();
    }
    if n < 0.01 {
        return format!("${}", js_to_fixed(n, 4));
    }
    if n < 100.0 {
        return format!("${}", js_to_fixed(n, 2));
    }
    format!("${}", n.round() as i64)
}

/// Deterministic accent hue for a label. Hashes UTF-16 code units, because the
/// `TypeScript` original reads `charCodeAt`.
#[must_use]
pub fn hash_hue(s: &str) -> u32 {
    let mut h: u32 = 0;
    for unit in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    h % 360
}

/// Inline tint: the theme supplies only `<sat%> <light%>` pairs, so the hue has
/// to be set on the same element that reads them.
#[must_use]
pub fn hue_tint(hue: u32) -> String {
    format!(
        "--mh:{hue};background:hsl(var(--mh) var(--mach-bg-sl));color:hsl(var(--mh) var(--mach-fg-sl));border-color:hsl(var(--mh) var(--mach-border-sl))"
    )
}

#[must_use]
pub fn machine_tint(label: &str, hue: Option<u32>) -> String {
    hue_tint(hue.unwrap_or_else(|| hash_hue(label)))
}

/// Render like `JavaScript`'s `String(n)` for the integral values these counters
/// carry.
fn trim_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        return format!("{}", n as i64);
    }
    format!("{n}")
}

#[cfg(test)]
mod tie_rounding {
    use super::{compact, js_to_fixed, usd};

    /// The right-hand sides are what V8's `toFixed` prints for the same inputs.
    #[test]
    fn js_to_fixed_matches_tofixed() {
        for (value, digits, want) in [
            (1.25_f64, 1, "1.3"),
            (0.125, 2, "0.13"),
            (2.5, 0, "3"),
            (3.5, 0, "4"),
            (1.5, 0, "2"),
            (0.0625, 4, "0.0625"),
            (1.005, 2, "1.00"),
            (9.95, 1, "9.9"),
            (8.345, 2, "8.35"),
            (99.995, 2, "100.00"),
            (0.1, 2, "0.10"),
            (0.0, 2, "0.00"),
            (-0.125, 2, "-0.13"),
        ] {
            assert_eq!(js_to_fixed(value, digits), want, "js_to_fixed({value}, {digits})");
        }
    }

    #[test]
    fn ties_round_up_as_the_web_rounds_them() {
        assert_eq!(compact(1250.0), "1.3k");
        assert_eq!(compact(1_250_000.0), "1.3M");
        assert_eq!(compact(1_250_000_000.0), "1.3B");
        assert_eq!(usd(0.125), "$0.13");
        assert_eq!(usd(0.001_25), "$0.0013");
    }
}
