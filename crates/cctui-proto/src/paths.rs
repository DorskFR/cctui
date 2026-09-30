//! Agent-linked local path detection, shared by the server's transcript indexer
//! and the TUI's file viewer.
//!
//! The twin of the webui `LOCAL_PATH` regex in `markdown.ts`; the golden cases
//! in `fixtures/parity/localPaths.json` hold the three implementations together.

use std::collections::BTreeSet;

const fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'@' | b'+' | b'%' | b'-' | b'/')
}

/// How many space-separated chunks a name may absorb before the candidate is
/// treated as prose rather than one file name.
const MAX_SPACE_CHUNKS: usize = 5;

/// Bytes a candidate may start after. Anything else (a letter, a `:`) means the
/// `/` belongs to something larger, such as a URL's path.
const fn lead_ok(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | b'('
            | b'['
            | b';'
            | b'>'
            | b'"'
            | b'\''
            | b'`'
            | b','
            | b'='
            | b'*'
    )
}

/// Absolute (`/a/b.ext`) and home-relative (`~/a/b.ext`) paths in `s`, in
/// order, with duplicates kept — the viewer offers them as a picker.
///
/// A candidate must start at a word boundary, so a URL's path
/// (`https://h/a.png`) never matches: its `//` follows `:` and `h/a.png` does
/// not start with `/`.
#[must_use]
pub fn scan(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || lead_ok(bytes[i - 1]);
        let starts = bytes[i] == b'/' || (bytes[i] == b'~' && bytes.get(i + 1) == Some(&b'/'));
        if !(boundary && starts) {
            i += 1;
            continue;
        }
        let mut j = if bytes[i] == b'~' { i + 1 } else { i };
        while j < bytes.len() && is_path_byte(bytes[j]) {
            j += 1;
        }
        let cand = trim_dots(&s[i..j]);
        if has_extension(cand) {
            out.push(cand.to_owned());
        } else if let Some(end) = extend_over_spaces(s, i, j) {
            out.push(s[i..end].to_owned());
            i = end;
            continue;
        }
        i = j.max(i + 1);
    }
    out
}

/// [`scan`] deduplicated and sorted, for an index rather than a display list.
pub fn scan_into(s: &str, out: &mut BTreeSet<String>) {
    out.extend(scan(s));
}

/// Grow a candidate that carries no extension yet across single spaces, so
/// `…/Screenshot 2026-09-29 at 10.11.12.png` links as one path. Returns the end
/// of the first chunk completing an alphabetic extension; a chunk opening with
/// `/` starts a new path and ends the attempt, which is what keeps surrounding
/// prose out.
fn extend_over_spaces(s: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut at = end;
    for _ in 0..MAX_SPACE_CHUNKS {
        if bytes.get(at) != Some(&b' ') || bytes.get(at + 1).is_none_or(|b| *b == b'/') {
            return None;
        }
        let mut j = at + 1;
        while j < bytes.len() && is_path_byte(bytes[j]) {
            j += 1;
        }
        if j == at + 1 {
            return None;
        }
        at = j;
        let cand = trim_dots(&s[start..at]);
        if extension_of(cand).is_some_and(|e| e.bytes().all(|b| b.is_ascii_alphabetic())) {
            return Some(start + cand.len());
        }
    }
    None
}

fn trim_dots(cand: &str) -> &str {
    cand.trim_end_matches('.')
}

/// The candidate's extension when it looks like one: alphanumeric, at most 8
/// bytes, and not the whole name (a dotfile has no extension).
#[must_use]
pub fn extension_of(cand: &str) -> Option<&str> {
    let name = &cand[cand.rfind('/').map_or(0, |p| p + 1)..];
    let dot = name.rfind('.')?;
    let ext = &name[dot + 1..];
    (!ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|b| b.is_ascii_alphanumeric()) && dot > 0)
        .then_some(ext)
}

