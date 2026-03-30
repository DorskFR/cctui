// Ported from OpenAI Codex CLI (Apache 2.0) — simplified for cctui dark-theme TUI.
//! Renders unified diffs with line numbers, gutter signs, and syntax highlighting.

#![allow(clippy::pedantic, clippy::nursery)]

use diffy::Hunk;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::highlight::highlight_code_to_spans;

const TAB_WIDTH: usize = 4;

// Muted dark-theme palette (consistent with cctui conversation view)
const ADD_LINE_BG: Color = Color::Rgb(33, 58, 43); // #213A2B — dark green tint
const DEL_LINE_BG: Color = Color::Rgb(74, 34, 29); // #4A221D — dark red tint
const ADD_FG: Color = Color::Rgb(100, 180, 100); // muted green
const DEL_FG: Color = Color::Rgb(200, 100, 90); // muted red
const GUTTER_DIM: Style = Style::new().fg(Color::Rgb(80, 80, 80));
const HUNK_SEP: Style = Style::new().fg(Color::Rgb(60, 60, 60));

/// Render a unified diff string into display lines.
///
/// `lang` is the file extension (e.g. "rs", "ts") for syntax highlighting.
/// `width` is the available terminal columns.
pub fn render_unified_diff(
    unified_diff: &str,
    lang: Option<&str>,
    width: usize,
) -> Vec<Line<'static>> {
    let Ok(patch) = diffy::Patch::from_str(unified_diff) else {
        // Fallback: render as plain text
        return unified_diff.lines().map(|l| Line::from(Span::raw(l.to_string()))).collect();
    };

    let mut out: Vec<Line<'static>> = Vec::new();

    // Pre-compute max line number for gutter width
    let mut max_ln: usize = 0;
    for h in patch.hunks() {
        let mut old_ln = h.old_range().start();
        let mut new_ln = h.new_range().start();
        for l in h.lines() {
            match l {
                diffy::Line::Insert(_) => {
                    max_ln = max_ln.max(new_ln);
                    new_ln += 1;
                }
                diffy::Line::Delete(_) => {
                    max_ln = max_ln.max(old_ln);
                    old_ln += 1;
                }
                diffy::Line::Context(_) => {
                    max_ln = max_ln.max(new_ln);
                    old_ln += 1;
                    new_ln += 1;
                }
            }
        }
    }
    let ln_width = line_number_width(max_ln);

    let mut is_first_hunk = true;
    for h in patch.hunks() {
        if !is_first_hunk {
            let spacer = format!("{:w$} ", "", w = ln_width.max(1));
            out.push(Line::from(vec![
                Span::styled(spacer, GUTTER_DIM),
                Span::styled("⋮", HUNK_SEP),
            ]));
        }
        is_first_hunk = false;

        // Highlight each hunk as a single block to preserve parser state
        let hunk_syntax = lang.and_then(|language| {
            let hunk_text: String = h
                .lines()
                .iter()
                .map(|line| match line {
                    diffy::Line::Insert(t) | diffy::Line::Delete(t) | diffy::Line::Context(t) => *t,
                })
                .collect();
            let spans = highlight_code_to_spans(&hunk_text, language)?;
            (spans.len() == h.lines().len()).then_some(spans)
        });

        let mut old_ln = h.old_range().start();
        let mut new_ln = h.new_range().start();
        for (idx, l) in h.lines().iter().enumerate() {
            let syntax_spans = hunk_syntax.as_ref().and_then(|sl| sl.get(idx));
            match l {
                diffy::Line::Insert(text) => {
                    let s = text.trim_end_matches('\n');
                    out.extend(render_diff_line(
                        new_ln,
                        DiffLineKind::Insert,
                        s,
                        width,
                        ln_width,
                        syntax_spans,
                    ));
                    new_ln += 1;
                }
                diffy::Line::Delete(text) => {
                    let s = text.trim_end_matches('\n');
                    out.extend(render_diff_line(
                        old_ln,
                        DiffLineKind::Delete,
                        s,
                        width,
                        ln_width,
                        syntax_spans,
                    ));
                    old_ln += 1;
                }
                diffy::Line::Context(text) => {
                    let s = text.trim_end_matches('\n');
                    out.extend(render_diff_line(
                        new_ln,
                        DiffLineKind::Context,
                        s,
                        width,
                        ln_width,
                        syntax_spans,
                    ));
                    old_ln += 1;
                    new_ln += 1;
                }
            }
        }
    }

    out
}

/// Render a full file as all-add or all-delete lines.
pub fn render_full_file(
    content: &str,
    kind: DiffLineKind,
    lang: Option<&str>,
    width: usize,
) -> Vec<Line<'static>> {
    let line_count = content.lines().count();
    let ln_width = line_number_width(line_count);
    let syntax_lines = lang.and_then(|l| highlight_code_to_spans(content, l));
    let mut out = Vec::new();

    for (i, raw) in content.lines().enumerate() {
        let syn = syntax_lines.as_ref().and_then(|sl| sl.get(i));
        out.extend(render_diff_line(i + 1, kind, raw, width, ln_width, syn));
    }
    out
}

/// Count add/remove lines from a unified diff.
pub fn count_add_remove(diff: &str) -> (usize, usize) {
    let Ok(patch) = diffy::Patch::from_str(diff) else {
        return (0, 0);
    };
    patch.hunks().iter().flat_map(Hunk::lines).fold((0, 0), |(a, d), l| match l {
        diffy::Line::Insert(_) => (a + 1, d),
        diffy::Line::Delete(_) => (a, d + 1),
        diffy::Line::Context(_) => (a, d),
    })
}

