//! Only masked string literals are spliced into the client's bytes, never a
//! re-serialization, so the provider's prompt cache sees stable bytes.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use cctui_crypto::redact::{self, CompiledPatterns};
use serde_json::Value;

const MAX_DEPTH: usize = 128;
const SETTINGS_TTL: Duration = Duration::from_secs(30);
const CACHE_SWEEP_AT: usize = 4096;

type UserPatterns = Vec<(String, String)>;
type SettingsCache = dashmap::DashMap<String, (Instant, Option<UserPatterns>)>;
type CompiledCache = dashmap::DashMap<UserPatterns, Arc<CompiledPatterns>>;

static SETTINGS_CACHE: LazyLock<SettingsCache> = LazyLock::new(SettingsCache::new);
static COMPILED_CACHE: LazyLock<CompiledCache> = LazyLock::new(CompiledCache::new);

pub(crate) async fn outbound_patterns(
    pool: &sqlx::PgPool,
    session_token: &str,
) -> Result<Option<Arc<CompiledPatterns>>, sqlx::Error> {
    let hash = cctui_server_kernel::auth::sha256_hex(session_token);
    let now = Instant::now();
    let cached = SETTINGS_CACHE
        .get(&hash)
        .filter(|hit| now.duration_since(hit.0) < SETTINGS_TTL)
        .map(|hit| hit.1.clone());
    let user = match cached {
        Some(user) => user,
        None => {
            let user = load_user_patterns(pool, &hash).await?;
            if SETTINGS_CACHE.len() >= CACHE_SWEEP_AT {
                SETTINGS_CACHE.retain(|_, (at, _)| now.duration_since(*at) < SETTINGS_TTL);
            }
            SETTINGS_CACHE.insert(hash, (now, user.clone()));
            user
        }
    };
    Ok(user.map(compiled))
}

async fn load_user_patterns(
    pool: &sqlx::PgPool,
    token_hash: &str,
) -> Result<Option<UserPatterns>, sqlx::Error> {
    let data: Option<Value> = sqlx::query_scalar(
        "SELECT us.data FROM session_tokens t \
           LEFT JOIN sessions s ON s.id = t.session_id \
           LEFT JOIN machines m ON m.id = s.machine_uuid \
           JOIN user_settings us ON us.user_id = COALESCE(s.user_id, m.user_id, t.user_id) \
          WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(data.as_ref().and_then(user_patterns_of))
}

fn user_patterns_of(data: &Value) -> Option<UserPatterns> {
    use cctui_server_kernel::execution_settings::{secret_scrub_of, secret_scrub_outbound_of};
    secret_scrub_outbound_of(data)
        .then(|| secret_scrub_of(data).patterns.into_iter().map(|p| (p.name, p.regex)).collect())
}

fn compiled(user: UserPatterns) -> Arc<CompiledPatterns> {
    if let Some(hit) = COMPILED_CACHE.get(&user) {
        return Arc::clone(&hit);
    }
    let patterns = Arc::new(redact::compile(true, &user, &cctui_crypto::vault_key()));
    if COMPILED_CACHE.len() >= CACHE_SWEEP_AT {
        COMPILED_CACHE.clear();
    }
    COMPILED_CACHE.insert(user, Arc::clone(&patterns));
    patterns
}

#[must_use]
pub(crate) fn scrub_request_body(body: Bytes, patterns: Option<&CompiledPatterns>) -> Bytes {
    let Some(patterns) = patterns.filter(|p| !p.is_empty()) else { return body };
    let Ok(src) = std::str::from_utf8(&body) else { return body };
    let Some(root) = Parser::parse(src.as_bytes()) else { return body };
    let mut targets = Vec::new();
    collect_targets(&root, src, &mut targets);
    let mut edits: Vec<(Range<usize>, String)> = targets
        .into_iter()
        .filter_map(|r| scrub_literal(src, &r, patterns).map(|lit| (r, lit)))
        .collect();
    if edits.is_empty() {
        return body;
    }
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for (r, lit) in edits {
        out.push_str(&src[last..r.start]);
        out.push_str(&lit);
        last = r.end;
    }
    out.push_str(&src[last..]);
    Bytes::from(out)
}

fn scrub_literal(src: &str, lit: &Range<usize>, patterns: &CompiledPatterns) -> Option<String> {
    let mut value = Value::String(decode(src, lit)?.into_owned());
    if redact::redact_json(&mut value, patterns) == 0 {
        return None;
    }
    serde_json::to_string(&value).ok()
}

enum Node {
    Str(Range<usize>),
    Arr(Vec<Self>),
    Obj(Vec<(Range<usize>, Self)>),
    Scalar,
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn parse(b: &'a [u8]) -> Option<Node> {
        let mut p = Self { b, i: 0 };
        let root = p.value(0)?;
        p.ws();
        (p.i == b.len()).then_some(root)
    }

    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'"' => self.string().map(Node::Str),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            _ => self.scalar(),
        }
    }

    fn string(&mut self) -> Option<Range<usize>> {
        let start = self.i;
        self.i += 1;
        loop {
            match *self.b.get(self.i)? {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return Some(start..self.i);
                }
                _ => self.i += 1,
            }
        }
    }

    fn scalar(&mut self) -> Option<Node> {
        let start = self.i;
        while self.b.get(self.i).is_some_and(|c| {
            !c.is_ascii_whitespace() && !matches!(c, b',' | b']' | b'}' | b':' | b'"')
        }) {
            self.i += 1;
        }
        (self.i > start).then_some(Node::Scalar)
    }

    fn array(&mut self, depth: usize) -> Option<Node> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Some(Node::Arr(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.ws();
            match *self.b.get(self.i)? {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    return Some(Node::Arr(items));
                }
                _ => return None,
            }
        }
    }

    fn object(&mut self, depth: usize) -> Option<Node> {
        self.i += 1;
        let mut fields = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Some(Node::Obj(fields));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return None;
            }
            let key = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return None;
            }
            self.i += 1;
            fields.push((key, self.value(depth + 1)?));
            self.ws();
            match *self.b.get(self.i)? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some(Node::Obj(fields));
                }
                _ => return None,
            }
        }
    }
}

