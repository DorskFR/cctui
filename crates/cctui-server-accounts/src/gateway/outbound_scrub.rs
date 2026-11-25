//! Only masked string literals are spliced into the client's bytes, never a
//! re-serialization, so the provider's prompt cache sees stable bytes.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use cctui_crypto::redact::{self, CompiledPatterns};
use serde_json::Value;

use super::config::Family;

const MAX_DEPTH: usize = 128;
const SETTINGS_TTL: Duration = Duration::from_secs(30);
const CACHE_SWEEP_AT: usize = 4096;

type UserPatterns = Vec<(String, String)>;
type SettingsCache = dashmap::DashMap<String, CachedSettings>;
type CompiledCache = dashmap::DashMap<UserPatterns, Arc<CompiledPatterns>>;

#[derive(Clone)]
struct CachedSettings {
    at: Instant,
    clock: u64,
    user: Option<uuid::Uuid>,
    patterns: Option<UserPatterns>,
}

impl CachedSettings {
    fn fresh(&self, now: Instant) -> bool {
        use cctui_server_kernel::execution_settings::scrub_settings_written_since;
        now.duration_since(self.at) < SETTINGS_TTL
            && !self.user.is_some_and(|u| scrub_settings_written_since(u, self.clock))
    }
}

static SETTINGS_CACHE: LazyLock<SettingsCache> = LazyLock::new(SettingsCache::new);
static COMPILED_CACHE: LazyLock<CompiledCache> = LazyLock::new(CompiledCache::new);

/// # Errors
///
/// A failed settings read; the caller refuses the request rather than forward
/// an opted-in user's secrets unscrubbed.
pub async fn outbound_patterns(
    pool: &sqlx::PgPool,
    session_token: &str,
) -> Result<Option<Arc<CompiledPatterns>>, sqlx::Error> {
    let hash = cctui_server_kernel::auth::sha256_hex(session_token);
    let now = Instant::now();
    let cached =
        SETTINGS_CACHE.get(&hash).filter(|hit| hit.fresh(now)).map(|hit| hit.patterns.clone());
    let user = if let Some(user) = cached { user } else { refresh(pool, hash, now).await? };
    Ok(user.map(compiled))
}

async fn refresh(
    pool: &sqlx::PgPool,
    hash: String,
    now: Instant,
) -> Result<Option<UserPatterns>, sqlx::Error> {
    let clock = cctui_server_kernel::execution_settings::scrub_settings_clock();
    let (user, data) = load_user_settings(pool, &hash).await?;
    let patterns = data.as_ref().and_then(user_patterns_of);
    if SETTINGS_CACHE.len() >= CACHE_SWEEP_AT {
        SETTINGS_CACHE.retain(|_, hit| hit.fresh(now));
    }
    SETTINGS_CACHE
        .insert(hash, CachedSettings { at: now, clock, user, patterns: patterns.clone() });
    Ok(patterns)
}

async fn load_user_settings(
    pool: &sqlx::PgPool,
    token_hash: &str,
) -> Result<(Option<uuid::Uuid>, Option<Value>), sqlx::Error> {
    let row: Option<(Option<uuid::Uuid>, Option<Value>)> = sqlx::query_as(
        "SELECT COALESCE(s.user_id, m.user_id, t.user_id), us.data FROM session_tokens t \
           LEFT JOIN sessions s ON s.id = t.session_id \
           LEFT JOIN machines m ON m.id = s.machine_uuid \
           LEFT JOIN user_settings us ON us.user_id = COALESCE(s.user_id, m.user_id, t.user_id) \
          WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or_default())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    Encoded,
    BadEncoding,
    TooLarge,
    NotUtf8,
    NotJson,
    TooDeep,
}

impl Unreadable {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Encoded => "unsupported content encoding",
            Self::BadEncoding => "body does not decode",
            Self::TooLarge => "decoded body is too large",
            Self::NotUtf8 => "body is not UTF-8",
            Self::NotJson => "body is not JSON",
            Self::TooDeep => "body nests too deep",
        }
    }

    #[must_use]
    pub const fn status(self) -> axum::http::StatusCode {
        match self {
            Self::Encoded => axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::TooLarge => axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            Self::BadEncoding | Self::NotUtf8 | Self::NotJson | Self::TooDeep => {
                axum::http::StatusCode::UNPROCESSABLE_ENTITY
            }
        }
    }
}

