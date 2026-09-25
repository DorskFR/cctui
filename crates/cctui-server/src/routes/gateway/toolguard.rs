//! Tool calls whose input matches the account's policy never reach the harness.
//!
//! From a message's first tool call to its end the response is held and then
//! scanned; clean bytes are released verbatim. A match becomes an explanation
//! text block and a normal end of turn, never an HTTP error: harnesses retry
//! errors and would regenerate the same call.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use dashmap::DashMap;
use futures_util::{Stream, StreamExt};
use regex::Regex;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::AppState;

const POLICY_TTL: Duration = Duration::from_secs(30);
const MAX_PATTERN_LEN: usize = 512;
const MAX_ENTRIES: usize = 256;
const MAX_DECODE_DEPTH: u8 = 4;

static CROSS_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([\w.-]+)/([\w.-]+)#\d+").expect("static regex"));
static GITHUB_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)github\.com/([\w.-]+)/([\w.-]+)/(?:pull|pulls|issues|commit|commits|blob|tree|compare|discussions)\b")
        .expect("static regex")
});

/// The stored, editable form of an account's policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ToolPolicy {
    /// Case-insensitive literals.
    #[serde(default)]
    pub terms: Vec<String>,
    /// Case-insensitive regular expressions.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// GitHub owners whose `owner/repo#n` references and PR/issue URLs are
    /// blocked.
    #[serde(default)]
    pub protected_owners: Vec<String>,
    /// Sessions whose cwd is under one of these roots are not scanned.
    #[serde(default)]
    pub exempt_roots: Vec<String>,
}

impl ToolPolicy {
    /// Trim, drop blanks and duplicates, and reject what would not compile.
    pub fn normalized(self) -> Result<Self, String> {
        fn clean(list: Vec<String>, what: &str) -> Result<Vec<String>, String> {
            let mut out: Vec<String> = Vec::new();
            for s in list {
                let s = s.trim().to_owned();
                if !s.is_empty() && !out.contains(&s) {
                    out.push(s);
                }
            }
            if out.len() > MAX_ENTRIES {
                return Err(format!("at most {MAX_ENTRIES} {what}"));
            }
            Ok(out)
        }
        let policy = Self {
            terms: clean(self.terms, "terms")?,
            patterns: clean(self.patterns, "patterns")?,
            protected_owners: clean(self.protected_owners, "protected owners")?,
            exempt_roots: clean(self.exempt_roots, "exempt roots")?
                .into_iter()
                .map(|r| r.trim_end_matches('/').to_owned())
                .filter(|r| !r.is_empty())
                .collect(),
        };
        for p in &policy.patterns {
            if p.len() > MAX_PATTERN_LEN {
                return Err(format!("pattern longer than {MAX_PATTERN_LEN} characters"));
            }
            compile_pattern(p).map_err(|e| format!("invalid pattern {p:?}: {e}"))?;
        }
        if let Some(r) = policy.exempt_roots.iter().find(|r| !r.starts_with('/')) {
            return Err(format!("exempt root {r:?} must be an absolute path"));
        }
        Ok(policy)
    }

    pub fn is_inert(&self) -> bool {
        self.terms.is_empty() && self.patterns.is_empty() && self.protected_owners.is_empty()
    }
}

fn compile_pattern(p: &str) -> Result<Regex, regex::Error> {
    regex::RegexBuilder::new(p).case_insensitive(true).size_limit(1 << 20).build()
}

#[derive(Debug)]
pub struct CompiledPolicy {
    terms: Vec<String>,
    patterns: Vec<Regex>,
    owners: Vec<String>,
    exempt_roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rule: &'static str,
    pub matched: String,
}

impl CompiledPolicy {
    /// `None` when the policy matches nothing, so no guard is installed.
    pub fn compile(p: &ToolPolicy) -> Option<Self> {
        if p.is_inert() {
            return None;
        }
        Some(Self {
            terms: p.terms.iter().map(|t| t.to_lowercase()).filter(|t| !t.is_empty()).collect(),
            patterns: p.patterns.iter().filter_map(|s| compile_pattern(s).ok()).collect(),
            owners: p.protected_owners.iter().map(|o| o.to_lowercase()).collect(),
            exempt_roots: p.exempt_roots.clone(),
        })
    }

    pub fn exempts(&self, cwd: Option<&str>) -> bool {
        let Some(cwd) = cwd.map(|c| c.trim_end_matches('/')) else { return false };
        self.exempt_roots.iter().any(|root| {
            cwd == root || cwd.strip_prefix(root.as_str()).is_some_and(|rest| rest.starts_with('/'))
        })
    }

    pub fn scan_str(&self, s: &str) -> Option<Hit> {
        if !self.terms.is_empty() {
            let lower = s.to_lowercase();
            if let Some(t) = self.terms.iter().find(|t| lower.contains(t.as_str())) {
                return Some(Hit { rule: "denylist", matched: t.clone() });
            }
        }
        if let Some(m) = self.patterns.iter().find_map(|re| re.find(s)) {
            return Some(Hit { rule: "pattern", matched: m.as_str().to_owned() });
        }
        if !self.owners.is_empty() {
            for re in [&*CROSS_REF, &*GITHUB_URL] {
                for caps in re.captures_iter(s) {
                    if self.owners.contains(&caps[1].to_lowercase()) {
                        return Some(Hit { rule: "cross_repo", matched: caps[0].to_owned() });
                    }
                }
            }
        }
        None
    }

    /// Every string value, recursively. Strings that hold JSON (function-call
    /// `arguments`) are also decoded and scanned.
    pub fn scan_value(&self, v: &Value) -> Option<Hit> {
        self.scan_value_at(v, 0)
    }

    fn scan_value_at(&self, v: &Value, depth: u8) -> Option<Hit> {
        match v {
            Value::String(s) => self.scan_str(s).or_else(|| {
                let t = s.trim_start();
                if depth >= MAX_DECODE_DEPTH || !(t.starts_with('{') || t.starts_with('[')) {
                    return None;
                }
                serde_json::from_str::<Value>(s).ok().and_then(|d| self.scan_value_at(&d, depth + 1))
            }),
            Value::Array(a) => a.iter().find_map(|x| self.scan_value_at(x, depth)),
            Value::Object(o) => o.values().find_map(|x| self.scan_value_at(x, depth)),
            _ => None,
        }
    }
}

pub fn mask(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    match chars.len() {
        0 => String::new(),
        n @ 1..=2 => "*".repeat(n),
        n => format!("{}{}{}", chars[0], "*".repeat((n - 2).min(8)), chars[n - 1]),
    }
}

pub fn explanation(tool: &str, hit: &Hit) -> String {
    format!(
        "⛔ cctui blocked a {tool} call: it contains a forbidden term (\"{}\"). Rewrite it without internal references.",
        mask(&hit.matched)
    )
}

/// One refused tool call, as recorded and announced. Never carries the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub tool_name: String,
    pub rule: String,
    pub input_sha256: String,
}

impl Block {
    fn new(tool: &str, hit: &Hit, input: &str) -> Self {
        Self {
            tool_name: tool.to_owned(),
            rule: format!("{}:{}", hit.rule, mask(&hit.matched)),
            input_sha256: crate::auth::sha256_hex(input),
        }
    }
}