fn decode<'s>(src: &'s str, lit: &Range<usize>) -> Option<Cow<'s, str>> {
    let raw = src.get(lit.clone())?;
    let inner = raw.get(1..raw.len().checked_sub(1)?)?;
    if inner.contains('\\') {
        serde_json::from_str::<String>(raw).ok().map(Cow::Owned)
    } else {
        Some(Cow::Borrowed(inner))
    }
}

fn get<'n>(fields: &'n [(Range<usize>, Node)], src: &str, key: &str) -> Option<&'n Node> {
    fields.iter().rev().find(|(k, _)| decode(src, k).as_deref() == Some(key)).map(|(_, v)| v)
}

fn str_at<'s>(fields: &[(Range<usize>, Node)], src: &'s str, key: &str) -> Option<Cow<'s, str>> {
    match get(fields, src, key)? {
        Node::Str(r) => decode(src, r),
        _ => None,
    }
}

const TOOL_OUTPUT_ITEMS: [&str; 3] =
    ["function_call_output", "custom_tool_call_output", "local_shell_call_output"];

fn collect_targets(root: &Node, src: &str, out: &mut Vec<Range<usize>>) {
    let Node::Obj(top) = root else { return };
    if let Some(Node::Arr(messages)) = get(top, src, "messages") {
        for message in messages {
            let Node::Obj(m) = message else { continue };
            if matches!(str_at(m, src, "role").as_deref(), Some("user" | "tool")) {
                content_targets(get(m, src, "content"), src, out);
            }
        }
    }
    match get(top, src, "input") {
        Some(Node::Str(r)) => out.push(r.clone()),
        Some(Node::Arr(items)) => {
            for item in items {
                let Node::Obj(it) = item else { continue };
                if str_at(it, src, "role").as_deref() == Some("user") {
                    content_targets(get(it, src, "content"), src, out);
                } else if str_at(it, src, "type").is_some_and(|t| TOOL_OUTPUT_ITEMS.contains(&&*t))
                {
                    content_targets(get(it, src, "output"), src, out);
                }
            }
        }
        _ => {}
    }
}

