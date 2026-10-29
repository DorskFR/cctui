//! [`markdown_text`] ports `renderMarkdown` (`webui/src/lib/markdown.ts`) to
//! its visible text and [`fold`] twins `foldSearchText` (`webui/src/lib/search.ts`):
//! keep them in step, `fixtures/parity/searchText.json` replays both.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Any phrase up to [`OVERLAP_BYTES`] lies whole inside at least one chunk.
pub const CHUNK_BYTES: usize = 4096;
pub const OVERLAP_BYTES: usize = 512;
/// Folded text indexed per event; the rest of a huge tool output is not.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// An unbroken run of base64/hex alphabet this long is a payload, not prose.
const BLOB_RUN: usize = 200;
/// How far back from a chunk's end a space is preferred as the cut.
const CUT_SLACK: usize = 256;

const BLOCK_L: char = '\u{E000}';
const BLOCK_R: char = '\u{E001}';
/// Stand-ins for the webui's `<code>` and `<strong>`/`<em>`/`<a>` tags, so the
/// later passes see a non-marker character where the HTML has a tag. `<code>`
/// carries a space (`class="md-code"`), which the link URL class excludes.
const CODE_TAG: char = '\u{E002}';
const EMPH_TAG: char = '\u{E003}';

const USER_PREFIX: &str = "▷ User:";

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static search regex")
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"[\x1B\x{9B}][\[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z0-9/#&.:=?%@~_]+)*|[a-zA-Z0-9]+(?:;[-a-zA-Z0-9/#&.:=?%@~_]*)*)?\x07)|(?:(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-PR-TZcf-ntqry=><~]))",
    )
});
static C0: LazyLock<Regex> = LazyLock::new(|| re(r"[\x00-\x08\x0B-\x1F\x7F]"));
static IMG_MARKER: LazyLock<Regex> =
    LazyLock::new(|| re(r"!\[[^\]]*\]\(cctui-img://[A-Za-z0-9-]+\)"));
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)```[^\n`]*\n?(.*?)```"));
static TABLE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?:^|\n)([ \t]*\|.+\|[ \t]*)\n[ \t]*\|(?:[ \t]*:?-+:?[ \t]*\|)+[ \t]*\n((?:[ \t]*\|.*\|[ \t]*(?:\n|$))+)",
    )
});
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| re(r"`([^`]+)`"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| re(r"\*\*([^*]+)\*\*"));
static ITALIC: LazyLock<Regex> = LazyLock::new(|| re(r"(^|[^*])\*([^*\n]+)\*"));
static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"\[([^\]]+)\]\(https?://[^\s)\x{E002}]+\)"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^#{1,6}\s+(.+)$"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^>\s?(.*)$"));
static LIST: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*[-*]\s+(.+)$"));
static BLOCK_REF: LazyLock<Regex> = LazyLock::new(|| re(r"\x{E000}(\d+)\x{E001}"));

/// `stripAnsi` from `webui/src/lib/highlight.ts`.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let s = ANSI.replace_all(s, "");
    C0.replace_all(&s, "").into_owned()
}

fn stash(blocks: &mut Vec<String>, text: String) -> String {
    blocks.push(text);
    format!("{BLOCK_L}{}{BLOCK_R}", blocks.len() - 1)
}

/// `splitRow`: cells of one GFM table row, honouring `\|`.
fn split_row(row: &str) -> Vec<String> {
    let t = row.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '|' {
            cells.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    cells.push(cur);
    cells
}

fn table_text(header: &str, body: &str) -> String {
    let row =
        |r: &str| split_row(r).iter().map(|c| inline_text(c.trim())).collect::<Vec<_>>().join(" ");
    let mut lines = vec![row(header)];
    lines.extend(body.split('\n').filter(|r| !r.trim().is_empty()).map(row));
    format!("\n{}\n", lines.join("\n"))
}

/// `inlineMd` minus its markup: code, bold, italic and link text survive,
/// their markers and URLs do not.
fn inline_text(s: &str) -> String {
    let s = INLINE_CODE.replace_all(s, format!("{CODE_TAG}${{1}}{CODE_TAG}"));
    let s = BOLD.replace_all(&s, format!("{EMPH_TAG}${{1}}{EMPH_TAG}"));
    let s = ITALIC.replace_all(&s, format!("${{1}}{EMPH_TAG}${{2}}{EMPH_TAG}"));
    LINK.replace_all(&s, format!("{EMPH_TAG}${{1}}{EMPH_TAG}")).into_owned()
}

/// The visible text of `renderMarkdown(src, { sessionId })`.
#[must_use]
pub fn markdown_text(src: &str) -> String {
    let src = strip_ansi(src);
    let src = IMG_MARKER.replace_all(&src, "");
    let mut blocks: Vec<String> = Vec::new();
    let s = FENCE
        .replace_all(&src, |c: &regex::Captures<'_>| {
            let code = &c[1];
            stash(&mut blocks, format!("\n{}\n", code.strip_suffix('\n').unwrap_or(code)))
        })
        .into_owned();
    let s = TABLE
        .replace_all(&s, |c: &regex::Captures<'_>| stash(&mut blocks, table_text(&c[1], &c[2])))
        .into_owned();
    let s = inline_text(&s);
    let s = HEADING.replace_all(&s, "${1}");
    let s = QUOTE.replace_all(&s, "${1}");
    let s = LIST.replace_all(&s, "• ${1}");
    let s = BLOCK_REF.replace_all(&s, |c: &regex::Captures<'_>| {
        c[1].parse::<usize>().ok().and_then(|i| blocks.get(i)).cloned().unwrap_or_default()
    });
    s.chars().filter(|c| *c != CODE_TAG && *c != EMPH_TAG).collect()
}

/// NFKC, typographic quotes to straight ones, list bullets and every
/// whitespace run to one space, invisible characters dropped. Case is kept.
#[must_use]
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.nfkc() {
        let c = match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}' => {
                continue;
            }
            '•' => ' ',
            c => c,
        };
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

