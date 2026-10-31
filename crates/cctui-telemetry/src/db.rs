//! `db.query` client spans around SQL queries.
use std::future::Future;

use tracing::Instrument;
use tracing::field::Empty;

const STATEMENTS: [&str; 5] = ["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE"];

/// Runs `build(sql)` in a `db.query` client span. Bind values are never
/// recorded; a disabled span costs no allocation.
pub async fn traced<T, E, F, Fut>(sql: &'static str, build: F) -> Result<T, E>
where
    F: FnOnce(&'static str) -> Fut + Send,
    Fut: Future<Output = Result<T, E>> + Send,
{
    let span = tracing::info_span!(
        "db.query",
        otel.kind = "client",
        db.system = "postgresql",
        db.operation = Empty,
        db.sql.table = Empty,
        db.statement = sql,
        error = Empty,
        otel.status_code = Empty,
    );
    if !span.is_disabled() {
        let (operation, table) = classify(sql);
        span.record("db.operation", operation);
        if let Some(table) = table {
            span.record("db.sql.table", table);
        }
    }
    let result = build(sql).instrument(span.clone()).await;
    if result.is_err() {
        span.record("error", true);
        span.record("otel.status_code", "ERROR");
    }
    result
}

fn classify(sql: &str) -> (&str, Option<&str>) {
    let mut words = Words { sql, pos: 0, depth: 0 };
    let Some(first) = words.next() else { return ("UNKNOWN", None) };
    let verb = if first.text.eq_ignore_ascii_case("WITH") {
        let depth = first.depth;
        let Some(verb) = words.find(|w| w.depth == depth && statement(w.text).is_some()) else {
            return (first.text, None);
        };
        verb
    } else {
        first
    };
    let Some(operation) = statement(verb.text) else { return (verb.text, None) };
    let named = match operation {
        "SELECT" => return (operation, select_table(words, verb.depth)),
        "UPDATE" => table(words, verb.depth),
        "DELETE" => after(words, "FROM", verb.depth),
        _ => after(words, "INTO", verb.depth),
    };
    (operation, named.map(|w| w.text))
}

fn statement(word: &str) -> Option<&'static str> {
    STATEMENTS.iter().find(|k| k.eq_ignore_ascii_case(word)).copied()
}

fn select_table(mut words: Words<'_>, depth: u32) -> Option<&str> {
    let mut nested = None;
    loop {
        let Some(word) = words.next() else { return nested };
        if !word.text.eq_ignore_ascii_case("FROM") {
            continue;
        }
        let found = table(words.clone(), word.depth).filter(|w| !w.call).map(|w| w.text);
        if word.depth == depth {
            return found.or(nested);
        }
        if nested.is_none() {
            nested = found;
        }
    }
}

fn after<'a>(mut words: Words<'a>, keyword: &str, depth: u32) -> Option<Word<'a>> {
    words.find(|w| w.depth == depth && w.text.eq_ignore_ascii_case(keyword))?;
    table(words, depth)
}

fn table(mut words: Words<'_>, depth: u32) -> Option<Word<'_>> {
    let mut word = words.next()?;
    if word.text.eq_ignore_ascii_case("ONLY") {
        word = words.next()?;
    }
    let named =
        word.depth == depth && word.text.bytes().next().is_some_and(|b| !b.is_ascii_digit());
    named.then_some(word)
}

struct Word<'a> {
    text: &'a str,
    depth: u32,
    /// Followed by `(`: a function call, not a table.
    call: bool,
}

#[derive(Clone)]
struct Words<'a> {
    sql: &'a str,
    pos: usize,
    depth: u32,
}

impl<'a> Words<'a> {
    fn skip_past(&mut self, needle: &[u8]) {
        let rest = &self.sql.as_bytes()[self.pos..];
        self.pos += rest
            .windows(needle.len())
            .position(|w| w == needle)
            .map_or(rest.len(), |at| at + needle.len());
    }

    fn word(&self, start: usize, end: usize) -> Word<'a> {
        let call = self.sql.as_bytes()[self.pos..]
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|&b| b == b'(');
        Word { text: &self.sql[start..end], depth: self.depth, call }
    }
}

const fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.'
}

impl<'a> Iterator for Words<'a> {
    type Item = Word<'a>;

    fn next(&mut self) -> Option<Word<'a>> {
        let sql = self.sql;
        let bytes = sql.as_bytes();
        while let Some(&byte) = bytes.get(self.pos) {
            let rest = &bytes[self.pos..];
            match byte {
                b'-' if rest.starts_with(b"--") => self.skip_past(b"\n"),
                b'/' if rest.starts_with(b"/*") => self.skip_past(b"*/"),
                b'\'' => {
                    self.pos += 1;
                    self.skip_past(b"'");
                }
                b'(' => {
                    self.depth += 1;
                    self.pos += 1;
                }
                b')' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.pos += 1;
                }
                b'"' => {
                    let start = self.pos + 1;
                    let len = rest[1..].iter().position(|&b| b == b'"').unwrap_or(rest.len() - 1);
                    self.pos = (start + len + 1).min(bytes.len());
                    return Some(self.word(start, start + len));
                }
                _ if is_word(byte) => {
                    let start = self.pos;
                    self.pos += rest.iter().take_while(|&&b| is_word(b)).count();
                    return Some(self.word(start, self.pos));
                }
                _ => self.pos += 1,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