fn content_targets(content: Option<&Node>, src: &str, out: &mut Vec<Range<usize>>) {
    match content {
        Some(Node::Str(r)) => out.push(r.clone()),
        Some(Node::Arr(blocks)) => {
            for block in blocks {
                let Node::Obj(b) = block else { continue };
                match str_at(b, src, "type").as_deref() {
                    Some("text" | "input_text") => {
                        if let Some(Node::Str(r)) = get(b, src, "text") {
                            out.push(r.clone());
                        }
                    }
                    Some("tool_result") => content_targets(get(b, src, "content"), src, out),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    fn patterns() -> CompiledPatterns {
        redact::compile(true, &[], b"test-key")
    }

    fn anthropic_with_tool_result(output: &str) -> String {
        format!(
            r#"{{"model":"claude-x","stream":true,"messages":[{{"role":"user","content":"hi"}},{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"cat .env"}}}}]}},{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"{output}"}}]}}],"system":"z"}}"#
        )
    }

    #[test]
    fn masks_a_token_in_a_tool_result() {
        let body = Bytes::from(anthropic_with_tool_result(&format!("GH={TOKEN}\\nok")));
        let out = scrub_request_body(body.clone(), Some(&patterns()));
        let text = std::str::from_utf8(&out).unwrap();
        assert!(!text.contains(TOKEN), "{text}");
        assert!(text.contains("[REDACTED:github_token"), "{text}");
        let parsed: Value = serde_json::from_str(text).unwrap();
        let original: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["messages"][1], original["messages"][1]);
        assert!(text.starts_with(r#"{"model":"claude-x","stream":true,"messages":"#), "{text}");
        assert!(text.ends_with(r#""system":"z"}"#), "{text}");
    }

    #[test]
    fn leaves_a_clean_body_byte_identical() {
        let body = Bytes::from(anthropic_with_tool_result("nothing secret here"));
        let out = scrub_request_body(body.clone(), Some(&patterns()));
        assert_eq!(out, body);
        assert_eq!(out.as_ptr(), body.as_ptr());
    }

    #[test]
    fn masking_is_deterministic_across_calls() {
        let body = Bytes::from(anthropic_with_tool_result(TOKEN));
        let a = scrub_request_body(body.clone(), Some(&patterns()));
        let b = scrub_request_body(body, Some(&patterns()));
        assert_ne!(a, Bytes::from(anthropic_with_tool_result(TOKEN)));
        assert_eq!(a, b);
    }

    #[test]
    fn disabled_patterns_are_a_no_op() {
        let body = Bytes::from(anthropic_with_tool_result(TOKEN));
        let out = scrub_request_body(body.clone(), Some(&CompiledPatterns::disabled()));
        assert_eq!(out, body);
        let out = scrub_request_body(body.clone(), None);
        assert_eq!(out, body);
    }

    #[test]
    fn assistant_turns_are_left_alone() {
        let body = Bytes::from(format!(
            r#"{{"messages":[{{"role":"assistant","content":[{{"type":"text","text":"{TOKEN}"}}]}}]}}"#
        ));
        assert_eq!(scrub_request_body(body.clone(), Some(&patterns())), body);
    }

    #[test]
    fn masks_user_text_and_tool_messages_in_chat_completions() {
        let body = Bytes::from(format!(
            r#"{{"messages":[{{"role":"user","content":[{{"type":"text","text":"use {TOKEN}"}}]}},{{"role":"tool","tool_call_id":"c","content":"{TOKEN}"}}]}}"#
        ));
        let out = scrub_request_body(body, Some(&patterns()));
        let text = std::str::from_utf8(&out).unwrap();
        assert!(!text.contains(TOKEN), "{text}");
        assert_eq!(text.matches("[REDACTED:github_token").count(), 2, "{text}");
    }

    #[test]
    fn masks_responses_api_function_output_and_user_input() {
        let body = Bytes::from(format!(
            r#"{{"model":"gpt-x","input":[{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{TOKEN}"}}]}},{{"type":"function_call","call_id":"c","arguments":"{{}}"}},{{"type":"function_call_output","call_id":"c","output":"{TOKEN}"}}]}}"#
        ));
        let out = scrub_request_body(body, Some(&patterns()));
        let text = std::str::from_utf8(&out).unwrap();
        assert!(!text.contains(TOKEN), "{text}");
        assert_eq!(text.matches("[REDACTED:github_token").count(), 2, "{text}");
    }

    #[test]
    fn non_json_bodies_pass_through() {
        let body = Bytes::from(format!("not json {TOKEN}"));
        assert_eq!(scrub_request_body(body.clone(), Some(&patterns())), body);
    }

    #[test]
    fn outbound_opt_in_is_off_by_default() {
        assert!(user_patterns_of(&serde_json::json!({})).is_none());
        assert!(user_patterns_of(&serde_json::json!({ "secretScrubOutbound": false })).is_none());
        let on = user_patterns_of(&serde_json::json!({
            "secretScrubOutbound": true,
            "secretScrubPatterns": [{ "name": "acme", "regex": "ACME-[0-9]+", "enabled": true }],
        }));
        assert_eq!(on, Some(vec![("acme".to_owned(), "ACME-[0-9]+".to_owned())]));
    }
}