/// The JSON model calls whose prompts the scrubber reads; every other path is
/// forwarded as is.
#[must_use]
pub fn model_call(family: Family, method: &axum::http::Method, tail: &str) -> bool {
    if *method != axum::http::Method::POST {
        return false;
    }
    let decoded = super::config::percent_decode(tail);
    let path = decoded.strip_suffix('/').unwrap_or(&decoded);
    match family {
        Family::Anthropic => matches!(path, "/v1/messages" | "/v1/messages/count_tokens"),
        Family::Openai => matches!(path, "/responses" | "/responses/compact" | "/chat/completions"),
        Family::Fireworks => path == "/chat/completions",
        Family::Google => ["/v1beta/models/", "/v1/models/"]
            .iter()
            .filter_map(|p| path.strip_prefix(*p))
            .filter_map(|rest| rest.rsplit_once(':'))
            .any(|(model, rpc)| {
                !model.is_empty()
                    && !model.contains('/')
                    && matches!(rpc, "generateContent" | "streamGenerateContent")
            }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Identity,
    Gzip,
    Zstd,
}

/// # Errors
///
/// An encoding the scrubber cannot undo, including a stack of several.
pub fn request_encoding(headers: &axum::http::HeaderMap) -> Result<Encoding, Unreadable> {
    let mut found = Encoding::Identity;
    for value in headers.get_all(axum::http::header::CONTENT_ENCODING) {
        let value = value.to_str().map_err(|_| Unreadable::Encoded)?;
        for coding in value
            .split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("identity"))
        {
            let next =
                if coding.eq_ignore_ascii_case("gzip") || coding.eq_ignore_ascii_case("x-gzip") {
                    Encoding::Gzip
                } else if coding.eq_ignore_ascii_case("zstd") {
                    Encoding::Zstd
                } else {
                    return Err(Unreadable::Encoded);
                };
            if found != Encoding::Identity {
                return Err(Unreadable::Encoded);
            }
            found = next;
        }
    }
    Ok(found)
}

/// # Errors
///
/// A body that does not decode, or decodes past `limit` bytes.
pub fn decode_body(body: Bytes, encoding: Encoding, limit: usize) -> Result<Bytes, Unreadable> {
    use std::io::Read;
    let reader: Box<dyn Read + '_> = match encoding {
        Encoding::Identity => return Ok(body),
        Encoding::Gzip => Box::new(flate2::read::MultiGzDecoder::new(&body[..])),
        Encoding::Zstd => Box::new(
            zstd::stream::read::Decoder::new(&body[..]).map_err(|_| Unreadable::BadEncoding)?,
        ),
    };
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut out = Vec::new();
    reader.take(cap).read_to_end(&mut out).map_err(|_| Unreadable::BadEncoding)?;
    if out.len() > limit {
        return Err(Unreadable::TooLarge);
    }
    Ok(Bytes::from(out))
}

