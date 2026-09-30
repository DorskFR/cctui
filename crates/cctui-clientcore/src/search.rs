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

/// Non-overlapping `[start, end)` char ranges, left to right.
fn ranges(text: &[char], terms: &[Vec<char>]) -> Vec<(usize, usize)> {
    let hay = lower(text);
    let needles: Vec<Vec<char>> = terms.iter().map(|t| lower(t)).collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let hit = needles
            .iter()
            .find(|n| !n.is_empty() && i + n.len() <= hay.len() && hay[i..i + n.len()] == n[..]);
        if let Some(n) = hit {
            hits.push((i, i + n.len()));
            i += n.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// Char ranges of every term occurrence in `text`.
///
/// For a caller that styles a match itself instead of wrapping it in markup —
/// the TUI's transcript highlight. Shares [`highlight_terms`]'s matcher, so the
/// two can never disagree about what a hit is.
#[must_use]
pub fn match_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let needles = ordered_terms(terms);
    if needles.is_empty() {
        return Vec::new();
    }
    ranges(&text.chars().collect::<Vec<char>>(), &needles)
}

fn mark_text(text: &[char], terms: &[Vec<char>]) -> String {
    let mut out = String::new();
    let mut at = 0;
    for (start, end) in ranges(text, terms) {
        out.extend(&text[at..start]);
        out.push_str("<mark class=\"search-hit\">");
        out.extend(&text[start..end]);
        out.push_str("</mark>");
        at = end;
    }
    out.extend(&text[at..]);
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

#[cfg(test)]
mod range_tests {
    use super::{highlight_terms, match_ranges, tokenize_query};

    fn terms(q: &str) -> Vec<String> {
        tokenize_query(q)
    }

    #[test]
    fn ranges_cover_every_occurrence_case_insensitively() {
        assert_eq!(match_ranges("Parser parses", &terms("parse")), [(0, 5), (7, 12)]);
        assert_eq!(match_ranges("nothing here", &terms("zzz")), []);
        assert_eq!(match_ranges("anything", &terms("")), []);
    }

    #[test]
    fn a_longer_term_wins_over_one_that_prefixes_it() {
        assert_eq!(match_ranges("parser", &terms("parse parser")), [(0, 6)]);
    }

    #[test]
    fn ranges_index_chars_not_bytes() {
        let hits = match_ranges("héllo wörld", &terms("wörld"));
        assert_eq!(hits, [(6, 11)]);
        let chars: Vec<char> = "héllo wörld".chars().collect();
        let (start, end) = hits[0];
        assert_eq!(chars[start..end].iter().collect::<String>(), "wörld");
    }

    /// The TUI styles these ranges itself while the webui wraps them in `<mark>`;
    /// one matcher means the two can never disagree about what a hit is.
    #[test]
    fn the_ranges_are_exactly_what_the_html_marks() {
        let text = "Parser parses the parse tree";
        let query = terms("parse \"parses\"");
        let marked = highlight_terms(text, &query);
        let from_html = marked.matches("<mark class=\"search-hit\">").count();
        assert_eq!(from_html, match_ranges(text, &query).len());

        let chars: Vec<char> = text.chars().collect();
        let spliced: String = match_ranges(text, &query)
            .iter()
            .map(|(s, e)| chars[*s..*e].iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(spliced, "Parse|parses|parse");
    }
}