// ---- streaming (SSE) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    Anthropic,
    Responses,
    Chat,
}

struct SseEvent {
    raw: Vec<u8>,
    name: Option<String>,
    data: String,
}

impl SseEvent {
    fn parse(raw: Vec<u8>) -> Self {
        let text = String::from_utf8_lossy(&raw);
        let mut name = None;
        let mut data: Option<String> = None;
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(v) = line.strip_prefix("event:") {
                name = Some(v.strip_prefix(' ').unwrap_or(v).to_owned());
            } else if let Some(v) = line.strip_prefix("data:") {
                let v = v.strip_prefix(' ').unwrap_or(v);
                match &mut data {
                    Some(d) => {
                        d.push('\n');
                        d.push_str(v);
                    }
                    None => data = Some(v.to_owned()),
                }
            }
        }
        Self { raw, name, data: data.unwrap_or_default() }
    }

    fn json(&self) -> Option<Value> {
        let d = self.data.trim();
        if d.is_empty() || d == "[DONE]" {
            return None;
        }
        serde_json::from_str(d).ok()
    }
}

/// End of the next complete SSE event in `buf` (exclusive, terminator included).
fn event_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\n' {
            match (buf.get(i + 1), buf.get(i + 2)) {
                (Some(b'\n'), _) => return Some(i + 2),
                (Some(b'\r'), Some(b'\n')) => return Some(i + 3),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn frame(name: Option<&str>, data: &Value) -> Vec<u8> {
    let mut s = String::new();
    if let Some(n) = name {
        s.push_str("event: ");
        s.push_str(n);
        s.push('\n');
    }
    s.push_str("data: ");
    s.push_str(&data.to_string());
    s.push_str("\n\n");
    s.into_bytes()
}

#[derive(Default)]
struct Call {
    key: String,
    position: i64,
    name: String,
    initial: Option<Value>,
    input: String,
    done_item: Option<Value>,
}

impl Call {
    fn scan(&self, policy: &CompiledPolicy) -> Option<(Hit, String)> {
        let decoded = (!self.input.is_empty())
            .then(|| serde_json::from_str::<Value>(&self.input).ok())
            .flatten();
        let hit = self
            .done_item
            .as_ref()
            .and_then(|v| policy.scan_value(v))
            .or_else(|| decoded.as_ref().and_then(|v| policy.scan_value(v)))
            .or_else(|| (decoded.is_none() && !self.input.is_empty()).then(|| policy.scan_str(&self.input)).flatten())
            .or_else(|| self.initial.as_ref().and_then(|v| policy.scan_value(v)))?;
        let input = if self.input.is_empty() {
            self.done_item.as_ref().or(self.initial.as_ref()).map(Value::to_string).unwrap_or_default()
        } else {
            self.input.clone()
        };
        Some((hit, input))
    }
}

const ANTHROPIC_TOOL_BLOCKS: &[&str] = &["tool_use", "server_tool_use", "mcp_tool_use"];
const RESPONSES_TOOL_ITEMS: &[&str] = &[
    "function_call",
    "custom_tool_call",
    "local_shell_call",
    "shell_call",
    "apply_patch_call",
    "mcp_call",
];
const RESPONSES_END: &[&str] = &["response.completed", "response.incomplete", "response.failed"];

/// Streaming holdback over one SSE response body.
pub struct SseGuard {
    policy: Arc<CompiledPolicy>,
    pending: Vec<u8>,
    held: Vec<SseEvent>,
    wire: Option<Wire>,
    calls: Vec<Call>,
    blocks: Vec<Block>,
}

enum Step {
    Pass,
    Hold,
    End,
}

impl SseGuard {
    pub fn new(policy: Arc<CompiledPolicy>) -> Self {
        Self { policy, pending: Vec::new(), held: Vec::new(), wire: None, calls: Vec::new(), blocks: Vec::new() }
    }

    pub fn take_blocks(&mut self) -> Vec<Block> {
        std::mem::take(&mut self.blocks)
    }

    fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = event_end(&self.pending) {
            let raw: Vec<u8> = self.pending.drain(..end).collect();
            self.on_event(SseEvent::parse(raw), &mut out);
        }
        out
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let raw = std::mem::take(&mut self.pending);
            self.on_event(SseEvent::parse(raw), &mut out);
        }
        if self.holding() {
            self.resolve(None, &mut out);
        }
        out
    }

    fn on_event(&mut self, ev: SseEvent, out: &mut Vec<u8>) {
        let json = ev.json();
        let done_marker = ev.data.trim() == "[DONE]";
        match self.classify(json.as_ref(), done_marker) {
            Step::Pass => out.extend_from_slice(&ev.raw),
            Step::Hold => self.held.push(ev),
            Step::End => self.resolve(Some(ev), out),
        }
    }

    fn call_mut(&mut self, key: &str) -> Option<&mut Call> {
        self.calls.iter_mut().find(|c| c.key == key)
    }

    fn start_call(&mut self, wire: Wire, call: Call) {
        self.wire.get_or_insert(wire);
        if self.call_mut(&call.key).is_none() {
            self.calls.push(call);
        }
    }

    fn classify(&mut self, json: Option<&Value>, done_marker: bool) -> Step {
        let holding = self.holding();
        if done_marker {
            return if holding { Step::End } else { Step::Pass };
        }
        let Some(v) = json else { return if holding { Step::Hold } else { Step::Pass } };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");

        if let Some(choices) = v.get("choices").and_then(Value::as_array) {
            let mut started = false;
            let mut finished = false;
            for choice in choices {
                let ci = choice.get("index").and_then(Value::as_i64).unwrap_or(0);
                if let Some(tcs) = choice.pointer("/delta/tool_calls").and_then(Value::as_array) {
                    for tc in tcs {
                        let ti = tc.get("index").and_then(Value::as_i64).unwrap_or(0);
                        let key = format!("{ci}:{ti}");
                        let name = tc.pointer("/function/name").and_then(Value::as_str);
                        let frag = tc.pointer("/function/arguments").and_then(Value::as_str);
                        self.start_call(Wire::Chat, Call {
                            key: key.clone(),
                            position: ci * 10_000 + ti,
                            ..Call::default()
                        });
                        if let Some(c) = self.call_mut(&key) {
                            if let Some(n) = name.filter(|n| !n.is_empty()) {
                                c.name = n.to_owned();
                            }
                            if let Some(f) = frag {
                                c.input.push_str(f);
                            }
                        }
                        started = true;
                    }
                }
                if choice.get("finish_reason").is_some_and(|f| !f.is_null()) {
                    finished = true;
                }
            }
            return match (started || holding, finished) {
                (true, true) => Step::End,
                (true, false) => Step::Hold,
                (false, _) => Step::Pass,
            };
        }

        match ty {
            "content_block_start" => {
                let block = v.get("content_block");
                let bty = block.and_then(|b| b.get("type")).and_then(Value::as_str).unwrap_or("");
                if ANTHROPIC_TOOL_BLOCKS.contains(&bty) {
                    let idx = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                    self.start_call(Wire::Anthropic, Call {
                        key: idx.to_string(),
                        position: idx,
                        name: block
                            .and_then(|b| b.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or(bty)
                            .to_owned(),
                        initial: block.and_then(|b| b.get("input")).cloned(),
                        ..Call::default()
                    });
                    return Step::Hold;
                }
            }
            "content_block_delta" => {
                let key = v.get("index").and_then(Value::as_i64).unwrap_or(0).to_string();
                if let Some(frag) = v.pointer("/delta/partial_json").and_then(Value::as_str)
                    && let Some(c) = self.call_mut(&key)
                {
                    c.input.push_str(frag);
                }
            }
            "message_delta" | "message_stop" if holding => return Step::End,
            "response.output_item.added" => {
                let item = v.get("item");
                let ity = item.and_then(|i| i.get("type")).and_then(Value::as_str).unwrap_or("");
                if RESPONSES_TOOL_ITEMS.contains(&ity) {
                    let idx = v.get("output_index").and_then(Value::as_i64).unwrap_or(0);
                    self.start_call(Wire::Responses, Call {
                        key: idx.to_string(),
                        position: idx,
                        name: item
                            .and_then(|i| i.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or(ity)
                            .to_owned(),
                        initial: item.cloned(),
                        ..Call::default()
                    });
                    return Step::Hold;
                }
            }
            "response.function_call_arguments.delta"
            | "response.custom_tool_call_input.delta"
            | "response.mcp_call_arguments.delta" => {
                let key = v.get("output_index").and_then(Value::as_i64).unwrap_or(0).to_string();
                if let Some(frag) = v.get("delta").and_then(Value::as_str)
                    && let Some(c) = self.call_mut(&key)
                {
                    c.input.push_str(frag);
                }
            }
            "response.output_item.done" => {
                let key = v.get("output_index").and_then(Value::as_i64).unwrap_or(0).to_string();
                if let Some(c) = self.call_mut(&key) {
                    c.done_item = v.get("item").cloned();
                }
            }
            t if holding && RESPONSES_END.contains(&t) => return Step::End,
            _ => {}
        }
        if holding { Step::Hold } else { Step::Pass }
    }

    fn resolve(&mut self, end: Option<SseEvent>, out: &mut Vec<u8>) {
        let mut calls = std::mem::take(&mut self.calls);
        calls.sort_by_key(|c| c.position);
        let held = std::mem::take(&mut self.held);
        let wire = self.wire.take();
        let hit = calls.iter().find_map(|c| c.scan(&self.policy).map(|(h, input)| (c, h, input)));
        let Some((call, hit, input)) = hit else {
            for ev in held.into_iter().chain(end) {
                out.extend_from_slice(&ev.raw);
            }
            return;
        };
        self.blocks.push(Block::new(&call.name, &hit, &input));
        let text = explanation(&call.name, &hit);
        let first = calls.first().map_or(0, |c| c.position);
        let named = held.first().is_some_and(|e| e.name.is_some());
        let end_json = end.as_ref().and_then(SseEvent::json);
        match wire {
            Some(Wire::Anthropic) | None => {
                anthropic_rewrite(out, first, &text, end.as_ref(), end_json);
            }
            Some(Wire::Responses) => {
                let seq = held
                    .first()
                    .and_then(SseEvent::json)
                    .and_then(|v| v.get("sequence_number").and_then(Value::as_i64));
                responses_rewrite(out, named, seq, first, &text, end.as_ref(), end_json);
            }
            Some(Wire::Chat) => {
                let template = held.first().and_then(SseEvent::json);
                chat_rewrite(out, template, &text, end.as_ref(), end_json);
            }
        }
    }
}

fn anthropic_rewrite(
    out: &mut Vec<u8>,
    index: i64,
    text: &str,
    end: Option<&SseEvent>,
    end_json: Option<Value>,
) {
    let ev = |out: &mut Vec<u8>, v: Value| {
        let name = v.get("type").and_then(Value::as_str).map(str::to_owned);
        out.extend(frame(name.as_deref(), &v));
    };
    ev(out, json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}));
    ev(out, json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}}));
    ev(out, json!({"type": "content_block_stop", "index": index}));
    match end_json {
        Some(mut v) if v.get("type").and_then(Value::as_str) == Some("message_delta") => {
            if let Some(d) = v.get_mut("delta").and_then(Value::as_object_mut) {
                d.insert("stop_reason".into(), json!("end_turn"));
                d.insert("stop_sequence".into(), Value::Null);
            }
            ev(out, v);
        }
        _ => {
            ev(out, json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 0}}));
            match end {
                Some(e) => out.extend_from_slice(&e.raw),
                None => ev(out, json!({"type": "message_stop"})),
            }
        }
    }
}

