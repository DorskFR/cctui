//! Rule of Two human gates, enforced on the worker's GitHub egress.
//!
//! Requests are reviewed only on intercepted (TLS-terminated) GitHub hosts, so
//! under an enforced gate a policed host missing from the inject config is
//! refused at connect time. A gate value that does not parse refuses every
//! outbound connection.

use std::borrow::Cow;

use serde_json::Value;

use crate::inject::{Framing, header_value};

pub const MAX_REVIEW_BYTES: usize = 1024 * 1024;

/// The guard cannot see a repository's real default branch, so `draftPrOnly`
/// refuses these names on top of the operator's list.
const DEFAULT_PROTECTED: [&str; 3] = ["main", "master", "trunk"];

const POLICED_HOSTS: [&str; 5] =
    ["github.com", "www.github.com", "gist.github.com", "api.github.com", "uploads.github.com"];

const REF_MUTATIONS: [&str; 12] = [
    "createRef",
    "updateRef",
    "updateRefs",
    "deleteRef",
    "createCommitOnBranch",
    "mergeBranch",
    "mergePullRequest",
    "enablePullRequestAutoMerge",
    "enqueuePullRequest",
    "revertPullRequest",
    "updateRepository",
    "archiveRepository",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanGate {
    NoPush,
    DraftPrOnly,
    OutwardHeldForApproval,
}

impl HumanGate {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoPush => "noPush",
            Self::DraftPrOnly => "draftPrOnly",
            Self::OutwardHeldForApproval => "outwardHeldForApproval",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gate {
    #[default]
    Open,
    Declared(HumanGate),
    Unparseable,
}

impl Gate {
    /// Parses `GUARD_PROXY_HUMAN_GATE`.
    ///
    /// Unset or blank is open; anything but a profile spelling is unparseable.
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
            return Self::Open;
        };
        match raw {
            "noPush" => Self::Declared(HumanGate::NoPush),
            "draftPrOnly" => Self::Declared(HumanGate::DraftPrOnly),
            "outwardHeldForApproval" => Self::Declared(HumanGate::OutwardHeldForApproval),
            _ => Self::Unparseable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Review {
    Allow,
    Refuse(String),
    NeedBody,
}

#[derive(Debug, Clone, Default)]
pub struct GatePolicy {
    gate: Gate,
    protected: Vec<String>,
}

impl GatePolicy {
    #[must_use]
    pub fn new(gate: Gate, extra_protected: &[String]) -> Self {
        let mut protected = Vec::from(DEFAULT_PROTECTED.map(str::to_owned));
        protected.extend(
            extra_protected
                .iter()
                .map(String::as_str)
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(str::to_owned),
        );
        Self { gate, protected }
    }

    #[must_use]
    pub const fn gate(&self) -> Gate {
        self.gate
    }

    const fn enforced(&self) -> Option<HumanGate> {
        match self.gate {
            Gate::Declared(g @ (HumanGate::NoPush | HumanGate::DraftPrOnly)) => Some(g),
            _ => None,
        }
    }

    /// Why a connection to `host` is refused before any byte is relayed.
    #[must_use]
    pub fn refuse_connection(&self, host: &str, intercepted: bool) -> Option<String> {
        if self.gate == Gate::Unparseable {
            return Some("human gate unparseable: every outbound connection is refused".to_owned());
        }
        let gate = self.enforced()?;
        (!intercepted && is_policed(host)).then(|| {
            format!(
                "human gate {}: {host} is not intercepted, so its writes cannot be reviewed; add \
                 it to the inject config",
                gate.as_str()
            )
        })
    }

    #[must_use]
    pub fn review_head(&self, host: &str, head: &[u8]) -> Review {
        let Some(gate) = self.enforced() else { return Review::Allow };
        let host = host.to_ascii_lowercase();
        if !is_policed(&host) {
            return Review::Allow;
        }
        if header_value(head, "x-http-method-override").is_some() {
            return refuse(gate, "a method-override header");
        }
        let Some(call) = Call::parse(head) else {
            return refuse(gate, "a request target it cannot parse");
        };
        match host.as_str() {
            "api.github.com" => self.api_head(gate, &call),
            "uploads.github.com" if call.writes() => refuse(gate, "an upload"),
            "uploads.github.com" => Review::Allow,
            _ => git_head(gate, &call),
        }
    }

    /// Reviews the decoded body prefix; `complete` when it is the whole body.
    ///
    /// Never answers [`Review::NeedBody`] for a complete body.
    #[must_use]
    pub fn review_body(&self, host: &str, head: &[u8], body: &[u8], complete: bool) -> Review {
        let Some(gate) = self.enforced() else { return Review::Allow };
        if header_value(head, "content-encoding")
            .is_some_and(|v| !v.eq_ignore_ascii_case(b"identity"))
        {
            return refuse(gate, "an encoded request body");
        }
        let Some(call) = Call::parse(head) else {
            return refuse(gate, "a request target it cannot parse");
        };
        if !host.eq_ignore_ascii_case("api.github.com") {
            return self.push_body(gate, body, complete);
        }
        if !complete {
            return Review::NeedBody;
        }
        let Ok(json) = serde_json::from_slice::<Value>(body) else {
            return refuse(gate, "a request body that is not JSON");
        };
        if call.lower == ["graphql"] {
            return graphql_body(gate, &json);
        }
        repo_rest(&call.lower).map_or_else(
            || refuse(gate, "a request it cannot classify"),
            |rest| self.rest_body(gate, &call.method, rest, &json),
        )
    }

    fn api_head(&self, gate: HumanGate, call: &Call) -> Review {
        if call.lower == ["graphql"] {
            return match call.method.as_str() {
                "POST" => Review::NeedBody,
                _ if call.query.is_empty() => Review::Allow,
                _ => refuse(gate, "a GraphQL request outside a POST body"),
            };
        }
        if !call.writes() {
            return Review::Allow;
        }
        let Some(rest) = repo_rest(&call.lower) else { return Review::Allow };
        let original = &call.segments[call.lower.len() - rest.len()..];
        if gate == HumanGate::NoPush {
            no_push_rest(gate, rest)
        } else {
            self.draft_rest(gate, &call.method, rest, original)
        }
    }

    fn draft_rest(
        &self,
        gate: HumanGate,
        method: &str,
        rest: &[String],
        original: &[String],
    ) -> Review {
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        match rest.as_slice() {
            [] | ["transfer"] => refuse(gate, "a repository settings write"),
            ["git", "refs"] | ["pulls"] if method == "POST" => Review::NeedBody,
            ["git", "refs", "heads", _, ..] => self.branch_verdict(gate, &original[3..].join("/")),
            ["git", "refs", ..] => refuse(gate, "a write to a ref that is not a branch"),
            ["contents", ..] | ["merges" | "merge-upstream"] => Review::NeedBody,
            ["pulls", _, "merge"] => refuse(gate, "a pull request merge"),
            ["branches", _, "rename"] => refuse(gate, "a branch rename"),
            ["releases", ..] => refuse(gate, "a release"),
            _ => Review::Allow,
        }
    }

    fn rest_body(&self, gate: HumanGate, method: &str, rest: &[String], json: &Value) -> Review {
        let field = |name: &str| json.get(name).and_then(Value::as_str);
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        match rest.as_slice() {
            ["git", "refs"] => field("ref").and_then(|r| r.strip_prefix("refs/heads/")).map_or_else(
                || refuse(gate, "a write to a ref that is not a branch"),
                |name| self.branch_verdict(gate, name),
            ),
            ["contents", ..] | ["merge-upstream"] => self.named_branch(gate, field("branch")),
            ["merges"] => self.named_branch(gate, field("base")),
            ["pulls"] if method == "POST" => {
                if json.get("draft") == Some(&Value::Bool(true)) {
                    Review::Allow
                } else {
                    refuse(gate, "a pull request that is not a draft")
                }
            }
            _ => refuse(gate, "a request it cannot classify"),
        }
    }

    /// An absent branch names the repository's default.
    fn named_branch(&self, gate: HumanGate, name: Option<&str>) -> Review {
        name.map_or_else(
            || refuse(gate, "a write to the default branch"),
            |name| self.branch_verdict(gate, name.strip_prefix("refs/heads/").unwrap_or(name)),
        )
    }

    fn branch_verdict(&self, gate: HumanGate, name: &str) -> Review {
        if name.is_empty() {
            return refuse(gate, "a write to an unnamed branch");
        }
        if self.protected.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            return refuse(gate, &format!("a write to protected branch `{name}`"));
        }
        Review::Allow
    }

    fn push_body(&self, gate: HumanGate, body: &[u8], complete: bool) -> Review {
        match push_refs(body) {
            PktScan::Refs(refs) => {
                for full in refs {
                    let Some(name) = full.strip_prefix("refs/heads/") else {
                        return refuse(gate, &format!("a push to `{full}`, which is not a branch"));
                    };
                    if let Review::Refuse(reason) = self.branch_verdict(gate, name) {
                        return Review::Refuse(reason);
                    }
                }
                Review::Allow
            }
            PktScan::Incomplete if !complete => Review::NeedBody,
            PktScan::Incomplete | PktScan::Malformed => refuse(gate, "a push it cannot parse"),
        }
    }
}

fn refuse(gate: HumanGate, what: &str) -> Review {
    Review::Refuse(format!("human gate {} refuses {what}", gate.as_str()))
}

fn is_policed(host: &str) -> bool {
    POLICED_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host))
}

