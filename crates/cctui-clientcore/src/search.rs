const MAX_TERMS: usize = 8;

/// Whitespace-split into terms, but a `"…"`-quoted span stays a single exact
/// term (spaces preserved).
///
/// Mirrors the server's tokenizer; terms are AND-matched server-side and
/// highlighted client-side with the same split.
#[must_use]
pub fn tokenize_query(q: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in q.chars() {
        if c == '"' {
            if !cur.is_empty() {
                terms.push(std::mem::take(&mut cur));
            }
            cur.clear();
            in_quote = !in_quote;
        } else if c.is_whitespace() && !in_quote {
            if !cur.is_empty() {
                terms.push(std::mem::take(&mut cur));
            }
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        terms.push(cur);
    }
    terms.truncate(MAX_TERMS);
    terms
}

/// One lowercase char per input char: index alignment with the original text is
/// what lets a match be spliced back with its original casing.
fn lower(chars: &[char]) -> Vec<char> {
    chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect()
}

/// Longest term first, so a term that is a prefix of another does not shadow it.
fn ordered_terms(terms: &[String]) -> Vec<Vec<char>> {
    let mut kept: Vec<Vec<char>> =
        terms.iter().filter(|t| !t.is_empty()).map(|t| t.chars().collect()).collect();
    kept.sort_by_key(|t| std::cmp::Reverse(t.len()));
    kept
}

fn mark_text(text: &[char], terms: &[Vec<char>]) -> String {
    let hay = lower(text);
    let needles: Vec<Vec<char>> = terms.iter().map(|t| lower(t)).collect();
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        let hit = needles
            .iter()
            .find(|n| !n.is_empty() && i + n.len() <= hay.len() && hay[i..i + n.len()] == n[..]);
        if let Some(n) = hit {
            out.push_str("<mark class=\"search-hit\">");
            out.extend(text[i..i + n.len()].iter());
            out.push_str("</mark>");
            i += n.len();
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// Wrap every occurrence of any term in `<mark class="search-hit">`, touching
/// only text *outside* HTML tags so it never corrupts the markup it is layered
/// over.
#[must_use]
pub fn highlight_terms(html: &str, terms: &[String]) -> String {
    let needles = ordered_terms(terms);
    if needles.is_empty() {
        return html.to_string();
    }
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(end) = chars[i..].iter().position(|c| *c == '>') {
                out.extend(&chars[i..=i + end]);
                i += end + 1;
                continue;
            }
            out.push('<');
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] != '<' {
            i += 1;
        }
        out.push_str(&mark_text(&chars[start..i], &needles));
    }
    out
}
