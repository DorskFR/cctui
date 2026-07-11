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
    Field(String, String),
}

fn raw_pieces(input: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted_run = false;
    let mut in_quote = false;
    for c in input.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                if in_quote {
                    quoted_run = true;
                }
            }
            '(' | ')' if !in_quote => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), quoted_run));
                    quoted_run = false;
                }
                out.push((c.to_string(), false));
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() || quoted_run {
                    out.push((std::mem::take(&mut cur), quoted_run));
                    quoted_run = false;
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || quoted_run {
        out.push((cur, quoted_run));
    }
    out
}

fn classify(piece: &str, quoted: bool, out: &mut Vec<Tok>) {
    if quoted {
        out.push(Tok::Text(piece.to_string()));
        return;
    }
    match piece {
        "(" => out.push(Tok::LParen),
        ")" => out.push(Tok::RParen),
        "and" | "AND" | "+" | "&&" => out.push(Tok::And),
        "or" | "OR" | "||" => out.push(Tok::Or),
        _ => {
            if let Some(rest) = piece.strip_prefix("not:") {
                out.push(Tok::Not);
                if !rest.is_empty() {
                    classify(rest, false, out);
                }
            } else if let Some(rest) = piece.strip_prefix('-') {
                if rest.is_empty() {
                    out.push(Tok::Text(piece.to_string()));
                } else {
                    out.push(Tok::Not);
                    classify(rest, false, out);
                }
            } else if let Some((field, value)) = piece.split_once(':') {
                if registry::resolve(field).is_some() && !value.is_empty() {
                    out.push(Tok::Field(field.to_string(), value.to_string()));
                } else {
                    out.push(Tok::Text(piece.to_string()));
                }
            } else {
                out.push(Tok::Text(piece.to_string()));
            }
        }
    }
}

fn tokenize(input: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    for (piece, quoted) in raw_pieces(input) {
        classify(&piece, quoted, &mut toks);
    }
    toks
}

fn field_leaf(field: &str, value: &str) -> Node {
    let def = registry::resolve(field).expect("field pre-validated by tokenizer");
    let raw_values: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    if raw_values.is_empty() {
        return Node::Text { value: format!("{field}:{value}") };
    }
    let values = if def.ty == FieldType::Bool {
        raw_values.iter().map(|v| bool_value(v).to_string()).collect()
    } else {
        raw_values
    };
    let op = if values.len() > 1 { FilterOp::In } else { def.default_op };
    Node::Filter { filter: Filter { field: def.name.to_string(), op, values } }
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
            return self.parse_unary().map(|child| Node::Not { child: Box::new(child) });
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
            Tok::Field(f, v) => Some(field_leaf(&f, &v)),
            Tok::And | Tok::Or | Tok::Not => self.parse_primary(),
        }
    }
}

fn collapse(mut children: Vec<Node>, or: bool) -> Option<Node> {
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