fn git_head(gate: HumanGate, call: &Call) -> Review {
    let path = call.lower.join("/");
    let push = path.ends_with("git-receive-pack");
    let discovery = path.ends_with("info/refs") && call.query.contains("service=git-receive-pack");
    match gate {
        HumanGate::NoPush if push || discovery => refuse(gate, "a git push"),
        _ if push && call.method == "POST" => Review::NeedBody,
        _ if push => refuse(gate, "a git push it cannot parse"),
        _ => Review::Allow,
    }
}

fn no_push_rest(gate: HumanGate, rest: &[String]) -> Review {
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    match rest.as_slice() {
        [] | ["transfer"] => refuse(gate, "a repository settings write"),
        ["git", "refs", ..] => refuse(gate, "a ref write"),
        ["contents", ..] => refuse(gate, "a contents write"),
        ["merges" | "merge-upstream"] | ["pulls", _, "merge"] => refuse(gate, "a merge"),
        ["pulls", _, "update-branch"] => refuse(gate, "a pull request branch update"),
        ["branches", _, "rename"] => refuse(gate, "a branch rename"),
        ["releases", ..] => refuse(gate, "a release"),
        _ => Review::Allow,
    }
}

/// The segments after `repos/{owner}/{repo}` or `repositories/{id}`.
fn repo_rest(lower: &[String]) -> Option<&[String]> {
    match lower.first().map(String::as_str) {
        Some("repos") if lower.len() >= 3 => Some(&lower[3..]),
        Some("repositories") if lower.len() >= 2 => Some(&lower[2..]),
        _ => None,
    }
}