fn blocked_message_item(text: &str, status: &str) -> Value {
    let content = if status == "completed" {
        json!([{"type": "output_text", "text": text, "annotations": []}])
    } else {
        json!([])
    };
    json!({"id": "msg_cctui_blocked", "type": "message", "status": status, "role": "assistant", "content": content})
}

fn responses_rewrite(
    out: &mut Vec<u8>,
    named: bool,
    seq: Option<i64>,
    output_index: i64,
    text: &str,
    end: Option<&SseEvent>,
    end_json: Option<Value>,
) {
    let mut next = seq;
    let mut ev = |out: &mut Vec<u8>, mut v: Value| {
        if let Some(n) = next.as_mut() {
            v["sequence_number"] = json!(*n);
            *n += 1;
        }
        let name = named.then(|| v.get("type").and_then(Value::as_str).map(str::to_owned)).flatten();
        out.extend(frame(name.as_deref(), &v));
    };
    let item_id = "msg_cctui_blocked";
    let part = json!({"type": "output_text", "text": text, "annotations": []});
    ev(out, json!({"type": "response.output_item.added", "output_index": output_index, "item": blocked_message_item(text, "in_progress")}));
    ev(out, json!({"type": "response.content_part.added", "item_id": item_id, "output_index": output_index, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}));
    ev(out, json!({"type": "response.output_text.delta", "item_id": item_id, "output_index": output_index, "content_index": 0, "delta": text}));
    ev(out, json!({"type": "response.output_text.done", "item_id": item_id, "output_index": output_index, "content_index": 0, "text": text}));
    ev(out, json!({"type": "response.content_part.done", "item_id": item_id, "output_index": output_index, "content_index": 0, "part": part}));
    ev(out, json!({"type": "response.output_item.done", "output_index": output_index, "item": blocked_message_item(text, "completed")}));
    match end_json {
        Some(mut v) if v.get("response").is_some() => {
            if let Some(output) = v.pointer_mut("/response/output").and_then(Value::as_array_mut) {
                output.retain(|item| {
                    let t = item.get("type").and_then(Value::as_str).unwrap_or("");
                    !RESPONSES_TOOL_ITEMS.contains(&t)
                });
                output.push(blocked_message_item(text, "completed"));
            }
            ev(out, v);
        }
        _ => {
            if let Some(e) = end {
                out.extend_from_slice(&e.raw);
            }
        }
    }
}