// -- internals ----------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum DiffLineKind {
    Insert,
    Delete,
    Context,
}

fn render_diff_line(
    line_number: usize,
    kind: DiffLineKind,
    text: &str,
    width: usize,
    ln_width: usize,
    syntax_spans: Option<&Vec<Span<'static>>>,
) -> Vec<Line<'static>> {
    let gutter_w = ln_width.max(1);
    // gutter + space + sign = prefix columns
    let prefix_cols = gutter_w + 2;

    let (sign, sign_style, content_style, line_bg) = match kind {
        DiffLineKind::Insert => (
            '+',
            Style::default().fg(ADD_FG).bg(ADD_LINE_BG),
            Style::default().fg(ADD_FG).bg(ADD_LINE_BG),
            Style::default().bg(ADD_LINE_BG),
        ),
        DiffLineKind::Delete => (
            '-',
            Style::default().fg(DEL_FG).bg(DEL_LINE_BG),
            Style::default().fg(DEL_FG).bg(DEL_LINE_BG),
            Style::default().bg(DEL_LINE_BG),
        ),
        DiffLineKind::Context => (' ', Style::default(), Style::default(), Style::default()),
    };

    let available = width.saturating_sub(prefix_cols).max(1);

    // Build styled content spans
    let styled: Vec<Span<'static>> = if let Some(syn) = syntax_spans {
        syn.iter()
            .map(|sp| {
                let style = if matches!(kind, DiffLineKind::Delete) {
                    sp.style.add_modifier(Modifier::DIM)
                } else {
                    sp.style
                };
                Span::styled(sp.content.to_string(), style)
            })
            .collect()
    } else {
        vec![Span::styled(text.to_string(), content_style)]
    };

    let wrapped = wrap_styled_spans(&styled, available);
    let ln_str = line_number.to_string();

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, chunk) in wrapped.into_iter().enumerate() {
        let mut row: Vec<Span<'static>> = Vec::new();
        if i == 0 {
            row.push(Span::styled(format!("{ln_str:>gutter_w$} "), GUTTER_DIM));
            row.push(Span::styled(format!("{sign}"), sign_style));
        } else {
            row.push(Span::styled(format!("{:gutter_w$}  ", ""), GUTTER_DIM));
        }
        row.extend(chunk);
        lines.push(Line::from(row).style(line_bg));
    }
    lines
}

fn line_number_width(max: usize) -> usize {
    if max == 0 { 1 } else { max.to_string().len() }
}

/// Split styled spans into chunks fitting `max_cols` display columns.
fn wrap_styled_spans(spans: &[Span<'static>], max_cols: usize) -> Vec<Vec<Span<'static>>> {
    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut col: usize = 0;

    for span in spans {
        let style = span.style;
        let text = span.content.as_ref();
        let mut remaining = text;

        while !remaining.is_empty() {
            let mut byte_end = 0;
            let mut chars_col = 0;

            for ch in remaining.chars() {
                let w = ch.width().unwrap_or(if ch == '\t' { TAB_WIDTH } else { 0 });
                if col + chars_col + w > max_cols {
                    break;
                }
                byte_end += ch.len_utf8();
                chars_col += w;
            }

            if byte_end == 0 {
                if !current_line.is_empty() {
                    result.push(std::mem::take(&mut current_line));
                }
                let Some(ch) = remaining.chars().next() else {
                    break;
                };
                let ch_len = ch.len_utf8();
                current_line.push(Span::styled(remaining[..ch_len].to_string(), style));
                col = ch.width().unwrap_or(if ch == '\t' { TAB_WIDTH } else { 1 });
                remaining = &remaining[ch_len..];
                continue;
            }

            let (chunk, rest) = remaining.split_at(byte_end);
            current_line.push(Span::styled(chunk.to_string(), style));
            col += chars_col;
            remaining = rest;

            if col >= max_cols {
                result.push(std::mem::take(&mut current_line));
                col = 0;
            }
        }
    }

    if !current_line.is_empty() || result.is_empty() {
        result.push(current_line);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_unified_diff_renders() {
        let diff = "\
--- a/test.rs
+++ b/test.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello world\");
 }
";
        let lines = render_unified_diff(diff, Some("rs"), 80);
        assert!(!lines.is_empty());
        // Should have 4 content lines (context, delete, insert, context)
        // plus possibly gutter/wrapping
        let plain: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        // Check gutter signs are present
        assert!(plain.iter().any(|l| l.contains('+')));
        assert!(plain.iter().any(|l| l.contains('-')));
    }

    #[test]
    fn count_add_remove_works() {
        let diff = "\
--- a/test.rs
+++ b/test.rs
@@ -1,3 +1,4 @@
 fn main() {
-    old();
+    new();
+    extra();
 }
";
        let (added, removed) = count_add_remove(diff);
        assert_eq!(added, 2);
        assert_eq!(removed, 1);
    }

    #[test]
    fn empty_diff_returns_empty() {
        let lines = render_unified_diff("", None, 80);
        assert!(lines.is_empty() || lines.len() == 1);
    }

    #[test]
    fn wrapping_respects_width() {
        let long = "a".repeat(200);
        let chunks = wrap_styled_spans(&[Span::raw(long)], 80);
        assert!(chunks.len() >= 3); // 200 / 80 = 2.5 → 3 chunks
    }
}