#[must_use]
pub fn has_extension(cand: &str) -> bool {
    extension_of(cand).is_some()
}

/// The path a cursor at `col` sits on, for `gf` on a rendered line.
#[must_use]
pub fn at_column(line: &str, col: usize) -> Option<String> {
    let mut best: Option<String> = None;
    let mut from = 0;
    for path in scan(line) {
        let Some(at) = line[from..].find(&path).map(|i| from + i) else { continue };
        from = at + path.len();
        if (at..at + path.len()).contains(&col) {
            return Some(path);
        }
        // Remember the first path on the line, so a cursor sitting in prose
        // still opens something rather than nothing.
        if best.is_none() {
            best = Some(path);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{at_column, extension_of, has_extension, scan};

    #[test]
    fn an_absolute_path_with_an_extension_is_found() {
        assert_eq!(scan("see /home/dev/a.rs now"), ["/home/dev/a.rs"]);
        assert_eq!(scan("/a.rs"), ["/a.rs"]);
    }

    #[test]
    fn a_home_relative_path_is_found() {
        assert_eq!(scan("in ~/src/main.rs"), ["~/src/main.rs"]);
        assert_eq!(scan("~notahome/x.rs"), [] as [String; 0]);
    }

    #[test]
    fn a_url_path_is_not_a_local_path() {
        assert!(scan("https://h/a.png").is_empty());
        assert!(scan("see http://example.com/x/y.rs").is_empty());
    }

    #[test]
    fn a_path_needs_a_word_boundary_on_the_left() {
        assert!(scan("x/a.rs").is_empty());
        assert_eq!(scan("(/a.rs)"), ["/a.rs"]);
        assert_eq!(scan("`/a.rs`"), ["/a.rs"]);
        assert_eq!(scan("path=/a.rs"), ["/a.rs"]);
    }

    #[test]
    fn trailing_sentence_punctuation_is_not_part_of_the_path() {
        assert_eq!(scan("edited /a/b.rs."), ["/a/b.rs"]);
    }

    #[test]
    fn a_name_absorbs_spaces_up_to_the_extension() {
        assert_eq!(
            scan("~/Pictures/Screenshot 2026-09-29 at 10.11.12.png here"),
            ["~/Pictures/Screenshot 2026-09-29 at 10.11.12.png"]
        );
    }

    #[test]
    fn absorbing_stops_before_a_chunk_that_opens_a_new_path() {
        assert_eq!(scan("/a/b /c/d.rs"), ["/c/d.rs"], "the first has no extension to complete");
    }

    #[test]
    fn a_directory_without_an_extension_is_not_offered() {
        assert!(scan("cd /home/dev/project").is_empty());
    }

    #[test]
    fn every_path_on_a_line_is_returned_in_order() {
        assert_eq!(scan("moved /a/one.rs to /b/two.rs"), ["/a/one.rs", "/b/two.rs"]);
    }

    #[test]
    fn extensions_are_bounded_and_alphanumeric() {
        assert_eq!(extension_of("/a/b.rs"), Some("rs"));
        assert_eq!(extension_of("/a/b.tar.gz"), Some("gz"));
        assert_eq!(extension_of("/a/.hidden"), None, "a dotfile has no extension");
        assert_eq!(extension_of("/a/b.toolongextension"), None);
        assert!(!has_extension("/a/b"));
    }

    #[test]
    fn the_cursor_picks_the_path_it_sits_on() {
        let line = "moved /a/one.rs to /b/two.rs";
        assert_eq!(at_column(line, 8).as_deref(), Some("/a/one.rs"));
        assert_eq!(at_column(line, 22).as_deref(), Some("/b/two.rs"));
    }

    #[test]
    fn a_cursor_in_prose_falls_back_to_the_first_path() {
        assert_eq!(at_column("moved /a/one.rs to /b/two.rs", 0).as_deref(), Some("/a/one.rs"));
        assert_eq!(at_column("no paths here", 2), None);
    }
}
