//! Text shaping for speech: markdown stripping, sentence splitting, the
//! spoken-style wrapper, and the WAV container the STT upstream expects.

use cctui_proto::injected::SESSION_CONTEXT;
use cctui_proto::voice::SpokenStyle;

/// Longer sentences are cut at a space so one TTS request stays short.
const MAX_SENTENCE_CHARS: usize = 300;
/// Streamed sentences shorter than this wait to be spoken with the next one.
const MIN_STREAMED_CHARS: usize = 40;
/// Words whose trailing period does not end a sentence.
const ABBREVIATIONS: [&str; 10] =
    ["e.g", "i.e", "etc", "vs", "cf", "mr", "mrs", "ms", "dr", "approx"];
pub const UTTERANCE_SAMPLE_RATE: u32 = 16_000;

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-' || c == '*' || c == '_' || c == ' ')
}

fn strip_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("![").or_else(|| rest.strip_prefix('['))
            && let Some(close) = after.find("](")
            && !after[..close].contains(']')
            && let Some(end) = after[close..].find(')')
        {
            out.push_str(&after[..close]);
            rest = &after[close + end + 1..];
            continue;
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('<')
            && after.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/')
            && let Some(end) = after.find('>')
        {
            rest = &after[end + 1..];
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        if matches!(c, '*' | '_' | '`' | '~') {
            rest = &rest[c.len_utf8()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn strip_line_prefix(line: &str) -> &str {
    let t = line.trim_start();
    let t = t.trim_start_matches('#');
    let t = t.trim_start_matches('>').trim_start();
    let t = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
        .unwrap_or(t);
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(after) =
            t[digits..].strip_prefix(". ").or_else(|| t[digits..].strip_prefix(") "))
    {
        return after;
    }
    t.strip_prefix("[ ] ").or_else(|| t.strip_prefix("[x] ")).unwrap_or(t)
}

/// Prose only: fenced code, tables, rules and link targets are dropped,
/// emphasis and list/heading markers are unwrapped.
///
/// Paragraph breaks become newlines so the splitter treats them as sentence
/// ends.
#[must_use]
pub fn strip_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.starts_with('|') || is_rule(trimmed) {
            continue;
        }
        if trimmed.is_empty() {
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        let body = strip_line_prefix(line);
        let block = body.len() != line.trim_start().len();
        let plain = strip_inline(body).split_whitespace().collect::<Vec<_>>().join(" ");
        if plain.is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(&plain);
        if block {
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

fn is_abbreviation(text: &str, dot: usize) -> bool {
    let word = text[..dot].rsplit(|c: char| c.is_whitespace() || c == '(').next().unwrap_or("");
    ABBREVIATIONS.iter().any(|a| word.eq_ignore_ascii_case(a))
}

/// Byte offset just past the first sentence end in `text`, if one is complete:
/// terminal punctuation (plus closing quotes or emphasis) followed by
/// whitespace, or a newline.
fn sentence_end(text: &str) -> Option<usize> {
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            return Some(i + 1);
        }
        if !matches!(c, '.' | '!' | '?' | '…') || (c == '.' && is_abbreviation(text, i)) {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some(&(j, n)) = chars.peek() {
            if !matches!(n, '.' | '!' | '?' | '…' | '"' | '\'' | ')' | '*' | '_' | '”' | '’')
            {
                break;
            }
            end = j + n.len_utf8();
            chars.next();
        }
        match chars.peek() {
            Some(&(_, n)) if n.is_whitespace() => return Some(end),
            Some(_) => {}
            None => return None,
        }
    }
    None
}

fn cut_long(sentence: &str, out: &mut Vec<String>) {
    let mut rest = sentence.trim();
    while rest.chars().count() > MAX_SENTENCE_CHARS {
        let limit = rest.char_indices().nth(MAX_SENTENCE_CHARS).map_or(rest.len(), |(i, _)| i);
        let head = &rest[..limit];
        let cut =
            head.rfind([',', ';', ':']).map(|i| i + 1).or_else(|| head.rfind(' ')).unwrap_or(limit);
        let (a, b) = rest.split_at(cut);
        if !a.trim().is_empty() {
            out.push(a.trim().to_owned());
        }
        rest = b.trim();
    }
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
}

/// Sentences of plain text, in order, each short enough for one TTS call.
#[must_use]
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(end) = sentence_end(rest) {
        cut_long(&rest[..end], &mut out);
        rest = &rest[end..];
    }
    cut_long(rest, &mut out);
    out
}

fn fences_closed(text: &str) -> bool {
    ["```", "~~~"].into_iter().all(|m| text.matches(m).count() % 2 == 0)
}

/// Accumulates streamed markdown and yields sentences as they complete, so
/// speech can start before the block ends.
///
/// Text inside an open code fence is held back until the fence closes, and
/// a sentence under [`MIN_STREAMED_CHARS`] waits for the next one.
#[derive(Debug, Default)]
pub struct SentenceBuffer {
    raw: String,
    /// Complete sentences too short to speak alone yet.
    held: String,
}

impl SentenceBuffer {
    pub fn push(&mut self, delta: &str) {
        self.raw.push_str(delta);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty() && self.held.is_empty()
    }

    /// Where an open code fence starts, or the end of the buffer.
    fn scan_end(&self) -> usize {
        ["```", "~~~"]
            .into_iter()
            .filter(|m| self.raw.matches(m).count() % 2 == 1)
            .filter_map(|m| self.raw.rfind(m))
            .min()
            .unwrap_or(self.raw.len())
    }

    /// Text a [`Self::pause`] would speak.
    pub fn has_pending(&self) -> bool {
        !self.held.is_empty() || !self.raw[..self.scan_end()].trim().is_empty()
    }

    /// Every sentence that is complete so far; the tail stays buffered.
    pub fn drain(&mut self) -> Vec<String> {
        let scan_end = self.scan_end();
        let (mut at, mut cut) = (0, 0);
        while let Some(end) = sentence_end(&self.raw[at..scan_end]) {
            at += end;
            if fences_closed(&self.raw[..at]) {
                cut = at;
            }
        }
        let head: String = self.raw.drain(..cut).collect();
        let mut out = Vec::new();
        for sentence in split_sentences(&strip_markdown(&head)) {
            if !self.held.is_empty() {
                self.held.push(' ');
            }
            self.held.push_str(&sentence);
            if self.held.chars().count() >= MIN_STREAMED_CHARS {
                out.push(std::mem::take(&mut self.held));
            }
        }
        out
    }

    /// Everything buffered outside an open code fence, complete or not: the
    /// stream paused.
    pub fn pause(&mut self) -> Vec<String> {
        let head: String = self.raw.drain(..self.scan_end()).collect();
        self.after_held(split_sentences(&strip_markdown(&head)))
    }

    /// Everything buffered, complete or not.
    pub fn flush(&mut self) -> Vec<String> {
        let raw = std::mem::take(&mut self.raw);
        self.after_held(split_sentences(&strip_markdown(&raw)))
    }

    fn after_held(&mut self, sentences: Vec<String>) -> Vec<String> {
        let held = std::mem::take(&mut self.held);
        if held.is_empty() {
            return sentences;
        }
        std::iter::once(held).chain(sentences).collect()
    }

    pub fn clear(&mut self) {
        self.raw.clear();
        self.held.clear();
    }
}

const fn style_words(style: SpokenStyle) -> &'static str {
    match style {
        SpokenStyle::Brief => "one to three short sentences",
        SpokenStyle::Normal => "a few short sentences",
        SpokenStyle::Verbose => "as much detail as needed, in short spoken sentences",
    }
}

/// The message the session receives for a spoken utterance: the transcript
/// behind a session-context block that asks for a reply fit to be read aloud.
#[must_use]
pub fn spoken_prompt(transcript: &str, style: Option<SpokenStyle>) -> String {
    let body = style.map_or_else(
        || "The user spoke this message aloud.".to_owned(),
        |s| {
            format!(
                "The user spoke this message aloud and will hear your reply through \
                 text-to-speech. Answer in {}: plain conversational prose, no markdown, \
                 no headings, no lists, no tables, no code.",
                style_words(s)
            )
        },
    );
    format!("<{SESSION_CONTEXT} voice=\"spoken\">\n{body}\n</{SESSION_CONTEXT}>\n\n{transcript}")
}

/// A 16-bit little-endian mono PCM buffer in a RIFF/WAVE container.
#[must_use]
pub fn pcm16_to_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_reduced_to_prose() {
        let md = "# Title\n\nSome **bold** and _em_ with `code` and a [link](https://x.y/z).\n\n\
                  ```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n\
                  - first item\n- second item\n\n---\n\n1. one\n2. two\n\nSee https://example.com now.";
        let plain = strip_markdown(md);
        assert_eq!(
            plain,
            "Title\nSome bold and em with code and a link.\nfirst item\nsecond item\none\ntwo\nSee now."
        );
    }

    #[test]
    fn html_tags_and_quotes_are_unwrapped() {
        assert_eq!(strip_markdown("> quoted <b>bold</b> text"), "quoted bold text");
        assert_eq!(strip_markdown("a < b and c > d"), "a < b and c > d");
    }

    #[test]
    fn sentences_split_on_terminal_punctuation_and_newlines() {
        assert_eq!(
            split_sentences("Hello there! How are you? Fine. Version 3.5 works.\nNext line"),
            vec!["Hello there!", "How are you?", "Fine.", "Version 3.5 works.", "Next line"]
        );
        assert_eq!(
            split_sentences("She said \"go.\" Then left…  Done"),
            vec!["She said \"go.\"", "Then left…", "Done"]
        );
        assert!(split_sentences("   \n\n ").is_empty());
    }

    #[test]
    fn long_sentences_are_cut_at_a_clause() {
        let long = format!("{}, and then more words follow here.", "word ".repeat(70).trim());
        let parts = split_sentences(&long);
        assert!(parts.len() >= 2, "{parts:?}");
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_SENTENCE_CHARS));
        assert_eq!(parts.concat().replace(' ', ""), long.replace(' ', ""));
    }

    const LONG_A: &str = "The first sentence is long enough to speak.";
    const LONG_B: &str = "The second one is also long enough to go.";

    #[test]
    fn buffer_yields_complete_sentences_and_holds_open_fences() {
        let mut buf = SentenceBuffer::default();
        buf.push(&format!("{LONG_A} Sec"));
        assert_eq!(buf.drain(), vec![LONG_A]);
        buf.push("ond one is also long enough to go now! ```rust\nlet x = 1. ");
        assert_eq!(buf.drain(), vec!["Second one is also long enough to go now!"]);
        buf.push(&format!("\n```\n{LONG_B} tail"));
        assert_eq!(buf.drain(), vec![LONG_B]);
        assert_eq!(buf.flush(), vec!["tail"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn buffer_holds_open_tilde_fences() {
        let mut buf = SentenceBuffer::default();
        buf.push(&format!("{LONG_A}\n~~~\ncode line one.\n"));
        assert_eq!(buf.drain(), vec![LONG_A]);
        buf.push(&format!("more code.\n~~~\n{LONG_B} tail"));
        assert_eq!(buf.drain(), vec![LONG_B]);
    }

    #[test]
    fn a_fence_closer_streamed_without_its_newline_still_closes() {
        let mut buf = SentenceBuffer::default();
        buf.push(&format!("{LONG_A}\n```\ncode line.\n```"));
        assert_eq!(buf.drain(), vec![LONG_A]);
        buf.push(&format!("\n{LONG_B} tail"));
        assert_eq!(buf.drain(), vec![LONG_B]);
        assert_eq!(buf.flush(), vec!["tail"]);
    }

    #[test]
    fn short_streamed_sentences_wait_for_the_next_one() {
        let mut buf = SentenceBuffer::default();
        buf.push("Sure. Okay. ");
        assert!(buf.drain().is_empty());
        assert!(buf.has_pending());
        buf.push(&format!("{LONG_A} Done. "));
        assert_eq!(buf.drain(), vec![format!("Sure. Okay. {LONG_A}")]);
        assert_eq!(buf.flush(), vec!["Done."]);
    }

    #[test]
    fn abbreviations_and_decimals_do_not_end_a_sentence() {
        assert_eq!(
            split_sentences("Use a tool, e.g. a hammer. Or i.e. this one vs. that. Done"),
            vec!["Use a tool, e.g. a hammer.", "Or i.e. this one vs. that.", "Done"]
        );
        let mut buf = SentenceBuffer::default();
        buf.push("Version 3.");
        assert!(buf.drain().is_empty());
        buf.push("5 shipped with a few fixes, e.g. ");
        assert!(buf.drain().is_empty());
        buf.push("the voice one. Next");
        assert_eq!(buf.drain(), vec!["Version 3.5 shipped with a few fixes, e.g. the voice one."]);
    }

    #[test]
    fn a_pause_speaks_the_tail_but_not_an_open_fence() {
        let mut buf = SentenceBuffer::default();
        buf.push("Short. And a half");
        assert!(buf.drain().is_empty());
        assert_eq!(buf.pause(), vec!["Short.", "And a half"]);
        assert!(!buf.has_pending());
        buf.push("See https://example.com/x for it:\n```sh\nrm -rf build.\n");
        assert_eq!(buf.pause(), vec!["See for it:"]);
        assert!(!buf.has_pending(), "an open fence is not pending speech");
        buf.push("```\nAfter the code.");
        assert_eq!(buf.pause(), vec!["After the code."]);
    }

    #[test]
    fn a_bracket_before_a_link_is_kept_as_text() {
        assert_eq!(strip_markdown("a[0] and [x](u) done"), "a[0] and x done");
    }

    #[test]
    fn buffer_treats_bold_closers_as_part_of_the_sentence() {
        let mut buf = SentenceBuffer::default();
        buf.push("**Done, and the whole build is green again.** Next");
        assert_eq!(buf.drain(), vec!["Done, and the whole build is green again."]);
        assert_eq!(buf.flush(), vec!["Next"]);
    }

    #[test]
    fn spoken_prompt_wraps_the_transcript() {
        let p = spoken_prompt("what time is it", Some(SpokenStyle::Brief));
        assert!(p.starts_with("<cctui-session-context voice=\"spoken\">\n"));
        assert!(p.contains("one to three short sentences"));
        assert!(p.ends_with("</cctui-session-context>\n\nwhat time is it"));
        let bare = spoken_prompt("hi", None);
        assert!(!bare.contains("text-to-speech"));
        assert!(bare.ends_with("\n\nhi"));
    }

    #[test]
    fn wav_header_describes_mono_16bit() {
        let wav = pcm16_to_wav(&[1, 0, 2, 0], 16_000);
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
    }
}