fn graphql_body(gate: HumanGate, json: &Value) -> Review {
    let Some(query) = json.get("query").and_then(Value::as_str) else {
        return refuse(gate, "a GraphQL body without a query");
    };
    let Some(tokens) = graphql_tokens(query) else {
        return refuse(gate, "a GraphQL document it cannot parse");
    };
    let draft_only = gate == HumanGate::DraftPrOnly;
    for (i, token) in tokens.iter().enumerate() {
        let Tok::Name(name) = *token else { continue };
        let refused = REF_MUTATIONS.contains(&name)
            || (!draft_only && name == "updatePullRequestBranch")
            || (draft_only && name == "markPullRequestReadyForReview");
        if refused {
            return refuse(gate, &format!("the `{name}` mutation"));
        }
        if draft_only && name == "createPullRequest" && !creates_draft(&tokens[i + 1..], json) {
            return refuse(gate, "a pull request that is not a draft");
        }
    }
    Review::Allow
}

/// True only for `createPullRequest(input: $v)` whose `$v` sets `draft: true`.
fn creates_draft(after: &[Tok<'_>], json: &Value) -> bool {
    let [Tok::Punct('('), Tok::Name("input"), Tok::Punct(':'), Tok::Var(var), Tok::Punct(')'), ..] =
        after
    else {
        return false;
    };
    json.get("variables").and_then(|v| v.get(*var)).and_then(|input| input.get("draft"))
        == Some(&Value::Bool(true))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok<'a> {
    Name(&'a str),
    Var(&'a str),
    Punct(char),
}

/// Lexes a GraphQL document, dropping strings, comments and commas.
///
/// `None` on an unterminated string.
fn graphql_tokens(src: &str) -> Option<Vec<Tok<'_>>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() || b == b',' {
            i += 1;
        } else if b == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if b == b'"' {
            i = skip_string(bytes, i)?;
        } else if b == b'$' || b == b'_' || b.is_ascii_alphanumeric() {
            let start = if b == b'$' { i + 1 } else { i };
            let mut end = start;
            while end < bytes.len() && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric()) {
                end += 1;
            }
            let word = &src[start..end];
            out.push(if b == b'$' { Tok::Var(word) } else { Tok::Name(word) });
            i = end.max(i + 1);
        } else {
            out.push(Tok::Punct(char::from(b)));
            i += 1;
        }
    }
    Some(out)
}