/// # Errors
///
/// A body the scrubber cannot read while patterns are active.
pub fn scrub_request_body(
    body: Bytes,
    patterns: Option<&CompiledPatterns>,
) -> Result<Bytes, Unreadable> {
    let Some(patterns) = patterns.filter(|p| !p.is_empty()) else { return Ok(body) };
    let src = std::str::from_utf8(&body).map_err(|_| Unreadable::NotUtf8)?;
    if src.trim().is_empty() {
        return Ok(body);
    }
    let root = Parser::parse(src.as_bytes())?;
    let mut targets = Vec::new();
    collect_targets(&root, src, &mut targets);
    let mut edits: Vec<(Range<usize>, String)> = targets
        .into_iter()
        .filter_map(|r| scrub_literal(src, &r, patterns).map(|lit| (r, lit)))
        .collect();
    if edits.is_empty() {
        return Ok(body);
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
    Ok(Bytes::from(out))
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
    too_deep: bool,
}

impl<'a> Parser<'a> {
    fn parse(b: &'a [u8]) -> Result<Node, Unreadable> {
        let mut p = Self { b, i: 0, too_deep: false };
        let root = p.value(0);
        p.ws();
        match root {
            Some(root) if p.i == b.len() => Ok(root),
            _ if p.too_deep => Err(Unreadable::TooDeep),
            _ => Err(Unreadable::NotJson),
        }
    }

    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH {
            self.too_deep = true;
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
    if let Some(Node::Arr(contents)) = get(top, src, "contents") {
        gemini_targets(contents, src, out);
    }
}

fn gemini_targets(contents: &[Node], src: &str, out: &mut Vec<Range<usize>>) {
    for content in contents {
        let Node::Obj(c) = content else { continue };
        if str_at(c, src, "role").as_deref() == Some("model") {
            continue;
        }
        let Some(Node::Arr(parts)) = get(c, src, "parts") else { continue };
        for part in parts {
            let Node::Obj(p) = part else { continue };
            if let Some(Node::Str(r)) = get(p, src, "text") {
                out.push(r.clone());
            }
            if let Some(Node::Obj(response)) = get(p, src, "functionResponse")
                && let Some(value) = get(response, src, "response")
            {
                string_values(value, out);
            }
        }
    }
}

fn string_values(node: &Node, out: &mut Vec<Range<usize>>) {
    match node {
        Node::Str(r) => out.push(r.clone()),
        Node::Arr(items) => items.iter().for_each(|n| string_values(n, out)),
        Node::Obj(fields) => fields.iter().for_each(|(_, n)| string_values(n, out)),
        Node::Scalar => {}
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

    fn scrub(body: Bytes, patterns: Option<&CompiledPatterns>) -> Bytes {
        scrub_request_body(body, patterns).unwrap()
    }

    fn anthropic_with_tool_result(output: &str) -> String {
        format!(
            r#"{{"model":"claude-x","stream":true,"messages":[{{"role":"user","content":"hi"}},{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"cat .env"}}}}]}},{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"{output}"}}]}}],"system":"z"}}"#
        )
    }

    #[test]
    fn masks_a_token_in_a_tool_result() {
        let body = Bytes::from(anthropic_with_tool_result(&format!("GH={TOKEN}\\nok")));
        let out = scrub(body.clone(), Some(&patterns()));
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
        let out = scrub(body.clone(), Some(&patterns()));
        assert_eq!(out, body);
        assert_eq!(out.as_ptr(), body.as_ptr());
    }

    #[test]
    fn masking_is_deterministic_across_calls() {
        let body = Bytes::from(anthropic_with_tool_result(TOKEN));
        let a = scrub(body.clone(), Some(&patterns()));
        let b = scrub(body, Some(&patterns()));
        assert_ne!(a, Bytes::from(anthropic_with_tool_result(TOKEN)));
        assert_eq!(a, b);
    }

    #[test]
    fn disabled_patterns_are_a_no_op() {
        let body = Bytes::from(anthropic_with_tool_result(TOKEN));
        let out = scrub(body.clone(), Some(&CompiledPatterns::disabled()));
        assert_eq!(out, body);
        let out = scrub(body.clone(), None);
        assert_eq!(out, body);
    }

    #[test]
    fn assistant_turns_are_left_alone() {
        let body = Bytes::from(format!(
            r#"{{"messages":[{{"role":"assistant","content":[{{"type":"text","text":"{TOKEN}"}}]}}]}}"#
        ));
        assert_eq!(scrub(body.clone(), Some(&patterns())), body);
    }

    #[test]
    fn masks_user_text_and_tool_messages_in_chat_completions() {
        let body = Bytes::from(format!(
            r#"{{"messages":[{{"role":"user","content":[{{"type":"text","text":"use {TOKEN}"}}]}},{{"role":"tool","tool_call_id":"c","content":"{TOKEN}"}}]}}"#
        ));
        let out = scrub(body, Some(&patterns()));
        let text = std::str::from_utf8(&out).unwrap();
        assert!(!text.contains(TOKEN), "{text}");
        assert_eq!(text.matches("[REDACTED:github_token").count(), 2, "{text}");
    }

    #[test]
    fn masks_responses_api_function_output_and_user_input() {
        let body = Bytes::from(format!(
            r#"{{"model":"gpt-x","input":[{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{TOKEN}"}}]}},{{"type":"function_call","call_id":"c","arguments":"{{}}"}},{{"type":"function_call_output","call_id":"c","output":"{TOKEN}"}}]}}"#
        ));
        let out = scrub(body, Some(&patterns()));
        let text = std::str::from_utf8(&out).unwrap();
        assert!(!text.contains(TOKEN), "{text}");
        assert_eq!(text.matches("[REDACTED:github_token").count(), 2, "{text}");
    }

    #[test]
    fn unreadable_bodies_are_refused_when_opted_in() {
        let refused = |body: Bytes| scrub_request_body(body, Some(&patterns())).unwrap_err();
        assert_eq!(refused(Bytes::from(format!("not json {TOKEN}"))), Unreadable::NotJson);
        assert_eq!(refused(Bytes::from(r#"{"messages":[}"#)), Unreadable::NotJson);
        assert_eq!(refused(Bytes::from_static(b"{\"input\":\"\xff\xfe\"}")), Unreadable::NotUtf8);
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert_eq!(refused(Bytes::from(deep)), Unreadable::TooDeep);
        assert_eq!(Unreadable::Encoded.status(), axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(Unreadable::TooLarge.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(Unreadable::NotJson.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(Unreadable::BadEncoding.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn unreadable_bodies_pass_through_when_not_opted_in() {
        let body = Bytes::from(format!("not json {TOKEN}"));
        assert_eq!(scrub(body.clone(), None), body);
        assert_eq!(scrub(body.clone(), Some(&CompiledPatterns::disabled())), body);
    }

    #[test]
    fn an_empty_body_is_forwarded() {
        let body = Bytes::from_static(b" \n");
        assert_eq!(scrub(body.clone(), Some(&patterns())), body);
    }

    fn with_encoding(v: &'static str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert(axum::http::header::CONTENT_ENCODING, axum::http::HeaderValue::from_static(v));
        h
    }

    #[test]
    fn only_identity_gzip_and_zstd_are_readable_encodings() {
        let of = |v| request_encoding(&with_encoding(v));
        assert_eq!(request_encoding(&axum::http::HeaderMap::new()), Ok(Encoding::Identity));
        assert_eq!(of("identity"), Ok(Encoding::Identity));
        assert_eq!(of("Identity"), Ok(Encoding::Identity));
        assert_eq!(of("gzip"), Ok(Encoding::Gzip));
        assert_eq!(of("x-gzip"), Ok(Encoding::Gzip));
        assert_eq!(of("ZSTD"), Ok(Encoding::Zstd));
        assert_eq!(of("identity, zstd"), Ok(Encoding::Zstd));
        assert_eq!(of("br"), Err(Unreadable::Encoded));
        assert_eq!(of("deflate"), Err(Unreadable::Encoded));
        assert_eq!(of("identity, br"), Err(Unreadable::Encoded));
        assert_eq!(of("gzip, zstd"), Err(Unreadable::Encoded));
        let mut stacked = with_encoding("gzip");
        stacked.append(axum::http::header::CONTENT_ENCODING, "gzip".parse().unwrap());
        assert_eq!(request_encoding(&stacked), Err(Unreadable::Encoded));
    }

    fn gzip(raw: &[u8]) -> Bytes {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(raw).unwrap();
        Bytes::from(enc.finish().unwrap())
    }

    #[test]
    fn gzip_and_zstd_bodies_decode_then_scrub() {
        let raw = anthropic_with_tool_result(TOKEN);
        let zstd_body = Bytes::from(zstd::encode_all(raw.as_bytes(), 3).unwrap());
        for (body, encoding) in
            [(gzip(raw.as_bytes()), Encoding::Gzip), (zstd_body, Encoding::Zstd)]
        {
            let decoded = decode_body(body, encoding, 1 << 20).unwrap();
            assert_eq!(decoded, raw.as_bytes());
            let out = scrub(decoded, Some(&patterns()));
            assert!(!std::str::from_utf8(&out).unwrap().contains(TOKEN));
        }
        let plain = Bytes::from(raw);
        let same = decode_body(plain.clone(), Encoding::Identity, 1).unwrap();
        assert_eq!(same.as_ptr(), plain.as_ptr());
    }

    #[test]
    fn decoding_is_capped_and_garbage_is_refused() {
        let bomb = vec![b' '; 4096];
        assert_eq!(decode_body(gzip(&bomb), Encoding::Gzip, 4096).unwrap().len(), 4096);
        assert_eq!(decode_body(gzip(&bomb), Encoding::Gzip, 4095), Err(Unreadable::TooLarge));
        let zstd_bomb = Bytes::from(zstd::encode_all(&bomb[..], 19).unwrap());
        assert_eq!(decode_body(zstd_bomb, Encoding::Zstd, 100), Err(Unreadable::TooLarge));
        let junk = Bytes::from_static(b"{\"messages\":[]}");
        assert_eq!(decode_body(junk.clone(), Encoding::Gzip, 4096), Err(Unreadable::BadEncoding));
        assert_eq!(decode_body(junk, Encoding::Zstd, 4096), Err(Unreadable::BadEncoding));
    }

    #[test]
    fn only_json_model_calls_are_scrubbed() {
        let post = axum::http::Method::POST;
        let yes = |family, tail| model_call(family, &post, tail);
        assert!(yes(Family::Anthropic, "/v1/messages"));
        assert!(yes(Family::Anthropic, "/v1/messages/"));
        assert!(yes(Family::Anthropic, "/v1/%6Dessages"));
        assert!(yes(Family::Anthropic, "/v1/messages/count_tokens"));
        assert!(!yes(Family::Anthropic, "/v1/complete"));
        assert!(!yes(Family::Anthropic, "/v1/files"));
        assert!(yes(Family::Openai, "/responses"));
        assert!(yes(Family::Openai, "/responses/compact"));
        assert!(yes(Family::Openai, "/chat/completions"));
        assert!(!yes(Family::Openai, "/responses/resp_1/cancel"));
        assert!(yes(Family::Fireworks, "/chat/completions"));
        assert!(!yes(Family::Fireworks, "/embeddings"));
        assert!(yes(Family::Google, "/v1beta/models/gemini-2.5-pro:generateContent"));
        assert!(yes(Family::Google, "/v1/models/gemini-2.5-pro:streamGenerateContent"));
        assert!(yes(Family::Google, "/v1beta/models/gemini-2.5-pro%3AgenerateContent"));
        assert!(!yes(Family::Google, "/v1beta/models/gemini-2.5-pro:countTokens"));
        assert!(!yes(Family::Google, "/v1beta/models/:generateContent"));
        assert!(!yes(Family::Google, "/v1beta/models/a/b:generateContent"));
        assert!(!model_call(Family::Anthropic, &axum::http::Method::GET, "/v1/messages"));
    }

    fn gemini_body(secret: &str) -> String {
        format!(
            r#"{{"systemInstruction":{{"parts":[{{"text":"sys {secret}"}}]}},"contents":[{{"role":"user","parts":[{{"text":"use {secret}"}}]}},{{"role":"model","parts":[{{"text":"model {secret}","thoughtSignature":"c2lnbmF0dXJlLWJsb2ItZm9yLXRlc3Rz"}},{{"functionCall":{{"name":"run","args":{{"cmd":"cat .env {secret}"}}}}}}]}},{{"role":"user","parts":[{{"functionResponse":{{"name":"run","response":{{"output":"GH={secret}","nested":[{{"line":"{secret}"}}],"code":0}}}}}}]}}],"generationConfig":{{"temperature":0}}}}"#
        )
    }

    #[test]
    fn masks_gemini_generate_content_user_text_and_function_responses() {
        let body = Bytes::from(gemini_body(TOKEN));
        let out = scrub(body.clone(), Some(&patterns()));
        let text = std::str::from_utf8(&out).unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        let original: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(text.matches("[REDACTED:github_token").count(), 3, "{text}");
        assert!(!parsed["contents"][0]["parts"][0]["text"].as_str().unwrap().contains(TOKEN));
        let response = &parsed["contents"][2]["parts"][0]["functionResponse"]["response"];
        assert!(!response.to_string().contains(TOKEN), "{response}");
        assert_eq!(response["code"], 0);
        assert_eq!(parsed["contents"][1], original["contents"][1]);
        assert_eq!(parsed["systemInstruction"], original["systemInstruction"]);
        assert_eq!(parsed["generationConfig"], original["generationConfig"]);
    }

    #[test]
    fn masks_gemini_stream_generate_content_without_a_role() {
        let body = Bytes::from(format!(
            r#"{{"contents":[{{"parts":[{{"text":"{TOKEN}"}}]}},{{"role":"model","parts":[{{"text":"ok","thoughtSignature":"{TOKEN}"}}]}},{{"role":"user","parts":[{{"text":"again {TOKEN}"}}]}}],"tools":[]}}"#
        ));
        let out = scrub(body, Some(&patterns()));
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert!(!parsed["contents"][0]["parts"][0]["text"].as_str().unwrap().contains(TOKEN));
        assert!(!parsed["contents"][2]["parts"][0]["text"].as_str().unwrap().contains(TOKEN));
        assert_eq!(parsed["contents"][1]["parts"][0]["thoughtSignature"], TOKEN);
    }

    #[test]
    fn a_clean_gemini_body_is_byte_identical() {
        let body = Bytes::from(gemini_body("nothing"));
        let out = scrub(body.clone(), Some(&patterns()));
        assert_eq!(out.as_ptr(), body.as_ptr());
    }

    #[test]
    fn outbound_opt_in_needs_the_base_switch() {
        let off = serde_json::json!({ "secretScrubEnabled": false, "secretScrubOutbound": true });
        assert!(user_patterns_of(&off).is_none());
    }

    #[test]
    fn a_settings_write_expires_that_users_cached_entry_only() {
        use cctui_server_kernel::execution_settings::{
            note_scrub_settings_written, scrub_settings_clock,
        };
        let now = Instant::now();
        let (writer, other) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let entry = |user| CachedSettings {
            at: now,
            clock: scrub_settings_clock(),
            user: Some(user),
            patterns: None,
        };
        let (mine, theirs) = (entry(writer), entry(other));
        assert!(mine.fresh(now) && theirs.fresh(now));
        note_scrub_settings_written(writer);
        assert!(!mine.fresh(now));
        assert!(theirs.fresh(now));
        assert!(entry(writer).fresh(now));
        assert!(!theirs.fresh(now + SETTINGS_TTL));
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