fn chat_rewrite(
    out: &mut Vec<u8>,
    template: Option<Value>,
    text: &str,
    end: Option<&SseEvent>,
    end_json: Option<Value>,
) {
    let mut base = serde_json::Map::new();
    if let Some(Value::Object(t)) = &template {
        for k in ["id", "object", "created", "model", "system_fingerprint"] {
            if let Some(v) = t.get(k) {
                base.insert(k.into(), v.clone());
            }
        }
    }
    let mut content = Value::Object(base.clone());
    content["choices"] = json!([{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]);
    out.extend(frame(None, &content));
    match end_json {
        Some(mut v) if v.get("choices").is_some() => {
            if let Some(choices) = v.get_mut("choices").and_then(Value::as_array_mut) {
                for c in choices {
                    if let Some(d) = c.get_mut("delta").and_then(Value::as_object_mut) {
                        d.remove("tool_calls");
                        d.remove("function_call");
                    }
                    c["finish_reason"] = json!("stop");
                }
            }
            out.extend(frame(None, &v));
        }
        _ => {
            let mut fin = Value::Object(base);
            fin["choices"] = json!([{"index": 0, "delta": {}, "finish_reason": "stop"}]);
            out.extend(frame(None, &fin));
            if let Some(e) = end {
                out.extend_from_slice(&e.raw);
            }
        }
    }
}

// ---- non-streaming JSON ----

/// Rewrite a complete JSON response. `None` = nothing matched; forward the
/// original bytes.
pub fn rewrite_json(policy: &CompiledPolicy, body: &[u8]) -> Option<(Vec<u8>, Block)> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let block;
    if let Some(content) = v.get("content").and_then(Value::as_array) {
        let (first, name, hit, input) = content.iter().enumerate().find_map(|(i, b)| {
            let t = b.get("type").and_then(Value::as_str).unwrap_or("");
            if !ANTHROPIC_TOOL_BLOCKS.contains(&t) {
                return None;
            }
            let input = b.get("input").cloned().unwrap_or(Value::Null);
            let hit = policy.scan_value(&input)?;
            Some((i, b.get("name").and_then(Value::as_str).unwrap_or(t).to_owned(), hit, input))
        })?;
        let first_tool = content
            .iter()
            .position(|b| ANTHROPIC_TOOL_BLOCKS.contains(&b.get("type").and_then(Value::as_str).unwrap_or("")))
            .unwrap_or(first);
        let mut kept: Vec<Value> = content[..first_tool].to_vec();
        kept.push(json!({"type": "text", "text": explanation(&name, &hit)}));
        block = Block::new(&name, &hit, &input.to_string());
        v["content"] = Value::Array(kept);
        v["stop_reason"] = json!("end_turn");
        v["stop_sequence"] = Value::Null;
    } else if let Some(output) = v.get("output").and_then(Value::as_array) {
        let (name, hit, input) = output.iter().find_map(|item| {
            let t = item.get("type").and_then(Value::as_str).unwrap_or("");
            if !RESPONSES_TOOL_ITEMS.contains(&t) {
                return None;
            }
            let hit = policy.scan_value(item)?;
            Some((item.get("name").and_then(Value::as_str).unwrap_or(t).to_owned(), hit, item.to_string()))
        })?;
        let text = explanation(&name, &hit);
        let mut kept: Vec<Value> = output
            .iter()
            .filter(|i| !RESPONSES_TOOL_ITEMS.contains(&i.get("type").and_then(Value::as_str).unwrap_or("")))
            .cloned()
            .collect();
        kept.push(blocked_message_item(&text, "completed"));
        block = Block::new(&name, &hit, &input);
        v["output"] = Value::Array(kept);
    } else if let Some(choices) = v.get("choices").and_then(Value::as_array) {
        let (name, hit, input) = choices.iter().find_map(|c| {
            c.pointer("/message/tool_calls").and_then(Value::as_array)?.iter().find_map(|tc| {
                let hit = policy.scan_value(tc.get("function").unwrap_or(tc))?;
                let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool");
                Some((name.to_owned(), hit, tc.to_string()))
            })
        })?;
        let text = explanation(&name, &hit);
        block = Block::new(&name, &hit, &input);
        if let Some(choices) = v.get_mut("choices").and_then(Value::as_array_mut) {
            for c in choices {
                if let Some(m) = c.get_mut("message").and_then(Value::as_object_mut)
                    && m.remove("tool_calls").is_some()
                {
                    m.insert("content".into(), json!(text));
                    c["finish_reason"] = json!("stop");
                }
            }
        }
    } else {
        return None;
    }
    Some((serde_json::to_vec(&v).ok()?, block))
}

// ---- policy lookup ----

type PolicyCache = DashMap<Uuid, (Instant, Option<(Uuid, Arc<CompiledPolicy>)>)>;
static POLICY_CACHE: LazyLock<PolicyCache> = LazyLock::new(DashMap::new);

/// Drop cached policies after an edit, so this replica applies it at once.
/// Other replicas pick it up within [`POLICY_TTL`].
pub fn invalidate_policy_cache() {
    POLICY_CACHE.clear();
}

async fn policy_for_provider(
    state: &AppState,
    provider_id: Uuid,
) -> Option<(Uuid, Arc<CompiledPolicy>)> {
    if let Some(entry) = POLICY_CACHE.get(&provider_id)
        && entry.0.elapsed() < POLICY_TTL
    {
        return entry.1.clone();
    }
    let row: Result<Option<(Uuid, Vec<String>, Vec<String>, Vec<String>, Vec<String>)>, _> =
        sqlx::query_as(
            "SELECT p.account_id, p.terms, p.patterns, p.protected_owners, p.exempt_roots \
             FROM account_providers ap JOIN account_tool_policies p ON p.account_id = ap.account_id \
             WHERE ap.id = $1",
        )
        .bind(provider_id)
        .fetch_optional(&state.pool)
        .await;
    let compiled = match row {
        Ok(row) => row.and_then(|(account_id, terms, patterns, protected_owners, exempt_roots)| {
            let p = ToolPolicy { terms, patterns, protected_owners, exempt_roots };
            CompiledPolicy::compile(&p).map(|c| (account_id, Arc::new(c)))
        }),
        Err(e) => {
            tracing::warn!(provider = %provider_id, error = %e, "tool policy lookup failed");
            return POLICY_CACHE.get(&provider_id).and_then(|e| e.1.clone());
        }
    };
    POLICY_CACHE.insert(provider_id, (Instant::now(), compiled.clone()));
    compiled
}

/// What the proxy needs to guard one response.
#[derive(Clone)]
pub struct ActiveGuard {
    policy: Arc<CompiledPolicy>,
    account_id: Uuid,
    session_id: Option<String>,
}

/// `None` when the account has no effective policy or the session is exempt.
pub async fn guard_for(
    state: &AppState,
    provider_id: Uuid,
    session_token: &str,
) -> Option<ActiveGuard> {
    let (account_id, policy) = policy_for_provider(state, provider_id).await?;
    let hash = crate::auth::sha256_hex(session_token);
    let session: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT t.session_id, s.working_dir FROM session_tokens t \
         LEFT JOIN sessions s ON s.id = t.session_id WHERE t.token_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (session_id, cwd) = session.map_or((None, None), |(s, c)| (Some(s), c));
    if policy.exempts(cwd.as_deref()) {
        return None;
    }
    Some(ActiveGuard { policy, account_id, session_id })
}