const fn is_blob_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

/// Replace every run of [`BLOB_RUN`]+ base64/hex-alphabet characters with a
/// space: inline images, keys and dumps nobody reads or searches.
#[must_use]
pub fn strip_blobs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run_start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if is_blob_char(c) {
            run_start.get_or_insert(i);
            continue;
        }
        if let Some(start) = run_start.take() {
            push_run(&mut out, &s[start..i]);
        }
        out.push(c);
    }
    if let Some(start) = run_start {
        push_run(&mut out, &s[start..]);
    }
    out
}

fn push_run(out: &mut String, run: &str) {
    if run.len() >= BLOB_RUN {
        out.push(' ');
    } else {
        out.push_str(run);
    }
}

fn push_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|i| push_strings(i, out)),
        Value::Object(map) => map.values().for_each(|i| push_strings(i, out)),
        _ => {}
    }
}

/// Text blocks of a content array; image and document blocks are skipped.
fn push_blocks(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|i| push_blocks(i, out)),
        Value::Object(map) => match map.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = map.get("text").and_then(Value::as_str) {
                    out.push(t.to_owned());
                }
            }
            Some("image" | "document") => {}
            _ => {
                if let Some(c) = map.get("content") {
                    push_blocks(c, out);
                }
            }
        },
        _ => {}
    }
}

/// A tool result's `output_summary`: plain text, or a stringified content
/// array whose text blocks are the readable part.
fn tool_result_text(summary: &str) -> String {
    if summary.trim_start().starts_with('[')
        && let Ok(v @ Value::Array(_)) = serde_json::from_str::<Value>(summary)
    {
        let mut parts = Vec::new();
        push_blocks(&v, &mut parts);
        return strip_ansi(&parts.join("\n"));
    }
    strip_ansi(summary)
}

/// The raw visible text of a canonical client payload ([`crate::normalize::for_client`]).
fn visible_text(v: &Value) -> String {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
    match s("type") {
        "text" => {
            let content = s("content");
            let body = content.strip_prefix(USER_PREFIX).unwrap_or(content);
            markdown_text(body)
        }
        "reply" | "compact_summary" => markdown_text(s("content")),
        "turn_summary" => strip_ansi(s("detail")),
        "tool_call" => {
            let mut parts = vec![s("tool").to_owned()];
            if let Some(input) = v.get("input") {
                push_strings(input, &mut parts);
            }
            strip_ansi(&parts.join("\n"))
        }
        "tool_result" => tool_result_text(s("output_summary")),
        _ => String::new(),
    }
}

/// Largest char boundary at or below `i`.
const fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The folded searchable text of a canonical client payload, capped at
/// [`MAX_TEXT_BYTES`].
#[must_use]
pub fn client_text(v: &Value) -> String {
    let mut text = fold(&strip_blobs(&visible_text(v)));
    text.truncate(floor_boundary(&text, MAX_TEXT_BYTES));
    text
}

