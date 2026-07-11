use crate::ast::{Filter, FilterOp, Node};
use crate::registry::{self, FieldType};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Text(String),
    Field { field: String, op: OpCode, values: Vec<String> },
}

/// The operator codes tsumikit's `FilterSearchBar` serialises (`machine=x`,
/// `title:"y"`, `status!=z`, `title!:"w"`, `tag in (a, b)`), which are the
/// wire format the webui sends. `:` doubles as the legacy cctui syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpCode {
    Colon,
    Eq,
    NotEq,
    NotContains,
    In,
}

/// One whitespace-delimited word, split into runs that were inside vs outside
/// double quotes — operator/comma structure only counts outside quotes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    segs: Vec<(String, bool)>,
}

impl Word {
    fn plain(&self) -> Option<&str> {
        match self.segs.as_slice() {
            [(s, false)] => Some(s),
            _ => None,
        }
    }

    fn fully_quoted(&self) -> bool {
        !self.segs.is_empty() && self.segs.iter().all(|(_, q)| *q)
    }

    fn joined(&self) -> String {
        self.segs.iter().map(|(s, _)| s.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawTok {
    LParen,
    RParen,
    Word(Word),
}

fn raw_lex(input: &str) -> Vec<RawTok> {
    let mut out = Vec::new();
    let mut segs: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut had_quote = false;

    let flush_seg = |segs: &mut Vec<(String, bool)>, cur: &mut String, quoted: bool| {
        if !cur.is_empty() || quoted {
            segs.push((std::mem::take(cur), quoted));
        }
    };
    let flush_word =
        |out: &mut Vec<RawTok>, segs: &mut Vec<(String, bool)>, had_quote: &mut bool| {
            if !segs.is_empty() {
                out.push(RawTok::Word(Word { segs: std::mem::take(segs) }));
            }
            *had_quote = false;
        };

    for c in input.chars() {
        match c {
            '"' => {
                flush_seg(&mut segs, &mut cur, in_quote);
                in_quote = !in_quote;
                had_quote = true;
            }
            '(' | ')' if !in_quote => {
                flush_seg(&mut segs, &mut cur, false);
                flush_word(&mut out, &mut segs, &mut had_quote);
                out.push(if c == '(' { RawTok::LParen } else { RawTok::RParen });
            }
            c if c.is_whitespace() && !in_quote => {
                flush_seg(&mut segs, &mut cur, false);
                flush_word(&mut out, &mut segs, &mut had_quote);
            }
            c => cur.push(c),
        }
    }
    flush_seg(&mut segs, &mut cur, in_quote);
    let _ = had_quote;
    if !segs.is_empty() {
        out.push(RawTok::Word(Word { segs }));
    }
    out
}

const OPS: &[(&str, OpCode)] = &[
    ("!=", OpCode::NotEq),
    ("!:", OpCode::NotContains),
    ("=", OpCode::Eq),
    (":", OpCode::Colon),
];

/// Find the earliest operator code in the first unquoted run of a word, so
/// `title:"a b"` splits on the `:` but `"title:x"` stays free text.
fn split_op(word: &Word) -> Option<(String, OpCode, Word)> {
    let (first, quoted) = word.segs.first()?;
    if *quoted {
        return None;
    }
    let (idx, pat, op) = OPS
        .iter()
        .filter_map(|(pat, op)| first.find(pat).map(|i| (i, *pat, *op)))
        .min_by_key(|(i, pat, _)| (*i, std::cmp::Reverse(pat.len())))?;
    let field = first[..idx].to_string();
    let mut rest_segs = vec![(first[idx + pat.len()..].to_string(), false)];
    rest_segs.extend(word.segs.iter().skip(1).cloned());
    rest_segs.retain(|(s, q)| !s.is_empty() || *q);
    Some((field, op, Word { segs: rest_segs }))
}

/// Split a value word into individual values on unquoted commas; quoted runs
/// are atomic so `"a, b"` stays one value.
fn split_values(word: &Word) -> Vec<String> {
    let mut values = Vec::new();
    let mut cur = String::new();
    for (seg, quoted) in &word.segs {
        if *quoted {
            cur.push_str(seg);
        } else {
            let mut parts = seg.split(',');
            if let Some(first) = parts.next() {
                cur.push_str(first);
            }
            for part in parts {
                values.push(std::mem::take(&mut cur));
                cur.push_str(part);
            }
        }
    }
    values.push(cur);
    values.into_iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
}

fn push_text(out: &mut Vec<Tok>, joined: String) {
    if !joined.trim().is_empty() {
        out.push(Tok::Text(joined));
    }
}

fn strip_negation(word: &Word) -> Option<Word> {
    let (first, quoted) = word.segs.first()?;
    if *quoted {
        return None;
    }
    let rest = first.strip_prefix("not:").or_else(|| first.strip_prefix('-'))?;
    let mut segs = word.segs.clone();
    segs[0].0 = rest.to_string();
    segs.retain(|(s, q)| !s.is_empty() || *q);
    Some(Word { segs })
}

fn classify_word(word: &Word, out: &mut Vec<Tok>) {
    if word.fully_quoted() {
        return push_text(out, word.joined());
    }
    if let Some(plain) = word.plain() {
        match plain {
            "and" | "AND" | "+" | "&&" => return out.push(Tok::And),
            "or" | "OR" | "||" => return out.push(Tok::Or),
            "not" | "NOT" => return out.push(Tok::Not),
            "-" => return out.push(Tok::Text(plain.to_string())),
            _ => {}
        }
    }
    if let Some(rest) = strip_negation(word) {
        out.push(Tok::Not);
        classify_word(&rest, out);
        return;
    }
    if let Some((field, op, value)) = split_op(word)
        && registry::resolve(&field).is_some()
    {
        out.push(Tok::Field { field, op, values: split_values(&value) });
        return;
    }
    push_text(out, word.joined());
}

/// Group raw tokens, assembling tsumikit `field in (v1, v2)` lists into a
/// single Field token.
fn tokenize(input: &str) -> Vec<Tok> {
    let raw = raw_lex(input);
    let mut toks = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        match &raw[i] {
            RawTok::LParen => toks.push(Tok::LParen),
            RawTok::RParen => toks.push(Tok::RParen),
            RawTok::Word(w) => {
                if let Some((field, consumed)) = match_in_list_head(&raw, i, w) {
                    let (values, end) = collect_in_list(&raw, i + consumed);
                    toks.push(Tok::Field { field, op: OpCode::In, values });
                    i = end;
                    continue;
                }
                classify_word(w, &mut toks);
            }
        }
        i += 1;
    }
    toks
}

fn match_in_list_head(raw: &[RawTok], i: usize, w: &Word) -> Option<(String, usize)> {
    let field = w.plain()?;
    registry::resolve(field)?;
    match (raw.get(i + 1), raw.get(i + 2)) {
        (Some(RawTok::Word(kw)), Some(RawTok::LParen))
            if kw.plain().is_some_and(|k| k.eq_ignore_ascii_case("in")) =>
        {
            Some((field.to_string(), 3))
        }
        _ => None,
    }
}

fn collect_in_list(raw: &[RawTok], start: usize) -> (Vec<String>, usize) {
    let mut values = Vec::new();
    let mut i = start;
    while i < raw.len() {
        match &raw[i] {
            RawTok::RParen => {
                i += 1;
                break;
            }
            RawTok::Word(w) => values.extend(split_values(w)),
            RawTok::LParen => {}
        }
        i += 1;
    }
    (values, i)
}

fn field_leaf(field: &str, op: OpCode, raw_values: Vec<String>) -> Node {
    let def = registry::resolve(field).expect("field pre-validated by tokenizer");
    if raw_values.is_empty() {
        // Mid-typing (`machine=`, `title:""`): neutral, constrains nothing.
        return Node::Empty;
    }
    let values: Vec<String> = if def.ty == FieldType::Bool {
        raw_values.iter().map(|v| bool_value(v).to_string()).collect()
    } else {
        raw_values
    };
    let ast_op = match (op, values.len()) {
        (OpCode::In, _) | (_, 2..) => FilterOp::In,
        (OpCode::Eq, _) => FilterOp::Eq,
        (OpCode::Colon | OpCode::NotEq | OpCode::NotContains, _) => def.default_op,
    };
    let node =
        Node::Filter { filter: Filter { field: def.name.to_string(), op: ast_op, values } };
    if matches!(op, OpCode::NotEq | OpCode::NotContains) {
        Node::Not { child: Box::new(node) }
    } else {
        node
    }
}

fn bool_value(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "true" | "1" | "yes" | "y" | "on" | "pinned" | "starred")
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Option<Node> {
        let mut children = Vec::new();
        if let Some(n) = self.parse_and() {
            children.push(n);
        }
        while matches!(self.peek(), Some(Tok::Or)) {
            self.bump();
            if let Some(n) = self.parse_and() {
                children.push(n);
            }
        }
        collapse(children, true)
    }

    fn parse_and(&mut self) -> Option<Node> {
        let mut children = Vec::new();
        loop {
            match self.peek() {
                None | Some(Tok::Or | Tok::RParen) => break,
                Some(Tok::And) => {
                    self.bump();
                }
                _ => {
                    if let Some(n) = self.parse_unary() {
                        children.push(n);
                    } else {
                        break;
                    }
                }
            }
        }
        collapse(children, false)
    }

    fn parse_unary(&mut self) -> Option<Node> {
        if matches!(self.peek(), Some(Tok::Not)) {
            self.bump();
            return self.parse_unary().map(|child| match child {
                Node::Empty => Node::Empty,
                child => Node::Not { child: Box::new(child) },
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Option<Node> {
        match self.bump()? {
            Tok::LParen => {
                let inner = self.parse_or();
                if matches!(self.peek(), Some(Tok::RParen)) {
                    self.bump();
                }
                inner
            }
            Tok::RParen => None,
            Tok::Text(t) => Some(Node::Text { value: t }),
            Tok::Field { field, op, values } => Some(field_leaf(&field, op, values)),
            Tok::And | Tok::Or | Tok::Not => self.parse_primary(),
        }
    }
}

fn collapse(children: Vec<Node>, or: bool) -> Option<Node> {
    let mut children: Vec<Node> =
        children.into_iter().filter(|n| !matches!(n, Node::Empty)).collect();
    match children.len() {
        0 => None,
        1 => children.pop(),
        _ => Some(if or { Node::Or { children } } else { Node::And { children } }),
    }
}

#[must_use]
pub fn parse(input: &str) -> Node {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Node::Empty;
    }
    let toks = tokenize(trimmed);
    let mut parser = Parser { toks, pos: 0 };
    let mut nodes = Vec::new();
    while parser.peek().is_some() {
        let before = parser.pos;
        if let Some(n) = parser.parse_or() {
            nodes.push(n);
        }
        if parser.pos == before {
            parser.pos += 1;
        }
    }
    collapse(nodes, false).unwrap_or(Node::Empty)
}