async fn record_block(state: &AppState, guard: &ActiveGuard, block: Block) {
    tracing::warn!(
        account = %guard.account_id,
        session = guard.session_id.as_deref().unwrap_or("-"),
        tool = %block.tool_name,
        rule = %block.rule,
        "gateway blocked a tool call"
    );
    if let Err(e) = sqlx::query(
        "INSERT INTO gateway_blocks (session_id, account_id, tool_name, rule, input_sha256) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(guard.session_id.as_deref())
    .bind(guard.account_id)
    .bind(&block.tool_name)
    .bind(&block.rule)
    .bind(&block.input_sha256)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(error = %e, "failed to record gateway block");
    }
    if let Some(session_id) = &guard.session_id {
        state.bus.publish_server(cctui_proto::ws::ServerEvent::ToolCallBlocked {
            session_id: session_id.clone(),
            tool_name: block.tool_name,
            rule: block.rule,
        });
    }
}

enum Guarded {
    Sse(SseGuard),
    Json(Arc<CompiledPolicy>, Vec<u8>),
}

impl Guarded {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        match self {
            Self::Sse(g) => g.push(chunk),
            Self::Json(_, buf) => {
                buf.extend_from_slice(chunk);
                Vec::new()
            }
        }
    }

    fn finish(&mut self) -> (Vec<u8>, Vec<Block>) {
        match self {
            Self::Sse(g) => {
                let out = g.finish();
                (out, g.take_blocks())
            }
            Self::Json(policy, buf) => {
                let buf = std::mem::take(buf);
                match rewrite_json(policy, &buf) {
                    Some((out, block)) => (out, vec![block]),
                    None => (buf, Vec::new()),
                }
            }
        }
    }

    fn take_blocks(&mut self) -> Vec<Block> {
        match self {
            Self::Sse(g) => g.take_blocks(),
            Self::Json(..) => Vec::new(),
        }
    }
}

/// Whether a response with these headers can be guarded. Compressed bodies
/// cannot be read here; the proxy strips `accept-encoding` while a guard is
/// active so upstreams answer in identity.
pub fn guardable(content_type: Option<&str>, content_encoding: Option<&str>) -> Option<bool> {
    if content_encoding.is_some_and(|e| !e.trim().eq_ignore_ascii_case("identity")) {
        return None;
    }
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.starts_with("text/event-stream") {
        Some(true)
    } else if ct.contains("json") {
        Some(false)
    } else {
        None
    }
}

fn report(state: &AppState, guard: &ActiveGuard, blocks: Vec<Block>) {
    for block in blocks {
        let (state, guard) = (state.clone(), guard.clone());
        tokio::spawn(async move { record_block(&state, &guard, block).await });
    }
}

/// Wrap an upstream body so tool calls pass through the guard.
pub fn guard_stream<S, E>(
    inner: S,
    guard: ActiveGuard,
    sse: bool,
    state: AppState,
) -> impl Stream<Item = Result<Bytes, E>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let body = if sse {
        Guarded::Sse(SseGuard::new(guard.policy.clone()))
    } else {
        Guarded::Json(guard.policy.clone(), Vec::new())
    };
    futures_util::stream::unfold(
        (Box::pin(inner), body, guard, state, false),
        move |(mut inner, mut body, guard, state, done)| async move {
            if done {
                return None;
            }
            loop {
                match inner.next().await {
                    Some(Ok(chunk)) => {
                        let out = body.push(&chunk);
                        report(&state, &guard, body.take_blocks());
                        if !out.is_empty() {
                            return Some((Ok(Bytes::from(out)), (inner, body, guard, state, false)));
                        }
                    }
                    Some(Err(e)) => return Some((Err(e), (inner, body, guard, state, true))),
                    None => {
                        let (out, blocks) = body.finish();
                        report(&state, &guard, blocks);
                        if out.is_empty() {
                            return None;
                        }
                        return Some((Ok(Bytes::from(out)), (inner, body, guard, state, true)));
                    }
                }
            }
        },
    )
}