/// Split `text` into chunks of at most [`CHUNK_BYTES`], each starting
/// [`OVERLAP_BYTES`] or more before the previous one ended, cut at a space
/// when one is near the end.
#[must_use]
pub fn chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }
    let mut start = 0;
    while text.len() - start > CHUNK_BYTES {
        let hard = floor_boundary(text, start + CHUNK_BYTES);
        let end = text[start..hard]
            .rfind(' ')
            .map(|i| start + i)
            .filter(|&i| i > hard - CUT_SLACK)
            .unwrap_or(hard);
        out.push(&text[start..end]);
        start = floor_boundary(text, end - OVERLAP_BYTES);
    }
    out.push(&text[start..]);
    out
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use serde_json::json;

    use super::*;

    fn rendered(md: &str) -> String {
        fold(&markdown_text(md))
    }

    #[test]
    fn parity_with_the_webui_renderer() {
        let fx: Value =
            serde_json::from_str(include_str!("../../../../fixtures/parity/searchText.json"))
                .unwrap();
        let cases = |k: &str| fx[k].as_array().unwrap().clone();
        for c in cases("markdownText") {
            assert_eq!(rendered(c["md"].as_str().unwrap()), c["out"], "{c}");
        }
        for c in cases("fold") {
            assert_eq!(fold(c["s"].as_str().unwrap()), c["out"], "{c}");
        }
    }

    #[test]
    fn inline_code_markers_vanish_so_a_copied_phrase_matches() {
        let text = rendered("Change the gate to `master...HEAD` (three dots).");
        assert_eq!(text, "Change the gate to master...HEAD (three dots).");
        let q = fold("Change the gate to master...HEAD (three dots).");
        assert!(text.contains(&q));
    }

    #[test]
    fn emphasis_links_headings_quotes_and_lists_render_to_their_text() {
        assert_eq!(rendered("a **bold** and *it* word"), "a bold and it word");
        assert_eq!(rendered("see [the docs](https://x.dev/a?b=1) now"), "see the docs now");
        assert_eq!(rendered("## Title here\nbody"), "Title here body");
        assert_eq!(rendered("> quoted line"), "quoted line");
        assert_eq!(rendered("- one\n- two"), "one two");
        assert_eq!(rendered("x ![shot](cctui-img://ab-12) y"), "x y");
    }

    #[test]
    fn fenced_code_keeps_its_body_and_drops_the_fence_and_language() {
        assert_eq!(rendered("run:\n```bash\nls -la `pwd`\n```\ndone"), "run: ls -la `pwd` done");
    }

    #[test]
    fn tables_render_cells_without_pipes() {
        let md = "| a | **b** |\n|---|:-:|\n| c \\| d | e |\n";
        assert_eq!(rendered(md), "a b c | d e");
    }

    #[test]
    fn fold_straightens_quotes_and_collapses_whitespace() {
        assert_eq!(fold("it\u{2019}s \u{201C}fine\u{201D}"), "it's \"fine\"");
        assert_eq!(fold("  a\n\n\tb\u{00A0}c\u{200B}d  "), "a b cd");
        assert_eq!(fold("ﬁle １２"), "file 12");
    }

    #[test]
    fn a_multi_line_copy_matches_soft_and_hard_breaks() {
        let text = rendered("first line\nsecond *line*\n\nthird");
        assert!(text.contains(&fold("first line\r\nsecond line\nthird")));
    }

    #[test]
    fn ansi_and_control_bytes_are_stripped() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m\r\n"), "red\n");
    }

    #[test]
    fn image_blocks_and_base64_runs_are_not_indexed() {
        let blob = "iVBORw0KGgo".repeat(60);
        let summary = json!([
            {"type": "text", "text": "Screenshot taken"},
            {"type": "image", "source": {"type": "base64", "data": blob}},
        ])
        .to_string();
        let v = json!({"type": "tool_result", "output_summary": summary});
        let text = client_text(&v);
        assert_eq!(text, "Screenshot taken");

        let inline =
            json!({"type": "text", "content": format!("look data:image/png;base64,{blob} ok")});
        let text = client_text(&inline);
        assert!(!text.contains("iVBOR"), "{text}");
        assert!(text.starts_with("look") && text.ends_with("ok"));
    }

    #[test]
    fn tool_calls_index_the_tool_and_input_values_not_json_syntax() {
        let v = json!({"type": "tool_call", "tool": "Bash", "input": {"command": "git diff\nmaster...HEAD"}});
        assert_eq!(client_text(&v), "Bash git diff master...HEAD");
    }

    #[test]
    fn user_turns_drop_the_storage_prefix() {
        let v = json!({"type": "text", "content": "▷ User: please *run* it"});
        assert_eq!(client_text(&v), "please run it");
    }

    #[test]
    fn chunks_overlap_so_text_after_8_kb_and_across_cuts_is_found() {
        let text =
            fold(&format!("{} needle phrase here {}", "word ".repeat(2000), "tail ".repeat(800)));
        let parts = chunks(&text);
        assert!(parts.len() >= 3, "{}", parts.len());
        assert!(parts.iter().all(|p| p.len() <= CHUNK_BYTES));
        assert!(parts.iter().any(|p| p.contains("needle phrase here")));
    }

    #[test]
    fn every_short_window_lies_inside_some_chunk() {
        let text = (0..3000).fold(String::new(), |mut acc, i| {
            let _ = write!(acc, "w{i} ");
            acc
        });
        let text = fold(&text);
        let parts = chunks(&text);
        let mut i = 0;
        while i + OVERLAP_BYTES <= text.len() {
            let w = &text[i..i + OVERLAP_BYTES];
            assert!(parts.iter().any(|p| p.contains(w)), "window at {i}");
            i += 97;
        }
    }

    #[test]
    fn chunks_respect_char_boundaries() {
        let text = "é".repeat(5000);
        let parts = chunks(&text);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.chars().all(|c| c == 'é')));
    }

    #[test]
    fn client_text_is_capped() {
        let v = json!({"type": "tool_result", "output_summary": "ab ".repeat(200_000)});
        assert!(client_text(&v).len() <= MAX_TEXT_BYTES);
    }
}