/// The index just past the string literal opening at `start`.
fn skip_string(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes[start..].starts_with(b"\"\"\"") {
        let mut i = start + 3;
        while i + 2 < bytes.len() {
            if bytes[i] == b'\\' && bytes[i + 1..].starts_with(b"\"\"\"") {
                i += 4;
            } else if bytes[i..].starts_with(b"\"\"\"") {
                return Some(i + 3);
            } else {
                i += 1;
            }
        }
        return None;
    }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            b'\n' => return None,
            _ => i += 1,
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum PktScan {
    Refs(Vec<String>),
    Incomplete,
    Malformed,
}

/// The refs a `git-receive-pack` request updates: its pkt-line command list,
/// up to the first flush packet.
fn push_refs(body: &[u8]) -> PktScan {
    if body.is_empty() {
        return PktScan::Incomplete;
    }
    let mut refs = Vec::new();
    let mut i = 0;
    loop {
        let Some(len_hex) = body.get(i..i + 4) else { return PktScan::Incomplete };
        let Some(len) =
            std::str::from_utf8(len_hex).ok().and_then(|h| usize::from_str_radix(h, 16).ok())
        else {
            return PktScan::Malformed;
        };
        if len == 0 {
            return PktScan::Refs(refs);
        }
        if len < 4 {
            return PktScan::Malformed;
        }
        let Some(payload) = body.get(i + 4..i + len) else { return PktScan::Incomplete };
        i += len;
        let line = payload.split(|&b| b == 0).next().unwrap_or_default();
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let Ok(line) = std::str::from_utf8(line) else { return PktScan::Malformed };
        if line.starts_with("shallow ") {
            continue;
        }
        let mut parts = line.split(' ');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(old), Some(new), Some(name), None) if is_oid(old) && is_oid(new) => {
                refs.push(name.to_owned());
            }
            _ => return PktScan::Malformed,
        }
    }
}

fn is_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The decoded body prefix in `raw`, and whether it is the whole body.
#[must_use]
pub fn body_prefix(raw: &[u8], framing: Framing) -> (Cow<'_, [u8]>, bool) {
    match framing {
        Framing::None | Framing::UntilClose => (Cow::Borrowed(&[]), true),
        Framing::Length(len) => (Cow::Borrowed(&raw[..raw.len().min(len)]), raw.len() >= len),
        Framing::Chunked => {
            let (body, complete) = dechunk(raw);
            (Cow::Owned(body), complete)
        }
    }
}

fn dechunk(raw: &[u8]) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let Some(eol) = raw[i..].iter().position(|&b| b == b'\n') else { return (out, false) };
        let line = String::from_utf8_lossy(&raw[i..i + eol]);
        let size = line.split(';').next().unwrap_or_default().trim();
        let Ok(size) = usize::from_str_radix(size, 16) else { return (out, false) };
        i += eol + 1;
        if size == 0 {
            return (out, true);
        }
        let end = (i + size).min(raw.len());
        out.extend_from_slice(&raw[i..end]);
        if end < i + size {
            return (out, false);
        }
        i = (i + size + 2).min(raw.len());
    }
}

struct Call {
    method: String,
    query: String,
    segments: Vec<String>,
    lower: Vec<String>,
}