#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const TERM: &str = "acmecorp";

    fn policy() -> Arc<CompiledPolicy> {
        Arc::new(
            CompiledPolicy::compile(&ToolPolicy {
                terms: vec!["AcmeCorp".into()],
                patterns: vec![r"internal-\d{4}".into()],
                protected_owners: vec!["acme".into()],
                exempt_roots: vec!["/home/u/work".into()],
            })
            .expect("non-empty policy"),
        )
    }

    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Block>) {
        let mut g = SseGuard::new(policy());
        let mut out = Vec::new();
        for c in chunks {
            out.extend(g.push(c));
        }
        out.extend(g.finish());
        (out, g.take_blocks())
    }

    /// Feed `body` in `size`-byte chunks so events and JSON split mid-way.
    fn run_chunked(body: &[u8], size: usize) -> (Vec<u8>, Vec<Block>) {
        let chunks: Vec<&[u8]> = body.chunks(size).collect();
        run(&chunks)
    }

    fn ev(name: Option<&str>, data: &Value) -> String {
        String::from_utf8(frame(name, data)).unwrap()
    }

    fn events(body: &[u8]) -> Vec<(Option<String>, String)> {
        let mut buf = body.to_vec();
        let mut out = Vec::new();
        while let Some(end) = event_end(&buf) {
            let raw: Vec<u8> = buf.drain(..end).collect();
            let e = SseEvent::parse(raw);
            out.push((e.name, e.data));
        }
        assert!(buf.iter().all(u8::is_ascii_whitespace), "trailing partial event");
        out
    }

    // ---- Anthropic ----

    fn anthropic(text: &str, tools: &[(&str, &[&str])]) -> Vec<u8> {
        let mut s = String::new();
        let mut e = |v: Value| s.push_str(&ev(v["type"].as_str(), &v));
        e(json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-opus-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":1}}}));
        e(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}));
        e(json!({"type":"ping"}));
        e(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}));
        e(json!({"type":"content_block_stop","index":0}));
        for (i, (name, frags)) in tools.iter().enumerate() {
            let idx = i + 1;
            e(json!({"type":"content_block_start","index":idx,"content_block":{"type":"tool_use","id":format!("toolu_{idx}"),"name":name,"input":{}}}));
            for f in *frags {
                e(json!({"type":"content_block_delta","index":idx,"delta":{"type":"input_json_delta","partial_json":f}}));
            }
            e(json!({"type":"content_block_stop","index":idx}));
        }
        let stop = if tools.is_empty() { "end_turn" } else { "tool_use" };
        e(json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":42}}));
        e(json!({"type":"message_stop"}));
        s.into_bytes()
    }

    /// Reassemble the message the way the SDK's stream accumulator does,
    /// asserting the framing is valid: every block opened before its deltas,
    /// closed exactly once, indices unique, `message_stop` last.
    fn parse_anthropic(body: &[u8]) -> (Vec<Value>, String, Value) {
        let mut blocks: Vec<Value> = Vec::new();
        let mut open: Option<usize> = None;
        let mut stop_reason = String::new();
        let mut usage = Value::Null;
        let mut stopped = false;
        let mut json_bufs: HashMap<usize, String> = HashMap::new();
        for (name, data) in events(body) {
            assert!(!stopped, "event after message_stop");
            let v: Value = serde_json::from_str(&data).expect("event data is JSON");
            let ty = v["type"].as_str().unwrap().to_owned();
            assert_eq!(name.as_deref(), Some(ty.as_str()), "event name matches data type");
            match ty.as_str() {
                "content_block_start" => {
                    assert!(open.is_none(), "block opened while another is open");
                    let idx = v["index"].as_u64().unwrap() as usize;
                    assert_eq!(idx, blocks.len(), "indices are contiguous");
                    blocks.push(v["content_block"].clone());
                    open = Some(idx);
                }
                "content_block_delta" => {
                    let idx = v["index"].as_u64().unwrap() as usize;
                    assert_eq!(open, Some(idx), "delta for the open block");
                    match v["delta"]["type"].as_str().unwrap() {
                        "text_delta" => {
                            let t = format!(
                                "{}{}",
                                blocks[idx]["text"].as_str().unwrap(),
                                v["delta"]["text"].as_str().unwrap()
                            );
                            blocks[idx]["text"] = json!(t);
                        }
                        "input_json_delta" => json_bufs
                            .entry(idx)
                            .or_default()
                            .push_str(v["delta"]["partial_json"].as_str().unwrap()),
                        other => panic!("unexpected delta {other}"),
                    }
                }
                "content_block_stop" => {
                    let idx = v["index"].as_u64().unwrap() as usize;
                    assert_eq!(open.take(), Some(idx), "stop closes the open block");
                    if let Some(j) = json_bufs.remove(&idx) {
                        blocks[idx]["input"] = serde_json::from_str(&j).expect("tool input JSON");
                    }
                }
                "message_delta" => {
                    stop_reason = v["delta"]["stop_reason"].as_str().unwrap().to_owned();
                    usage = v["usage"].clone();
                }
                "message_stop" => stopped = true,
                "message_start" | "ping" => {}
                other => panic!("unexpected event {other}"),
            }
        }
        assert!(stopped && open.is_none(), "stream ends with message_stop");
        (blocks, stop_reason, usage)
    }

    fn assert_blocked_anthropic(out: &[u8], tool: &str) {
        let (blocks, stop, usage) = parse_anthropic(out);
        assert_eq!(stop, "end_turn");
        assert_eq!(usage["output_tokens"], 42, "usage survives the rewrite");
        assert!(blocks.iter().all(|b| b["type"] == "text"), "no tool call survives: {blocks:?}");
        let last = blocks.last().unwrap()["text"].as_str().unwrap();
        assert!(last.starts_with(&format!("⛔ cctui blocked a {tool} call")), "{last}");
        assert!(!last.to_lowercase().contains(TERM), "the term itself is masked");
    }

    #[test]
    fn anthropic_clean_tool_call_passes_byte_for_byte() {
        let body = anthropic("hello", &[("Bash", &[r#"{"command":"ls "#, r#"-la"}"#])]);
        for size in [body.len(), 7, 1] {
            let (out, blocks) = run_chunked(&body, size);
            assert_eq!(out, body, "chunk size {size}");
            assert!(blocks.is_empty());
        }
    }

    #[test]
    fn anthropic_text_mention_is_not_blocked() {
        let body = anthropic("we never say AcmeCorp or acme/secret#1 in tools", &[]);
        assert_eq!(run_chunked(&body, 5).0, body);
        let body = anthropic("AcmeCorp", &[("Read", &[r#"{"file_path":"/tmp/x"}"#])]);
        assert_eq!(run_chunked(&body, 5).0, body);
    }

    #[test]
    fn anthropic_match_is_rewritten_into_a_clean_end_of_turn() {
        let body =
            anthropic("ok", &[("Bash", &[r#"{"command":"gh pr create --body 'see AcmeCorp'"}"#])]);
        let (out, blocks) = run_chunked(&body, 11);
        assert_blocked_anthropic(&out, "Bash");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tool_name, "Bash");
        assert!(blocks[0].rule.starts_with("denylist:"));
        assert!(!blocks[0].rule.contains(TERM));
        assert_eq!(blocks[0].input_sha256.len(), 64);
    }

    #[test]
    fn anthropic_term_split_across_deltas_is_caught() {
        let body = anthropic("ok", &[("Bash", &[r#"{"command":"echo acme"#, r#"CORP"}"#])]);
        let (out, blocks) = run_chunked(&body, 3);
        assert_blocked_anthropic(&out, "Bash");
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn anthropic_match_in_a_later_parallel_call_drops_them_all() {
        let body = anthropic(
            "ok",
            &[
                ("Read", &[r#"{"file_path":"/tmp/a"}"#]),
                ("Write", &[r#"{"file_path":"/tmp/body.md","content":"Refs acme/secret#12"}"#]),
                ("Bash", &[r#"{"command":"ls"}"#]),
            ],
        );
        let (out, blocks) = run_chunked(&body, 13);
        assert_blocked_anthropic(&out, "Write");
        let (parsed, ..) = parse_anthropic(&out);
        assert_eq!(parsed.len(), 2, "text + explanation only");
        assert!(blocks[0].rule.starts_with("cross_repo:"));
    }

    #[test]
    fn anthropic_stream_cut_mid_tool_call_still_ends_cleanly() {
        let body = anthropic("ok", &[("Bash", &[r#"{"command":"echo AcmeCorp"}"#])]);
        let full = String::from_utf8(body).unwrap();
        let cut = &full[..full.rfind("event: content_block_stop").unwrap()];
        let (out, blocks) = run(&[cut.as_bytes()]);
        assert_eq!(blocks.len(), 1);
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("tool_use"));
        assert!(text.contains("\"stop_reason\":\"end_turn\""));
        assert!(text.trim_end().ends_with(r#"{"type":"message_stop"}"#));
    }

    #[test]
    fn crlf_framed_events_are_parsed() {
        let body = String::from_utf8(anthropic("ok", &[("Bash", &[r#"{"command":"AcmeCorp"}"#])]))
            .unwrap()
            .replace('\n', "\r\n");
        let (_, blocks) = run_chunked(body.as_bytes(), 4);
        assert_eq!(blocks.len(), 1);
    }

    // ---- OpenAI Responses ----

    fn responses(text: &str, tools: &[(&str, &[&str])]) -> Vec<u8> {
        let mut s = String::new();
        let mut seq = 0;
        let mut e = |mut v: Value| {
            v["sequence_number"] = json!(seq);
            seq += 1;
            s.push_str(&ev(v["type"].as_str(), &v));
        };
        let msg = json!({"id":"msg_1","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]});
        e(json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}));
        e(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message","status":"in_progress","role":"assistant","content":[]}}));
        e(json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":text}));
        e(json!({"type":"response.output_item.done","output_index":0,"item":msg.clone()}));
        let mut output = vec![msg];
        for (i, (name, frags)) in tools.iter().enumerate() {
            let idx = i + 1;
            let id = format!("fc_{idx}");
            e(json!({"type":"response.output_item.added","output_index":idx,"item":{"id":id,"type":"function_call","status":"in_progress","call_id":format!("call_{idx}"),"name":name,"arguments":""}}));
            for f in *frags {
                e(json!({"type":"response.function_call_arguments.delta","item_id":id,"output_index":idx,"delta":f}));
            }
            let args = frags.concat();
            e(json!({"type":"response.function_call_arguments.done","item_id":id,"output_index":idx,"arguments":args}));
            let item = json!({"id":id,"type":"function_call","status":"completed","call_id":format!("call_{idx}"),"name":name,"arguments":args});
            e(json!({"type":"response.output_item.done","output_index":idx,"item":item.clone()}));
            output.push(item);
        }
        e(json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":output,"usage":{"input_tokens":10,"output_tokens":42,"total_tokens":52}}}));
        s.into_bytes()
    }

    /// What codex keeps from the stream: every `output_item.done` in order, and
    /// the terminal `response.completed`, whose output must agree.
    fn parse_responses(body: &[u8]) -> (Vec<Value>, Value) {
        let mut items = Vec::new();
        let mut completed = None;
        let mut last_seq = -1;
        for (name, data) in events(body) {
            let v: Value = serde_json::from_str(&data).expect("event data is JSON");
            assert!(completed.is_none(), "event after response.completed");
            assert_eq!(name.as_deref(), v["type"].as_str());
            let seq = v["sequence_number"].as_i64().expect("sequence_number");
            assert!(seq > last_seq, "sequence numbers increase");
            last_seq = seq;
            match v["type"].as_str().unwrap() {
                "response.output_item.done" => items.push(v["item"].clone()),
                "response.completed" => completed = Some(v["response"].clone()),
                _ => {}
            }
        }
        let completed = completed.expect("stream ends with response.completed");
        (items, completed)
    }

    fn assert_blocked_responses(out: &[u8], tool: &str) {
        let (items, completed) = parse_responses(out);
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["usage"]["output_tokens"], 42);
        for list in [&items, completed["output"].as_array().unwrap()] {
            assert!(list.iter().all(|i| i["type"] == "message"), "no tool call survives: {list:?}");
            let text = list.last().unwrap()["content"][0]["text"].as_str().unwrap();
            assert!(text.starts_with(&format!("⛔ cctui blocked a {tool} call")), "{text}");
        }
    }

    #[test]
    fn responses_clean_tool_call_passes_byte_for_byte() {
        let body = responses("hi AcmeCorp", &[("shell", &[r#"{"command":["ls"#, r#"","-la"]}"#])]);
        for size in [body.len(), 9, 1] {
            let (out, blocks) = run_chunked(&body, size);
            assert_eq!(out, body, "chunk size {size}");
            assert!(blocks.is_empty());
        }
    }

    #[test]
    fn responses_match_is_rewritten_and_split_terms_are_caught() {
        let body = responses("ok", &[("shell", &[r#"{"command":["gh","pr","create","--body","ac"#, r#"meCorp"]}"#])]);
        let (out, blocks) = run_chunked(&body, 6);
        assert_blocked_responses(&out, "shell");
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn responses_match_in_a_later_parallel_call_drops_them_all() {
        let body = responses(
            "ok",
            &[
                ("shell", &[r#"{"command":["ls"]}"#]),
                ("shell", &[r#"{"command":["gh","pr","edit","--body","https://github.com/Acme/secret/pull/7"]}"#]),
            ],
        );
        let (out, blocks) = run_chunked(&body, 17);
        assert_blocked_responses(&out, "shell");
        assert!(blocks[0].rule.starts_with("cross_repo:"));
    }

    #[test]
    fn responses_custom_tool_call_input_is_scanned() {
        let mut s = String::new();
        let mut e = |v: Value| s.push_str(&ev(v["type"].as_str(), &v));
        e(json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":"ct_1","type":"custom_tool_call","call_id":"c1","name":"apply_patch","input":""}}));
        e(json!({"type":"response.custom_tool_call_input.delta","sequence_number":2,"output_index":0,"item_id":"ct_1","delta":"*** Add File: notes.md\n+see acme/"}));
        e(json!({"type":"response.custom_tool_call_input.delta","sequence_number":3,"output_index":0,"item_id":"ct_1","delta":"secret#44\n"}));
        e(json!({"type":"response.completed","sequence_number":4,"response":{"status":"completed","output":[{"type":"custom_tool_call","name":"apply_patch"}]}}));
        let (out, blocks) = run_chunked(s.as_bytes(), 8);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tool_name, "apply_patch");
        let (_, completed) = parse_responses(&out);
        assert!(completed["output"].as_array().unwrap().iter().all(|i| i["type"] == "message"));
    }

    // ---- Chat Completions ----

    fn chat(text: &str, tools: &[(&str, &[&str])]) -> Vec<u8> {
        let mut s = String::new();
        let base = json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"kimi"});
        let mut e = |choices: Value| {
            let mut v = base.clone();
            v["choices"] = choices;
            s.push_str(&ev(None, &v));
        };
        e(json!([{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":null}]));
        for (i, (name, frags)) in tools.iter().enumerate() {
            e(json!([{"index":0,"delta":{"tool_calls":[{"index":i,"id":format!("call_{i}"),"type":"function","function":{"name":name,"arguments":""}}]},"finish_reason":null}]));
            for f in *frags {
                e(json!([{"index":0,"delta":{"tool_calls":[{"index":i,"function":{"arguments":f}}]},"finish_reason":null}]));
            }
        }
        let finish = if tools.is_empty() { "stop" } else { "tool_calls" };
        e(json!([{"index":0,"delta":{},"finish_reason":finish}]));
        let mut usage = base.clone();
        usage["choices"] = json!([]);
        usage["usage"] = json!({"prompt_tokens":10,"completion_tokens":42,"total_tokens":52});
        s.push_str(&ev(None, &usage));
        s.push_str("data: [DONE]\n\n");
        s.into_bytes()
    }

    /// The openai SDK's chunk accumulator: content concatenated, tool calls
    /// merged by index, one finish_reason, `[DONE]` last.
    fn parse_chat(body: &[u8]) -> (String, Vec<(String, String)>, String, Value) {
        let mut content = String::new();
        let mut calls: Vec<(String, String)> = Vec::new();
        let mut finish = String::new();
        let mut usage = Value::Null;
        let mut done = false;
        for (name, data) in events(body) {
            assert!(name.is_none());
            assert!(!done, "chunk after [DONE]");
            if data == "[DONE]" {
                done = true;
                continue;
            }
            let v: Value = serde_json::from_str(&data).expect("chunk is JSON");
            assert_eq!(v["id"], "chatcmpl-1");
            if !v["usage"].is_null() {
                usage = v["usage"].clone();
            }
            for c in v["choices"].as_array().unwrap() {
                if let Some(t) = c["delta"]["content"].as_str() {
                    content.push_str(t);
                }
                for tc in c["delta"]["tool_calls"].as_array().into_iter().flatten() {
                    let i = tc["index"].as_u64().unwrap() as usize;
                    if calls.len() <= i {
                        calls.resize(i + 1, (String::new(), String::new()));
                    }
                    if let Some(n) = tc["function"]["name"].as_str() {
                        calls[i].0.push_str(n);
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        calls[i].1.push_str(a);
                    }
                }
                if let Some(f) = c["finish_reason"].as_str() {
                    assert!(finish.is_empty(), "one finish_reason");
                    finish = f.to_owned();
                }
            }
        }
        assert!(done, "stream ends with [DONE]");
        (content, calls, finish, usage)
    }

    #[test]
    fn chat_clean_tool_call_passes_byte_for_byte() {
        let body = chat("AcmeCorp is fine in text", &[("bash", &[r#"{"command":"#, r#""ls"}"#])]);
        for size in [body.len(), 10, 1] {
            let (out, blocks) = run_chunked(&body, size);
            assert_eq!(out, body, "chunk size {size}");
            assert!(blocks.is_empty());
        }
    }

    #[test]
    fn chat_match_split_across_fragments_is_rewritten() {
        let body = chat("ok", &[("bash", &[r#"{"command":"echo inter"#, r#"nal-2024"}"#])]);
        let (out, blocks) = run_chunked(&body, 7);
        let (content, calls, finish, usage) = parse_chat(&out);
        assert!(calls.is_empty(), "no tool call survives");
        assert_eq!(finish, "stop");
        assert_eq!(usage["completion_tokens"], 42);
        assert!(content.contains("⛔ cctui blocked a bash call"), "{content}");
        assert!(blocks[0].rule.starts_with("pattern:"));
    }

    #[test]
    fn chat_match_in_a_later_parallel_call_drops_them_all() {
        let body = chat(
            "",
            &[("read", &[r#"{"path":"/tmp/a"}"#]), ("bash", &[r#"{"command":"cat <<EOF\nacme/x#1\nEOF"}"#])],
        );
        let (out, blocks) = run_chunked(&body, 5);
        let (_, calls, finish, _) = parse_chat(&out);
        assert!(calls.is_empty());
        assert_eq!(finish, "stop");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tool_name, "bash");
    }

    // ---- non-streaming JSON ----

    #[test]
    fn anthropic_json_is_rewritten_only_on_a_tool_match() {
        let p = policy();
        let clean = json!({"id":"m","type":"message","role":"assistant","content":[{"type":"text","text":"AcmeCorp"},{"type":"tool_use","id":"t","name":"Bash","input":{"command":"ls"}}],"stop_reason":"tool_use","usage":{"output_tokens":3}});
        assert!(rewrite_json(&p, clean.to_string().as_bytes()).is_none());

        let hit = json!({"id":"m","type":"message","role":"assistant","content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a"}},{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"gh pr create --body-file <(echo acme/secret#3)"}}],"stop_reason":"tool_use","usage":{"output_tokens":3}});
        let (out, block) = rewrite_json(&p, hit.to_string().as_bytes()).expect("blocked");
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["usage"]["output_tokens"], 3);
        let content = v["content"].as_array().unwrap();
        assert!(content.iter().all(|b| b["type"] == "text"));
        assert!(content[1]["text"].as_str().unwrap().starts_with("⛔ cctui blocked a Bash call"));
        assert_eq!(block.tool_name, "Bash");
    }

    #[test]
    fn responses_json_is_rewritten_only_on_a_tool_match() {
        let p = policy();
        let clean = json!({"id":"r","status":"completed","output":[{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}]});
        assert!(rewrite_json(&p, clean.to_string().as_bytes()).is_none());
        let hit = json!({"id":"r","status":"completed","output":[{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"},{"type":"function_call","name":"shell","arguments":"{\"command\":[\"echo\",\"ACMECORP\"]}"}]});
        let (out, _) = rewrite_json(&p, hit.to_string().as_bytes()).expect("blocked");
        let v: Value = serde_json::from_slice(&out).unwrap();
        let output = v["output"].as_array().unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], "message");
    }

    #[test]
    fn chat_json_is_rewritten_only_on_a_tool_match() {
        let p = policy();
        let clean = json!({"id":"c","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"AcmeCorp","tool_calls":[{"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]});
        assert!(rewrite_json(&p, clean.to_string().as_bytes()).is_none());
        let hit = json!({"id":"c","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}},{"id":"y","type":"function","function":{"name":"bash","arguments":"{\"command\":\"open https://github.com/acme/secret/issues/9\"}"}}]},"finish_reason":"tool_calls"}]});
        let (out, _) = rewrite_json(&p, hit.to_string().as_bytes()).expect("blocked");
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v["choices"][0]["message"].get("tool_calls").is_none());
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert!(v["choices"][0]["message"]["content"].as_str().unwrap().starts_with("⛔"));
    }

    // ---- rules ----

    #[test]
    fn cross_repo_rule_catches_protected_references_only() {
        let p = policy();
        for hit in [
            "see acme/secret#123",
            "Fixes Acme/secret-repo.v2#9.",
            "https://github.com/acme/secret/pull/5",
            "http://www.github.com/ACME/x/issues/1",
            "github.com/acme/x/commit/abcdef",
            "github.com/acme/x/blob/main/README.md",
            "gh pr create --body \"$(cat <<'EOF'\nRelated: acme/secret#9\nEOF\n)\"",
        ] {
            assert_eq!(p.scan_str(hit).map(|h| h.rule), Some("cross_repo"), "{hit}");
        }
        for clean in [
            "closes #123",
            "DorskFR/cctui#12",
            "https://github.com/DorskFR/cctui/pull/3",
            "github.com/acme",
            "path/to/acme/file.rs",
        ] {
            assert_eq!(p.scan_str(clean), None, "{clean}");
        }
    }

    #[test]
    fn nested_and_encoded_inputs_are_scanned() {
        let p = policy();
        let write = json!({"file_path":"/tmp/pr-body.md","content":"## Summary\nRefs https://github.com/acme/secret/pull/1\n"});
        assert!(p.scan_value(&write).is_some(), "Write content feeding --body-file");
        let nested = json!({"edits":[{"new":"x"},{"new":{"deep":"ACMECORP"}}]});
        assert_eq!(p.scan_value(&nested).unwrap().rule, "denylist");
        let encoded = json!({"arguments":"{\"body\":\"acme\\u002fsecret#4\"}"});
        assert!(p.scan_value(&encoded).is_some(), "JSON-string arguments are decoded");
    }

    #[test]
    fn exempt_roots_match_whole_path_components() {
        let p = policy();
        assert!(p.exempts(Some("/home/u/work")));
        assert!(p.exempts(Some("/home/u/work/")));
        assert!(p.exempts(Some("/home/u/work/repo/sub")));
        assert!(!p.exempts(Some("/home/u/workshop")));
        assert!(!p.exempts(Some("/home/u/other")));
        assert!(!p.exempts(None), "unknown cwd is scanned");
    }

    #[test]
    fn an_empty_policy_installs_no_guard() {
        assert!(CompiledPolicy::compile(&ToolPolicy::default()).is_none());
        let roots_only = ToolPolicy { exempt_roots: vec!["/w".into()], ..ToolPolicy::default() };
        assert!(CompiledPolicy::compile(&roots_only).is_none());
        assert!(guardable(Some("text/event-stream"), Some("gzip")).is_none());
        assert_eq!(guardable(Some("text/event-stream; charset=utf-8"), None), Some(true));
        assert_eq!(guardable(Some("application/json"), Some("identity")), Some(false));
    }

    #[test]
    fn policy_normalization_rejects_bad_input() {
        let p = ToolPolicy {
            terms: vec![" a ".into(), "a".into(), String::new()],
            exempt_roots: vec!["/w/".into()],
            ..ToolPolicy::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(p.terms, vec!["a"]);
        assert_eq!(p.exempt_roots, vec!["/w"]);
        assert!(ToolPolicy { patterns: vec!["(".into()], ..ToolPolicy::default() }.normalized().is_err());
        assert!(ToolPolicy { exempt_roots: vec!["rel".into()], ..ToolPolicy::default() }.normalized().is_err());
    }

    #[test]
    fn mask_hides_the_middle() {
        assert_eq!(mask("acmecorp"), "a******p");
        assert_eq!(mask("ab"), "**");
        assert_eq!(mask("acme/secret-repository#1234"), "a********4");
    }
}
