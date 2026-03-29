// Ported from OpenAI Codex CLI (Apache 2.0)
//! Syntax highlighting for code blocks in markdown.

use ratatui::style::{Color as RtColor, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

const ANSI_ALPHA_INDEX: u8 = 0x00;
const ANSI_ALPHA_DEFAULT: u8 = 0x01;
const OPAQUE_ALPHA: u8 = 0xFF;
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn default_theme() -> Theme {
    two_face::theme::extra().get(two_face::theme::EmbeddedThemeName::CatppuccinMocha).clone()
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();

    let patched = match lang {
        "csharp" | "c-sharp" => "c#",
        "golang" => "go",
        "python3" => "python",
        "shell" => "bash",
        _ => lang,
    };

    if let Some(s) = ss.find_syntax_by_token(patched) {
        return Some(s);
    }
    if let Some(s) = ss.find_syntax_by_name(patched) {
        return Some(s);
    }
    let lower = patched.to_ascii_lowercase();
    if let Some(s) = ss.syntaxes().iter().find(|s| s.name.to_ascii_lowercase() == lower) {
        return Some(s);
    }
    if let Some(s) = ss.find_syntax_by_extension(lang) {
        return Some(s);
    }
    None
}

#[allow(clippy::disallowed_methods)]
fn ansi_palette_color(index: u8) -> RtColor {
    match index {
        0x00 => RtColor::Black,
        0x01 => RtColor::Red,
        0x02 => RtColor::Green,
        0x03 => RtColor::Yellow,
        0x04 => RtColor::Blue,
        0x05 => RtColor::Magenta,
        0x06 => RtColor::Cyan,
        0x07 => RtColor::Gray,
        n => RtColor::Indexed(n),
    }
}

#[allow(clippy::disallowed_methods)]
fn convert_syntect_color(color: SyntectColor) -> Option<RtColor> {
    match color.a {
        ANSI_ALPHA_INDEX => Some(ansi_palette_color(color.r)),
        ANSI_ALPHA_DEFAULT => None,
        OPAQUE_ALPHA => Some(RtColor::Rgb(color.r, color.g, color.b)),
        _ => Some(RtColor::Rgb(color.r, color.g, color.b)),
    }
}

fn convert_style(syn_style: syntect::highlighting::Style) -> Style {
    let mut rt_style = Style::default();

    if let Some(fg) = convert_syntect_color(syn_style.foreground) {
        rt_style = rt_style.fg(fg);
    }

    if syn_style.font_style.contains(FontStyle::BOLD) {
        rt_style.add_modifier |= Modifier::BOLD;
    }

    rt_style
}

pub(crate) fn highlight_code_to_lines(code: &str, lang: &str) -> Vec<Line<'static>> {
    if code.is_empty() {
        return vec![Line::from(String::new())];
    }

    if code.len() > MAX_HIGHLIGHT_BYTES || code.lines().count() > MAX_HIGHLIGHT_LINES {
        let mut result: Vec<Line<'static>> =
            code.lines().map(|l| Line::from(l.to_string())).collect();
        if result.is_empty() {
            result.push(Line::from(String::new()));
        }
        return result;
    }

    let Some(syntax) = find_syntax(lang) else {
        let mut result: Vec<Line<'static>> =
            code.lines().map(|l| Line::from(l.to_string())).collect();
        if result.is_empty() {
            result.push(Line::from(String::new()));
        }
        return result;
    };

    let theme = default_theme();
    let mut h = HighlightLines::new(syntax, &theme);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for line in LinesWithEndings::from(code) {
        let Ok(ranges) = h.highlight_line(line, syntax_set()) else {
            lines.push(Line::from(line.to_string()));
            continue;
        };

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            spans.push(Span::styled(text.to_string(), convert_style(style)));
        }
        if spans.is_empty() {
            spans.push(Span::raw(String::new()));
        }
        lines.push(Line::from(spans));
    }

    if lines.is_empty() {
        lines.push(Line::from(String::new()));
    }
    lines
}