impl Call {
    /// Refuses (`None`) what a gate cannot route the way GitHub would: an
    /// absolute-form target, bad percent-encoding, a dot segment.
    fn parse(head: &[u8]) -> Option<Self> {
        let line = head.split(|&b| b == b'\n').next()?;
        let line = std::str::from_utf8(line).ok()?.trim_end();
        let mut parts = line.split(' ');
        let method = parts.next()?.to_ascii_uppercase();
        let target = parts.next()?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if !path.starts_with('/') {
            return None;
        }
        let decoded = percent_decode(path)?;
        let segments: Vec<String> =
            decoded.split('/').filter(|s| !s.is_empty()).map(str::to_owned).collect();
        if segments.iter().any(|s| s == "." || s == "..") {
            return None;
        }
        let lower = segments.iter().map(|s| s.to_ascii_lowercase()).collect();
        Some(Self { method, query: query.to_ascii_lowercase(), segments, lower })
    }

    fn writes(&self) -> bool {
        !matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS")
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "0000000000000000000000000000000000000000";
    const NEW: &str = "1111111111111111111111111111111111111111";
    const RECEIVE: &str = "/o/r.git/git-receive-pack";
    const GH_CREATE: &str = "mutation PullRequestCreate($input: CreatePullRequestInput!) {\n  \
                             createPullRequest(input: $input) { pullRequest { id url } }\n}";

    fn policy(gate: HumanGate) -> GatePolicy {
        GatePolicy::new(Gate::Declared(gate), &[])
    }

    fn head(method: &str, target: &str) -> Vec<u8> {
        format!("{method} {target} HTTP/1.1\r\nHost: x\r\n\r\n").into_bytes()
    }

    fn pkt(line: &str) -> String {
        format!("{:04x}{line}", line.len() + 4)
    }

    fn push_body(refs: &[&str]) -> Vec<u8> {
        let mut body = String::new();
        for (i, r) in refs.iter().enumerate() {
            let caps = if i == 0 { "\0report-status side-band-64k" } else { "" };
            body.push_str(&pkt(&format!("{OLD} {NEW} {r}{caps}\n")));
        }
        body.push_str("0000PACK....");
        body.into_bytes()
    }

    /// Head first, then the whole body, the way the injector reviews.
    fn review(p: &GatePolicy, host: &str, method: &str, target: &str, body: &[u8]) -> Review {
        let h = head(method, target);
        match p.review_head(host, &h) {
            Review::NeedBody => p.review_body(host, &h, body, true),
            other => other,
        }
    }

    fn api(p: &GatePolicy, method: &str, target: &str, body: &[u8]) -> Review {
        review(p, "api.github.com", method, target, body)
    }

    fn graphql(p: &GatePolicy, query: &str, variables: &Value) -> Review {
        let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
        api(p, "POST", "/graphql", body.as_bytes())
    }

    fn refused(r: &Review) -> bool {
        matches!(r, Review::Refuse(_))
    }

    #[test]
    fn gate_parses_only_the_profile_spellings() {
        assert_eq!(Gate::parse(None), Gate::Open);
        assert_eq!(Gate::parse(Some("  ")), Gate::Open);
        assert_eq!(Gate::parse(Some("noPush")), Gate::Declared(HumanGate::NoPush));
        assert_eq!(Gate::parse(Some(" draftPrOnly ")), Gate::Declared(HumanGate::DraftPrOnly));
        assert_eq!(
            Gate::parse(Some("outwardHeldForApproval")),
            Gate::Declared(HumanGate::OutwardHeldForApproval)
        );
        for bad in ["nopush", "no_push", "draft", "true"] {
            assert_eq!(Gate::parse(Some(bad)), Gate::Unparseable, "{bad}");
        }
    }

    #[test]
    fn unparseable_gate_refuses_every_connection() {
        let p = GatePolicy::new(Gate::parse(Some("draft_pr_only")), &[]);
        for (host, intercepted) in
            [("api.github.com", true), ("api.anthropic.com", false), ("registry.npmjs.org", true)]
        {
            assert!(p.refuse_connection(host, intercepted).is_some(), "{host}");
        }
    }

    #[test]
    fn no_gate_keeps_todays_behaviour() {
        let p = GatePolicy::default();
        assert!(p.refuse_connection("github.com", false).is_none());
        let push = push_body(&["refs/heads/main"]);
        assert_eq!(review(&p, "github.com", "POST", RECEIVE, &push), Review::Allow);
        assert_eq!(api(&p, "PUT", "/repos/o/r/pulls/1/merge", b"{}"), Review::Allow);
        let ready = "mutation { markPullRequestReadyForReview(input: {}) { clientMutationId } }";
        assert_eq!(graphql(&p, ready, &Value::Null), Review::Allow);
    }

    #[test]
    fn outward_held_for_approval_is_not_enforced_by_the_guard() {
        let p = policy(HumanGate::OutwardHeldForApproval);
        assert!(p.refuse_connection("github.com", false).is_none());
        let push = push_body(&["refs/heads/main"]);
        assert_eq!(review(&p, "github.com", "POST", RECEIVE, &push), Review::Allow);
    }

    #[test]
    fn enforced_gate_refuses_an_unintercepted_github_host_only() {
        for gate in [HumanGate::NoPush, HumanGate::DraftPrOnly] {
            let p = policy(gate);
            assert!(p.refuse_connection("github.com", false).is_some());
            assert!(p.refuse_connection("API.GitHub.com", false).is_some());
            assert!(p.refuse_connection("github.com", true).is_none());
            assert!(p.refuse_connection("api.anthropic.com", false).is_none());
            assert!(p.refuse_connection("objects.githubusercontent.com", false).is_none());
        }
    }

    #[test]
    fn no_push_refuses_git_push() {
        let p = policy(HumanGate::NoPush);
        let push = push_body(&["refs/heads/feature/x"]);
        assert!(refused(&review(&p, "github.com", "POST", RECEIVE, &push)));
        assert!(refused(&review(&p, "github.com", "POST", "/o/r/git-receive-pack", &push)));
        let discovery = "/o/r.git/info/refs?service=git-receive-pack";
        assert!(refused(&review(&p, "github.com", "GET", discovery, b"")));
        let fetch = "/o/r.git/info/refs?service=git-upload-pack";
        assert_eq!(review(&p, "github.com", "GET", fetch, b""), Review::Allow);
        let upload = "/o/r.git/git-upload-pack";
        assert_eq!(review(&p, "github.com", "POST", upload, b"0000"), Review::Allow);
    }

    #[test]
    fn no_push_refuses_api_writes_to_refs_and_contents() {
        let p = policy(HumanGate::NoPush);
        for (method, target) in [
            ("POST", "/repos/o/r/git/refs"),
            ("PATCH", "/repos/o/r/git/refs/heads/feature"),
            ("DELETE", "/repos/o/r/git/refs/tags/v1"),
            ("PUT", "/repos/o/r/contents/README.md"),
            ("DELETE", "/repos/o/r/contents/a/b.rs"),
            ("POST", "/repos/o/r/merges"),
            ("POST", "/repos/o/r/merge-upstream"),
            ("PUT", "/repos/o/r/pulls/7/merge"),
            ("PUT", "/repos/o/r/pulls/7/update-branch"),
            ("POST", "/repos/o/r/branches/main/rename"),
            ("POST", "/repos/o/r/releases"),
            ("PATCH", "/repos/o/r"),
            ("DELETE", "/repos/o/r"),
            ("PUT", "/repositories/42/pulls/7/merge"),
            ("PUT", "/repos/o/r/pulls%2F7%2Fmerge"),
            ("PUT", "/Repos/o/r/Pulls/7/Merge"),
        ] {
            assert!(refused(&api(&p, method, target, b"{}")), "{method} {target}");
        }
        let names = REF_MUTATIONS.iter().copied().chain(["updatePullRequestBranch"]);
        for name in names {
            let query = format!("mutation($i: X!) {{ {name}(input: $i) {{ clientMutationId }} }}");
            let verdict = graphql(&p, &query, &serde_json::json!({ "i": {} }));
            assert!(refused(&verdict), "{name}");
        }
    }

    #[test]
    fn no_push_allows_reads_comments_and_pull_requests() {
        let p = policy(HumanGate::NoPush);
        for (method, target, body) in [
            ("GET", "/repos/o/r/contents/README.md", &b""[..]),
            ("POST", "/repos/o/r/issues/3/comments", &br#"{"body":"hi"}"#[..]),
            ("POST", "/repos/o/r/pulls", &br#"{"head":"f","base":"main"}"#[..]),
            ("POST", "/repos/o/r/git/blobs", &b"{}"[..]),
        ] {
            assert_eq!(api(&p, method, target, body), Review::Allow, "{target}");
        }
        let query = "query { repository(owner: \"o\", name: \"mergeBranch\") { id } }";
        assert_eq!(graphql(&p, query, &Value::Null), Review::Allow);
    }

    #[test]
    fn draft_pr_only_refuses_a_default_branch_push() {
        let p = policy(HumanGate::DraftPrOnly);
        for refs in [
            &["refs/heads/main"][..],
            &["refs/heads/feature", "refs/heads/master"][..],
            &["refs/tags/v1"][..],
        ] {
            let verdict = review(&p, "github.com", "POST", RECEIVE, &push_body(refs));
            assert!(refused(&verdict), "{refs:?}");
        }
        let gate = Gate::Declared(HumanGate::DraftPrOnly);
        let extra = GatePolicy::new(gate, &["release".to_owned()]);
        let push = push_body(&["refs/heads/release"]);
        assert!(refused(&review(&extra, "github.com", "POST", RECEIVE, &push)));
    }

    #[test]
    fn draft_pr_only_allows_a_feature_branch_push() {
        let p = policy(HumanGate::DraftPrOnly);
        let body = push_body(&["refs/heads/cctui/fix-1", "refs/heads/feature/main-fix"]);
        assert_eq!(review(&p, "github.com", "POST", RECEIVE, &body), Review::Allow);
        let discovery = "/o/r.git/info/refs?service=git-receive-pack";
        assert_eq!(review(&p, "github.com", "GET", discovery, b""), Review::Allow);
        assert_eq!(review(&p, "github.com", "POST", RECEIVE, b"0000"), Review::Allow);
    }

    #[test]
    fn draft_pr_only_decides_a_push_from_its_command_list_alone() {
        let p = policy(HumanGate::DraftPrOnly);
        let h = head("POST", RECEIVE);
        assert_eq!(p.review_head("github.com", &h), Review::NeedBody);
        let body = push_body(&["refs/heads/feature"]);
        assert_eq!(p.review_body("github.com", &h, &body[..20], false), Review::NeedBody);
        assert_eq!(p.review_body("github.com", &h, &body, false), Review::Allow);
        assert!(refused(&p.review_body("github.com", &h, &body[..20], true)));
        assert!(refused(&p.review_body("github.com", &h, b"zzzz", false)));
        let cert = pkt("push-cert\0report-status\n");
        assert!(refused(&p.review_body("github.com", &h, cert.as_bytes(), false)));
    }

    #[test]
    fn draft_pr_only_refuses_a_non_draft_pull_request() {
        let p = policy(HumanGate::DraftPrOnly);
        for body in [
            &br#"{"head":"f","base":"main"}"#[..],
            &br#"{"draft":false}"#[..],
            &br#"{"draft":"true"}"#[..],
        ] {
            assert!(refused(&api(&p, "POST", "/repos/o/r/pulls", body)));
        }
        let open = serde_json::json!({ "input": { "draft": false } });
        assert!(refused(&graphql(&p, GH_CREATE, &open)));
        assert!(refused(&graphql(&p, GH_CREATE, &serde_json::json!({ "input": {} }))));
        let inline = "mutation { createPullRequest(input: {draft: true}) { clientMutationId } }";
        assert!(refused(&graphql(&p, inline, &Value::Null)));
        let decoy = "mutation($input: CreatePullRequestInput!, $d: X) { \
                     createPullRequest(input: $input) { clientMutationId } }";
        let vars = serde_json::json!({ "input": { "draft": false }, "d": { "draft": true } });
        assert!(refused(&graphql(&p, decoy, &vars)));
    }

    #[test]
    fn draft_pr_only_allows_a_draft_pull_request() {
        let p = policy(HumanGate::DraftPrOnly);
        let rest = br#"{"head":"f","base":"main","title":"t","draft":true}"#;
        assert_eq!(api(&p, "POST", "/repos/o/r/pulls", rest), Review::Allow);
        let vars = serde_json::json!({ "input": { "draft": true, "title": "createPullRequest" } });
        assert_eq!(graphql(&p, GH_CREATE, &vars), Review::Allow);
    }

    #[test]
    fn draft_pr_only_refuses_ready_for_review_and_merge() {
        let p = policy(HumanGate::DraftPrOnly);
        for mutation in [
            "markPullRequestReadyForReview(input: $i)",
            "mergePullRequest(input: $i)",
            "enablePullRequestAutoMerge(input: $i)",
            "alias: enqueuePullRequest(input: $i)",
            "mergeBranch(input: $i)",
        ] {
            let query = format!("mutation($i: X!) {{ {mutation} {{ clientMutationId }} }}");
            let verdict = graphql(&p, &query, &serde_json::json!({ "i": {} }));
            assert!(refused(&verdict), "{mutation}");
        }
        assert!(refused(&api(&p, "PUT", "/repos/o/r/pulls/7/merge", b"{}")));
        let merge = br#"{"base":"main","head":"f"}"#;
        assert!(refused(&api(&p, "POST", "/repos/o/r/merges", merge)));
    }

    #[test]
    fn draft_pr_only_reviews_branch_writes_by_name() {
        let p = policy(HumanGate::DraftPrOnly);
        for (method, target, body) in [
            ("PATCH", "/repos/o/r/git/refs/heads/main", &b"{}"[..]),
            ("POST", "/repos/o/r/git/refs", &br#"{"ref":"refs/heads/master","sha":"x"}"#[..]),
            ("POST", "/repos/o/r/git/refs", &br#"{"ref":"refs/tags/v1","sha":"x"}"#[..]),
            ("PUT", "/repos/o/r/contents/a.md", &br#"{"message":"m","content":"eA=="}"#[..]),
            ("PUT", "/repos/o/r/contents/a.md", &br#"{"branch":"main"}"#[..]),
            ("POST", "/repos/o/r/releases", &b"{}"[..]),
            ("PATCH", "/repos/o/r", &br#"{"default_branch":"feature"}"#[..]),
        ] {
            assert!(refused(&api(&p, method, target, body)), "{method} {target}");
        }
        for (method, target, body) in [
            ("PATCH", "/repos/o/r/git/refs/heads/feature%2Fx", &b"{}"[..]),
            ("POST", "/repos/o/r/git/refs", &br#"{"ref":"refs/heads/feature","sha":"x"}"#[..]),
            ("PUT", "/repos/o/r/contents/a.md", &br#"{"branch":"feature"}"#[..]),
            ("PUT", "/repos/o/r/pulls/7/update-branch", &b"{}"[..]),
        ] {
            assert_eq!(api(&p, method, target, body), Review::Allow, "{method} {target}");
        }
    }

    #[test]
    fn enforced_gate_refuses_requests_it_cannot_read() {
        let p = policy(HumanGate::DraftPrOnly);
        assert!(refused(&api(&p, "PUT", "/repos/o/r/%zz/merge", b"{}")));
        assert!(refused(&api(&p, "PUT", "/repos/o/r/x/../pulls/1/merge", b"{}")));
        assert!(refused(&api(&p, "POST", "/graphql", b"not json")));
        assert!(refused(&api(&p, "POST", "/graphql", br#"[{"query":"query{a}"}]"#)));
        assert!(refused(&api(&p, "POST", "/graphql", br#"{"query":"mutation { a(s: \"x) }"}"#)));
        let upload = "/repos/o/r/releases/1/assets";
        assert!(refused(&review(&p, "uploads.github.com", "POST", upload, b"")));
        let over = b"POST /repos/o/r/pulls HTTP/1.1\r\nX-HTTP-Method-Override: PUT\r\n\r\n";
        assert!(refused(&p.review_head("api.github.com", over)));
        let gz = b"POST /graphql HTTP/1.1\r\nContent-Encoding: gzip\r\n\r\n";
        assert!(refused(&p.review_body("api.github.com", gz, b"\x1f\x8b", true)));
    }

    #[test]
    fn body_prefix_decodes_counted_and_chunked_bodies() {
        let (b, complete) = body_prefix(b"abcdefNEXT", Framing::Length(6));
        assert_eq!((&*b, complete), (&b"abcdef"[..], true));
        let (b, complete) = body_prefix(b"abc", Framing::Length(6));
        assert_eq!((&*b, complete), (&b"abc"[..], false));
        let (b, complete) = body_prefix(b"3\r\nabc\r\n2;x=y\r\nde\r\n0\r\n\r\n", Framing::Chunked);
        assert_eq!((&*b, complete), (&b"abcde"[..], true));
        let (b, complete) = body_prefix(b"5\r\nab", Framing::Chunked);
        assert_eq!((&*b, complete), (&b"ab"[..], false));
        let (b, complete) = body_prefix(b"", Framing::None);
        assert!(b.is_empty() && complete);
    }
}
