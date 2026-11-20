//! `GET /sessions/{id}/brief`: the user/assistant transcript of a session as
//! markdown, for a follow-up session's first prompt. Tool calls, tool
//! results, thinking, harness meta and keep-alive ticks are dropped; images
//! stay as the name the transcript already references them by. Consecutive
//! chunks of one speaker collapse into one turn. The oldest turns are elided
//! behind one line once the turn or byte cap is hit.
//!
//! The cctui wrappers leading a user turn describe the old session, and a
//! follow-up wraps this brief in its own: session context is dropped and a
//! previous conversation shrinks to one line, so chains never nest. An inbox
//! batch keeps its numbered event lines and loses its header.
//!
//! `?chain=1` renders every turn of every session of the follow-up line ending
//! at `id`, root first. Only past the chain ceiling ([`CHAIN_MAX_BYTES_ENV`])
//! are turns elided, from the middle of the oldest sessions first, behind an
//! explicit marker; the root's first ask is never elided. Each session also
//! carries the newest turns that fit inline beside the pointer to the
//! [`BRIEF_FILE_NAME`] file holding the whole chain ([`followup_envelope`]).

use std::collections::HashSet;
use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use cctui_proto::api::{ApiError, BriefResponse, BriefSession};
use cctui_proto::injected::{
    InjectedBlock, InjectedKind, PREVIOUS_CONVERSATION, PREVIOUS_SESSIONS, split_injected,
};

use crate::auth::AuthContext;
use crate::state::AppState;

pub const DEFAULT_MAX_TURNS: usize = 60;
pub const DEFAULT_MAX_BYTES: usize = 48 * 1024;
pub const BRIEF_FILE_NAME: &str = "previous-conversation.md";
/// A follow-up's previous conversation stays in its prompt up to this size.
pub const INLINE_MAX_BYTES: usize = 48 * 1024;
pub const DEFAULT_CHAIN_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const CHAIN_MAX_BYTES_ENV: &str = "CCTUI_FOLLOWUP_BRIEF_MAX_BYTES";
const USER_PREFIX: &str = "▷ User: ";
const CONTINUED_PREFIX: &str = "(continued from session \"";
const DICTATED_LINE: &str = "(dictated: may hold speech-recognition errors)\n\n";
const MARKER_BYTES: usize = 160;
const TAIL_NOTE_BYTES: usize = 64;
const MAX_ATTR_CHARS: usize = 200;
const FILE_POINTER: &str = "In the attached file previous-conversation.md.";
const READ_POINTER: &str = "The whole conversation is in the attached file \
    previous-conversation.md. Read all of it before you continue: it can be longer than one \
    Read call returns, so keep reading until the end of the file.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    const fn heading(self) -> &'static str {
        match self {
            Self::User => "**User:**",
            Self::Assistant => "**Assistant:**",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub max_turns: usize,
    pub max_bytes: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Self { max_turns: DEFAULT_MAX_TURNS, max_bytes: DEFAULT_MAX_BYTES }
    }
}

fn is_keepalive(payload: &Value) -> bool {
    payload.get("metadata").and_then(|m| m.get("keepalive")).and_then(Value::as_bool) == Some(true)
}

/// The speaker and prose of one stored row, or `None` for anything the brief
/// drops.
fn classify(adapter_id: &str, event_type: &str, payload: Value) -> Option<(Role, String)> {
    let v = crate::normalize::for_client(adapter_id, event_type, payload)?;
    if v.get("type").and_then(Value::as_str) != Some("text")
        || v.get("meta").and_then(Value::as_bool).unwrap_or(false)
    {
        return None;
    }
    let content = v.get("content").and_then(Value::as_str).unwrap_or_default();
    let kind = v.get("kind").and_then(Value::as_str);
    if let Some(text) = content.strip_prefix(USER_PREFIX) {
        return without_injected(text)
            .and_then(without_inbox_marker)
            .map(|text| (Role::User, text));
    }
    if v.get("role").and_then(Value::as_str) == Some("Assistant")
        && matches!(kind, None | Some("attachment"))
    {
        return Some((Role::Assistant, content.to_owned()));
    }
    None
}

fn is_previous(block: &InjectedBlock<'_>) -> bool {
    block.kind == InjectedKind::PreviousConversation
}

/// The previous sessions `blocks` name, a chain container expanded into the
/// sessions it holds.
fn previous_sessions<'a>(blocks: &[InjectedBlock<'a>]) -> Vec<InjectedBlock<'a>> {
    let mut out = Vec::new();
    for block in blocks.iter().filter(|b| is_previous(b)) {
        if block.tag == PREVIOUS_SESSIONS {
            out.extend(split_injected(block.body).0.into_iter().filter(|b| is_previous(b)));
        } else {
            out.push(*block);
        }
    }
    out
}

/// A user turn without its leading cctui wrappers, or `None` when nothing but
/// wrappers was left.
fn without_injected(text: &str) -> Option<String> {
    let (blocks, prose) = split_injected(text);
    if blocks.is_empty() {
        return Some(text.to_owned());
    }
    let prose = prose.trim();
    if prose.is_empty() {
        return None;
    }
    let mut out = String::new();
    for block in previous_sessions(&blocks) {
        let name = block.attr("session").unwrap_or_default();
        let id = block.attr("id").unwrap_or_default();
        let _ = writeln!(out, "{CONTINUED_PREFIX}{name}\" {id})");
    }
    if !out.is_empty() {
        out.push('\n');
    }
    if cctui_proto::injected::is_dictated_prompt(text) {
        out.push_str(DICTATED_LINE);
    }
    out.push_str(prose);
    Some(out)
}

fn continued_id(line: &str) -> Option<&str> {
    let inner = line.trim_end().strip_prefix(CONTINUED_PREFIX)?.strip_suffix(')')?;
    inner.rsplit_once(' ').map(|(_, id)| id)
}

/// `text` without its leading `(continued from …)` lines naming a session in
/// `included`.
fn without_continued(text: &str, included: &HashSet<&str>) -> String {
    let mut kept = String::new();
    let mut rest = text;
    loop {
        let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
        let Some(id) = continued_id(line) else { break };
        if !included.contains(id) {
            kept.push_str(line);
            kept.push('\n');
        }
        rest = after;
    }
    if kept.is_empty() {
        return rest.trim_start().to_owned();
    }
    kept.push_str(rest);
    kept.trim_end().to_owned()
}

fn without_inbox_marker(text: String) -> Option<String> {
    let Some(rest) = text.trim_start().strip_prefix(crate::inbox::deliver::MARKER) else {
        return Some(text);
    };
    let (head, events) = rest.split_once('\n').unwrap_or((rest, ""));
    if !head.trim_end().ends_with(']') {
        return Some(text);
    }
    let events = events.trim();
    (!events.is_empty()).then(|| events.to_owned())
}

/// Fold stored rows (oldest first) into speaker turns.
pub fn collect_turns<'a>(
    adapter_id: &str,
    rows: impl IntoIterator<Item = (&'a str, Value)>,
) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut in_tick = false;
    for (event_type, payload) in rows {
        if is_keepalive(&payload) {
            in_tick = true;
            continue;
        }
        let Some((role, text)) = classify(adapter_id, event_type, payload) else { continue };
        if in_tick && role == Role::Assistant {
            continue;
        }
        in_tick = false;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match turns.last_mut() {
            Some(last) if last.role == role => {
                last.text.push_str("\n\n");
                last.text.push_str(text);
            }
            _ => turns.push(Turn { role, text: text.to_owned() }),
        }
    }
    turns
}

const fn turn_len(turn: &Turn) -> usize {
    turn.role.heading().len() + 2 + turn.text.len() + 2
}

fn cut_head(text: &str, keep: usize) -> String {
    if text.len() <= keep {
        return text.to_owned();
    }
    let mut start = text.len() - keep;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// Render the newest turns that fit the caps.
pub fn render(turns: &[Turn], caps: Caps) -> BriefResponse {
    let max_turns = caps.max_turns.max(1);
    let mut start = turns.len().saturating_sub(max_turns);
    let mut truncated = false;
    let mut bytes: usize = turns[start..].iter().map(turn_len).sum();
    while bytes > caps.max_bytes && start + 1 < turns.len() {
        bytes -= turn_len(&turns[start]);
        start += 1;
        truncated = true;
    }
    let omitted = start;
    let mut kept: Vec<Turn> = turns[start..].to_vec();
    if let Some(only) = kept.first_mut()
        && turn_len(only) > caps.max_bytes
    {
        let overhead = turn_len(only) - only.text.len();
        only.text = cut_head(&only.text, caps.max_bytes.saturating_sub(overhead));
        truncated = true;
    }
    let mut markdown = String::new();
    if omitted > 0 {
        let _ = write!(markdown, "… {omitted} earlier turns omitted\n\n");
    }
    for turn in &kept {
        markdown.push_str(turn.role.heading());
        markdown.push_str("\n\n");
        markdown.push_str(&turn.text);
        markdown.push_str("\n\n");
    }
    let markdown = markdown.trim_end().to_owned();
    BriefResponse {
        markdown,
        turns: u32::try_from(kept.len()).unwrap_or(u32::MAX),
        omitted: u32::try_from(omitted).unwrap_or(u32::MAX),
        truncated,
        sessions: None,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// One session of a follow-up chain; `transcript` is `None` when the caller
/// cannot read it.
pub struct Link {
    pub id: String,
    pub transcript: Option<Transcript>,
}

fn unavailable(id: &str) -> String {
    format!("(session {id} could not be included)")
}

fn heading(n: usize, link: &Link) -> String {
    link.transcript.as_ref().map_or_else(
        || format!("### Session {n} ({})", link.id),
        |t| format!("### Session {n} \"{}\" ({})", t.name, link.id),
    )
}

/// A chain session's turns; `turns[..keep]` is never elided.
struct Part {
    turns: Vec<Turn>,
    keep: usize,
    cut: Option<(usize, usize)>,
}

fn middle_window(
    turns: &[Turn],
    lo: usize,
    hi: usize,
    need: usize,
) -> Option<(usize, usize, usize)> {
    if lo >= hi {
        return None;
    }
    let mid = lo + (hi - lo) / 2;
    let (mut start, mut end, mut freed) = (mid, mid, 0);
    while freed < need && (start > lo || end < hi) {
        if end < hi {
            freed += turn_len(&turns[end]);
            end += 1;
        }
        if freed < need && start > lo {
            start -= 1;
            freed += turn_len(&turns[start]);
        }
    }
    Some((start, end, freed))
}

/// The newest session is cut only as a last resort, and never its last turn.
fn elide(parts: &mut [Part], mut over: usize) {
    let Some(newest) = parts.len().checked_sub(1) else { return };
    for i in (0..newest).chain([newest]) {
        if over == 0 {
            return;
        }
        let part = &mut parts[i];
        let hi = if i == newest { part.turns.len().saturating_sub(1) } else { part.turns.len() };
        let Some((start, end, freed)) =
            middle_window(&part.turns, part.keep, hi, over + MARKER_BYTES)
        else {
            continue;
        };
        part.cut = Some((start, end));
        over = over.saturating_sub(freed.saturating_sub(MARKER_BYTES));
    }
}

fn marker(n: usize, ceiling: usize) -> String {
    format!(
        "… {n} turns elided here: the follow-up chain is over its {} KiB ceiling",
        ceiling / 1024
    )
}

fn pieces(part: &Part, ceiling: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(part.turns.len());
    for (i, turn) in part.turns.iter().enumerate() {
        match part.cut {
            Some((start, end)) if i == start => out.push(marker(end - start, ceiling)),
            Some((start, end)) if (start..end).contains(&i) => {}
            _ => out.push(format!("{}\n\n{}", turn.role.heading(), turn.text)),
        }
    }
    out
}

fn tails(pieces: &[Vec<String>], budget: usize) -> Vec<Option<String>> {
    let mut out = vec![None; pieces.len()];
    let mut left = budget;
    for (i, session) in pieces.iter().enumerate().rev() {
        left = left.saturating_sub(TAIL_NOTE_BYTES);
        let mut from = session.len();
        while from > 0 && session[from - 1].len() + 2 <= left {
            from -= 1;
            left -= session[from].len() + 2;
        }
        if from < session.len() {
            let note = if from > 0 {
                format!("… {from} earlier turns are in the file\n\n")
            } else {
                String::new()
            };
            out[i] = Some(format!("{note}{}", session[from..].join("\n\n")));
        }
        if from > 0 {
            break;
        }
    }
    out
}

fn chain_parts(links: &[Link]) -> Vec<Part> {
    let included: HashSet<&str> =
        links.iter().filter(|l| l.transcript.is_some()).map(|l| l.id.as_str()).collect();
    links
        .iter()
        .enumerate()
        .map(|(i, link)| {
            let turns: Vec<Turn> = link.transcript.as_ref().map_or_else(Vec::new, |t| {
                t.turns
                    .iter()
                    .filter_map(|turn| {
                        let text = without_continued(&turn.text, &included);
                        (!text.is_empty()).then_some(Turn { role: turn.role, text })
                    })
                    .collect()
            });
            let keep = if i == 0 {
                turns.iter().position(|t| t.role == Role::User).map_or(0, |at| at + 1)
            } else {
                0
            };
            Part { turns, keep, cut: None }
        })
        .collect()
}

fn chain_bytes(links: &[Link], parts: &[Part], headings: &[String]) -> usize {
    headings.iter().map(|h| h.len() + 4).sum::<usize>()
        + links
            .iter()
            .zip(parts)
            .map(|(link, part)| {
                if link.transcript.is_some() {
                    part.turns.iter().map(turn_len).sum::<usize>()
                } else {
                    unavailable(&link.id).len()
                }
            })
            .sum::<usize>()
}

/// The chain `links` (root first) as one brief: every turn, elided only past
/// `ceiling` bytes.
pub fn assemble(links: &[Link], ceiling: usize) -> BriefResponse {
    let mut parts = chain_parts(links);
    let headings: Vec<String> =
        links.iter().enumerate().map(|(i, link)| heading(i + 1, link)).collect();
    elide(&mut parts, chain_bytes(links, &parts, &headings).saturating_sub(ceiling));
    let rendered: Vec<Vec<String>> = parts.iter().map(|p| pieces(p, ceiling)).collect();
    let mut sessions: Vec<BriefSession> = links
        .iter()
        .zip(&parts)
        .zip(&rendered)
        .map(|((link, part), pieces)| {
            let cut = part.cut.map_or(0, |(start, end)| end - start);
            BriefSession {
                id: link.id.clone(),
                name: link.transcript.as_ref().map(|t| t.name.clone()),
                markdown: if link.transcript.is_some() {
                    pieces.join("\n\n")
                } else {
                    unavailable(&link.id)
                },
                turns: count(part.turns.len() - cut),
                omitted: count(cut),
                truncated: cut > 0,
                tail: None,
            }
        })
        .collect();
    let overhead = followup_envelope(&sessions).inline.len() + 2 * sessions.len();
    let budget = INLINE_MAX_BYTES.saturating_sub(overhead);
    for (session, tail) in sessions.iter_mut().zip(tails(&rendered, budget)) {
        session.tail = tail;
    }
    let markdown = sessions
        .iter()
        .zip(&headings)
        .map(|(part, heading)| format!("{heading}\n\n{}", part.markdown))
        .collect::<Vec<_>>()
        .join("\n\n");
    BriefResponse {
        markdown,
        turns: sessions.iter().fold(0, |n, p| n.saturating_add(p.turns)),
        omitted: sessions.iter().fold(0, |n, p| n.saturating_add(p.omitted)),
        truncated: sessions.iter().any(|p| p.truncated),
        sessions: Some(sessions),
    }
}

fn attr(raw: &str) -> String {
    let clipped: String = raw.chars().take(MAX_ATTR_CHARS).collect();
    clipped.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// A follow-up's previous conversation: what its prompt carries `inline`, and
/// the [`BRIEF_FILE_NAME`] file it points at when the transcript is not inline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowupEnvelope {
    pub inline: String,
    pub file: Option<String>,
}

fn pointed(pointer: &str, tail: Option<&str>) -> String {
    tail.map_or_else(|| pointer.to_owned(), |tail| format!("{pointer}\n\n{tail}"))
}

fn chain_envelope(
    sessions: &[BriefSession],
    body: impl Fn(usize, &BriefSession) -> String,
) -> String {
    let mut out = format!("<{PREVIOUS_SESSIONS}>\n");
    for (i, s) in sessions.iter().enumerate() {
        let name =
            s.name.as_deref().map_or_else(String::new, |n| format!(" session=\"{}\"", attr(n)));
        let open =
            format!("<{PREVIOUS_CONVERSATION} n=\"{}\"{name} id=\"{}\">", i + 1, attr(&s.id));
        let _ = writeln!(out, "{open}\n{}\n</{PREVIOUS_CONVERSATION}>", body(i, s));
    }
    let _ = write!(out, "</{PREVIOUS_SESSIONS}>");
    out
}

/// The envelope the webui's `followupPrompt` builds from the same sessions: a
/// lone session inline while it fits [`INLINE_MAX_BYTES`]; else, and always
/// for a chain, the whole transcript in the file and pointers plus the
/// sessions' tails inline.
#[must_use]
pub fn followup_envelope(sessions: &[BriefSession]) -> FollowupEnvelope {
    if let [only] = sessions {
        let name = attr(only.name.as_deref().unwrap_or_default());
        let open =
            format!("<{PREVIOUS_CONVERSATION} session=\"{name}\" id=\"{}\">", attr(&only.id));
        let close = format!("</{PREVIOUS_CONVERSATION}>");
        let whole = format!("{open}\n{}\n{close}", only.markdown);
        if only.markdown.len() <= INLINE_MAX_BYTES {
            return FollowupEnvelope { inline: whole, file: None };
        }
        let body = pointed(READ_POINTER, only.tail.as_deref());
        let inline = format!("{open}\n{body}\n{close}");
        return FollowupEnvelope { inline, file: Some(format!("{whole}\n")) };
    }
    let last = sessions.len().saturating_sub(1);
    let inline = chain_envelope(sessions, |i, s| {
        pointed(if i == last { READ_POINTER } else { FILE_POINTER }, s.tail.as_deref())
    });
    let file = format!("{}\n", chain_envelope(sessions, |_, s| s.markdown.clone()));
    FollowupEnvelope { inline, file: Some(file) }
}

/// `?chain=1` ignores both caps: only the chain ceiling applies.
#[derive(Debug, Default, Deserialize)]
pub struct BriefQuery {
    pub max_turns: Option<usize>,
    pub max_bytes: Option<usize>,
    pub chain: Option<String>,
}

impl BriefQuery {
    fn chain(&self) -> bool {
        matches!(self.chain.as_deref(), Some("1" | "true"))
    }

    fn caps(&self) -> Caps {
        let d = Caps::default();
        Caps {
            max_turns: self.max_turns.unwrap_or(d.max_turns).clamp(1, 10_000),
            max_bytes: self.max_bytes.unwrap_or(d.max_bytes).clamp(256, 4 * 1024 * 1024),
        }
    }
}

/// The chain ceiling from [`CHAIN_MAX_BYTES_ENV`], resolved once.
pub fn chain_max_bytes() -> usize {
    static MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| parse_chain_max_bytes(std::env::var(CHAIN_MAX_BYTES_ENV).ok().as_deref()))
}

fn parse_chain_max_bytes(raw: Option<&str>) -> usize {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_CHAIN_MAX_BYTES;
    };
    match raw.parse::<usize>() {
        Ok(v) if v >= INLINE_MAX_BYTES => v,
        _ => {
            tracing::warn!(
                "{CHAIN_MAX_BYTES_ENV} must be a byte count of at least {INLINE_MAX_BYTES}, got \
                 `{raw}`; using {DEFAULT_CHAIN_MAX_BYTES}"
            );
            DEFAULT_CHAIN_MAX_BYTES
        }
    }
}

/// `adapter_id, session_name, working_dir`.
type SessionRow = (Option<String>, Option<String>, String);

pub struct Transcript {
    pub name: String,
    pub turns: Vec<Turn>,
}

/// `None` when the session does not exist. `name` falls back to the working dir.
pub async fn load(pool: &PgPool, session_id: &str) -> sqlx::Result<Option<Transcript>> {
    let session: Option<SessionRow> =
        sqlx::query_as("SELECT adapter_id, session_name, working_dir FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    let Some((adapter_id, name, working_dir)) = session else { return Ok(None) };
    let adapter_id = adapter_id.unwrap_or_else(|| "claude-code".to_owned());
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM stream_events \
         WHERE session_id = $1 AND event_type IN ('message', 'tool_use') ORDER BY id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let turns = collect_turns(&adapter_id, rows.iter().map(|(t, p)| (t.as_str(), p.clone())));
    let name = name.filter(|n| !n.trim().is_empty()).unwrap_or(working_dir);
    Ok(Some(Transcript { name, turns }))
}

type Failure = (StatusCode, Json<ApiError>);

fn failure(status: StatusCode, error: &str) -> Failure {
    (status, Json(ApiError { error: error.into() }))
}

fn db_failure(e: &sqlx::Error) -> Failure {
    tracing::error!("db error: {e}");
    failure(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

/// Whose read access decides which ancestors a chain brief may include.
#[derive(Clone, Copy)]
pub enum Reader<'a> {
    Caller(&'a AuthContext),
    /// A server-side spawn: the owner of the session it continues.
    Owner(Option<uuid::Uuid>),
}

async fn readable(reader: Reader<'_>, id: &str, pool: &PgPool) -> sqlx::Result<bool> {
    match reader {
        Reader::Caller(ctx) => match crate::authz::authorize_session_read(ctx, id, pool).await {
            Ok(()) => Ok(true),
            Err(StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => Ok(false),
            Err(status) => Err(sqlx::Error::Protocol(format!("session authz failed: {status}"))),
        },
        Reader::Owner(None) => Ok(false),
        Reader::Owner(owner) => Ok(crate::authz::session_owner(id, pool).await? == owner),
    }
}

/// The follow-up chain ending at `id`, root first; empty when `id` is unknown.
/// `id` itself is not checked: the caller already may read it.
pub async fn load_chain(pool: &PgPool, reader: Reader<'_>, id: &str) -> sqlx::Result<Vec<Link>> {
    let ids = crate::followup::chain_of(pool, id).await?;
    let mut links = Vec::with_capacity(ids.len());
    for sid in ids {
        let transcript = if sid == id || readable(reader, &sid, pool).await? {
            load(pool, &sid).await?
        } else {
            None
        };
        links.push(Link { id: sid, transcript });
    }
    Ok(links)
}

pub async fn session_brief(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Query(q): Query<BriefQuery>,
) -> Result<Json<BriefResponse>, Failure> {
    if q.chain() {
        let links = load_chain(&state.pool, Reader::Caller(&ctx), &session_id)
            .await
            .map_err(|e| db_failure(&e))?;
        if links.is_empty() {
            return Err(failure(StatusCode::NOT_FOUND, "session not found"));
        }
        return Ok(Json(assemble(&links, chain_max_bytes())));
    }
    let transcript = load(&state.pool, &session_id).await.map_err(|e| db_failure(&e))?;
    let Some(transcript) = transcript else {
        return Err(failure(StatusCode::NOT_FOUND, "session not found"));
    };
    Ok(Json(render(&transcript.turns, q.caps())))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use cctui_proto::api::BriefResponse;

    use super::{
        BRIEF_FILE_NAME, BriefQuery, Caps, DEFAULT_CHAIN_MAX_BYTES, INLINE_MAX_BYTES, Link, Role,
        Transcript, Turn, assemble, collect_turns, followup_envelope, parse_chain_max_bytes,
        render,
    };

    fn user(text: &str) -> (&'static str, serde_json::Value) {
        ("message", json!({"role": "user", "text": text}))
    }

    fn assistant(text: &str) -> (&'static str, serde_json::Value) {
        ("message", json!({"role": "assistant", "text": text, "message_id": "m1"}))
    }

    #[test]
    fn keeps_user_and_assistant_prose_only() {
        let rows = vec![
            user("hello"),
            ("message", json!({"role": "assistant_thinking", "text": "hmm"})),
            ("tool_use", json!({"tool": "Bash", "input": {"command": "ls"}})),
            ("tool_use", json!({"kind": "tool_result", "content": "a b c"})),
            ("message", json!({"role": "system_marker", "text": "compacted"})),
            (
                "message",
                json!({"role": "user", "text": "<task-notification>x</task-notification>", "meta": true}),
            ),
            assistant("hi there"),
        ];
        let turns = collect_turns("claude-code", rows);
        assert_eq!(
            turns,
            vec![
                Turn { role: Role::User, text: "hello".into() },
                Turn { role: Role::Assistant, text: "hi there".into() },
            ]
        );
    }

    #[test]
    fn keepalive_ticks_are_dropped() {
        let prompt = crate::keepalive::tick_prompt(chrono::Utc::now());
        let mut tick = json!({"role": "user", "text": prompt});
        assert!(crate::keepalive::stamp_tick(&mut tick));
        let turns = collect_turns(
            "claude-code",
            vec![user("q"), assistant("a"), ("message", tick), assistant("warm"), user("next")],
        );
        assert_eq!(
            turns,
            vec![
                Turn { role: Role::User, text: "q".into() },
                Turn { role: Role::Assistant, text: "a".into() },
                Turn { role: Role::User, text: "next".into() },
            ]
        );
    }

    #[test]
    fn an_inbox_batch_keeps_its_event_lines_without_the_wrapper() {
        let events = "1. (n8n, 2026-11-16T09:02Z) ticket moved\n\
                      2. (approval, 2026-11-16T09:07Z) approved: ship it\nsecond line";
        let batch = format!("[cctui inbox: 2 events]\n{events}");
        let turns = collect_turns(
            "claude-code",
            vec![user(&batch), assistant("on it"), user("[cctui inbox: 0 events]")],
        );
        assert_eq!(
            turns,
            vec![
                Turn { role: Role::User, text: events.into() },
                Turn { role: Role::Assistant, text: "on it".into() },
            ]
        );
        let md = render(&turns, Caps::default()).markdown;
        assert!(!md.contains("[cctui inbox"), "{md}");
        let typed = collect_turns("claude-code", vec![user("[cctui inbox: what is this")]);
        assert_eq!(typed[0].text, "[cctui inbox: what is this");
    }

    #[test]
    fn consecutive_chunks_of_one_speaker_fold_into_one_turn() {
        let turns =
            collect_turns("claude-code", vec![user("q"), assistant("part 1"), assistant("part 2")]);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].text, "part 1\n\npart 2");
    }

    #[test]
    fn images_stay_as_their_reference() {
        let rows = vec![
            user("[Image: source: shot.png]\nwhat is this"),
            (
                "message",
                json!({"role": "assistant_attachment", "text": "[image attachment]", "message_id": "m"}),
            ),
            assistant("a chart"),
        ];
        let turns = collect_turns("claude-code", rows);
        assert_eq!(turns[0].text, "[Image: source: shot.png]\nwhat is this");
        assert_eq!(turns[1].text, "[image attachment]\n\na chart");
    }

    #[test]
    fn codex_turns_map_too() {
        let rows = vec![
            ("message", json!({"type": "userMessage", "content": [{"type": "text", "text": "q"}]})),
            ("message", json!({"type": "agentMessage", "text": "a"})),
        ];
        let turns = collect_turns("codex", rows);
        assert!(turns.iter().any(|t| t.role == Role::Assistant), "{turns:?}");
    }

    #[test]
    fn a_spawn_turn_loses_its_session_context_under_either_name() {
        for (open, close) in [
            ("<cctui-session-context>", "</cctui-session-context>"),
            ("<session-context>", "</session-context>"),
        ] {
            let first = format!("{open}\nsession: a\ncwd: /w\n{close}\n\nfix the parser");
            let turns = collect_turns("claude-code", vec![user(&first), assistant("done")]);
            assert_eq!(turns[0].text, "fix the parser", "{open}");
            let md = render(&turns, Caps::default()).markdown;
            assert!(!md.contains("session-context"), "{md}");
            assert!(!md.contains("cwd: /w"), "{md}");
        }
    }

    #[test]
    fn a_previous_conversation_collapses_to_one_line_under_either_name() {
        let cases = [("cctui-previous-conversation", ""), ("previous-conversation", "===\n\n")];
        for (tag, sep) in cases {
            let first = format!(
                "<cctui-session-context>\nx\n</cctui-session-context>\n\n\
                 <{tag} session=\"wave &quot;3&quot;\" id=\"s-a\">\n**User:**\n\nold ask\n\
                 </{tag}>\n\n{sep}Now add tests."
            );
            let turns = collect_turns("claude-code", vec![user(&first)]);
            assert_eq!(
                turns[0].text, "(continued from session \"wave \"3\"\" s-a)\n\nNow add tests.",
                "{tag}"
            );
        }
    }

    #[test]
    fn a_follow_up_of_a_follow_up_brief_carries_no_nested_wrapper() {
        let first = "<session-context>\nb ctx\n</session-context>\n\n\
                     <previous-conversation session=\"b\" id=\"s-b\">\n**User:**\n\n\
                     <session-context>\na ctx\n</session-context>\n\n\
                     <previous-conversation session=\"a\" id=\"s-a\">\nolder\n\
                     </previous-conversation>\n\n===\n\nfirst ask\n</previous-conversation>\n\n\
                     ===\n\nsecond ask";
        let turns = collect_turns("claude-code", vec![user(first), assistant("ok")]);
        let md = render(&turns, Caps::default()).markdown;
        assert!(!md.contains("previous-conversation"), "{md}");
        assert!(!md.contains("session-context"), "{md}");
        assert_eq!(md.matches("(continued from session").count(), 1, "{md}");
        assert!(md.contains("(continued from session \"b\" s-b)\n\nsecond ask"), "{md}");
    }

    #[test]
    fn a_turn_of_nothing_but_wrappers_is_dropped() {
        let turns = collect_turns(
            "claude-code",
            vec![user("<cctui-session-context>\nx\n</cctui-session-context>"), assistant("hi")],
        );
        assert_eq!(turns, vec![Turn { role: Role::Assistant, text: "hi".into() }]);
    }

    #[test]
    fn a_wrapper_quoted_mid_message_is_kept() {
        let quoted = "why is this here:\n<cctui-session-context>\nx\n</cctui-session-context>";
        let turns = collect_turns("claude-code", vec![user(quoted)]);
        assert_eq!(turns[0].text, quoted);
    }

    #[test]
    fn a_dictated_turn_keeps_the_fact_without_its_wrapper() {
        let dictated = cctui_proto::injected::dictated_prompt("fix the login bug");
        let turns = collect_turns("claude-code", vec![user(&dictated), assistant("on it")]);
        assert_eq!(turns.len(), 2, "{turns:?}");
        let text = &turns[0].text;
        assert!(text.starts_with("(dictated: may hold speech-recognition errors)\n\n"), "{text}");
        assert!(text.contains("fix the login bug"), "{text}");
        assert!(!text.contains("session-context"), "{text}");
    }

    #[test]
    fn renders_markdown_headings() {
        let turns = collect_turns("claude-code", vec![user("q"), assistant("a")]);
        let out = render(&turns, Caps::default());
        assert_eq!(out.markdown, "**User:**\n\nq\n\n**Assistant:**\n\na");
        assert_eq!((out.turns, out.omitted, out.truncated), (2, 0, false));
    }

    #[test]
    fn turn_cap_elides_the_oldest_turns() {
        let rows: Vec<_> = (0..10)
            .flat_map(|i| {
                [("message", json!({"role": "user", "text": format!("q{i}")})), assistant("a")]
            })
            .collect();
        let turns = collect_turns("claude-code", rows);
        let out = render(&turns, Caps { max_turns: 4, max_bytes: 1 << 20 });
        assert!(out.markdown.starts_with("… 16 earlier turns omitted\n\n**User:**\n\nq8"));
        assert_eq!((out.turns, out.omitted, out.truncated), (4, 16, false));
    }

    #[test]
    fn byte_cap_elides_oldest_turns_and_flags_truncation() {
        let turns: Vec<Turn> =
            (0..5).map(|i| Turn { role: Role::User, text: format!("{i}").repeat(100) }).collect();
        let out = render(&turns, Caps { max_turns: 60, max_bytes: 250 });
        assert_eq!(out.omitted, 3);
        assert_eq!(out.turns, 2);
        assert!(out.truncated);
        assert!(out.markdown.starts_with("… 3 earlier turns omitted"));
        assert!(out.markdown.contains(&"3".repeat(100)) && out.markdown.contains(&"4".repeat(100)));
        assert!(!out.markdown.contains(&"2".repeat(100)));
    }

    #[test]
    fn a_single_oversized_turn_keeps_its_tail() {
        let turns = vec![Turn {
            role: Role::Assistant,
            text: format!("{}é{}", "x".repeat(300), "y".repeat(100)),
        }];
        let out = render(&turns, Caps { max_turns: 60, max_bytes: 128 });
        assert!(out.truncated);
        assert_eq!(out.omitted, 0);
        assert!(out.markdown.len() <= 128 + "…".len());
        assert!(out.markdown.ends_with(&"y".repeat(100)));
        assert!(out.markdown.contains("**Assistant:**\n\n…"));
    }

    fn link(id: &str, turns: &[(Role, &str)]) -> Link {
        let turns = turns.iter().map(|&(role, text)| Turn { role, text: text.into() }).collect();
        Link { id: id.into(), transcript: Some(Transcript { name: format!("n-{id}"), turns }) }
    }

    fn hidden(id: &str) -> Link {
        Link { id: id.into(), transcript: None }
    }

    #[test]
    fn a_chain_container_collapses_to_one_line_per_session() {
        let first = "<cctui-session-context>\nx\n</cctui-session-context>\n\n\
                     <cctui-previous-sessions>\n\
                     <cctui-previous-conversation n=\"1\" session=\"a\" id=\"s-a\">\nold\n\
                     </cctui-previous-conversation>\n\
                     <cctui-previous-conversation n=\"2\" id=\"s-b\">\nhidden\n\
                     </cctui-previous-conversation>\n\
                     </cctui-previous-sessions>\n\nContinue from this.";
        let turns = collect_turns("claude-code", vec![user(first)]);
        assert_eq!(
            turns[0].text,
            "(continued from session \"a\" s-a)\n(continued from session \"\" s-b)\n\n\
             Continue from this."
        );
        let quoted = "see <cctui-previous-sessions>\nx\n</cctui-previous-sessions>";
        assert_eq!(collect_turns("claude-code", vec![user(quoted)])[0].text, quoted);
    }

    #[test]
    fn a_chain_of_three_carries_each_transcript_once_root_first() {
        let links = [
            link("s-a", &[(Role::User, "ask A"), (Role::Assistant, "answer A")]),
            link(
                "s-b",
                &[
                    (Role::User, "(continued from session \"n-s-a\" s-a)\n\nask B"),
                    (Role::Assistant, "answer B"),
                ],
            ),
            link(
                "s-c",
                &[
                    (
                        Role::User,
                        "(continued from session \"n-s-a\" s-a)\n\
                         (continued from session \"n-s-b\" s-b)\n\nask C",
                    ),
                    (Role::Assistant, "answer C"),
                ],
            ),
        ];
        let out = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        let sessions = out.sessions.as_deref().expect("sessions");
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["s-a", "s-b", "s-c"]);
        assert_eq!(sessions[0].markdown, "**User:**\n\nask A\n\n**Assistant:**\n\nanswer A");
        assert_eq!(sessions[1].markdown, "**User:**\n\nask B\n\n**Assistant:**\n\nanswer B");
        assert_eq!(sessions[2].name.as_deref(), Some("n-s-c"));
        assert!(!out.markdown.contains("(continued from"), "{}", out.markdown);
        for needle in ["ask A", "answer A", "ask B", "answer B", "ask C", "answer C"] {
            assert_eq!(out.markdown.matches(needle).count(), 1, "{needle}: {}", out.markdown);
        }
        let a = out.markdown.find("ask A").unwrap();
        let b = out.markdown.find("ask B").unwrap();
        assert!(a < b && b < out.markdown.find("ask C").unwrap());
        assert!(out.markdown.starts_with("### Session 1 \"n-s-a\" (s-a)\n\n**User:**"));
        assert_eq!((out.turns, out.omitted, out.truncated), (6, 0, false));
    }

    #[test]
    fn an_unreadable_ancestor_is_one_line_and_its_mention_stays() {
        let links = [
            hidden("s-x"),
            link("s-y", &[(Role::User, "(continued from session \"x\" s-x)\n\nask Y")]),
        ];
        let out = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        let sessions = out.sessions.as_deref().expect("sessions");
        assert_eq!(sessions[0].name, None);
        assert_eq!(sessions[0].markdown, "(session s-x could not be included)");
        assert_eq!(
            sessions[1].markdown,
            "**User:**\n\n(continued from session \"x\" s-x)\n\nask Y"
        );
        assert!(out.markdown.starts_with("### Session 1 (s-x)\n\n(session s-x could not"));
    }

    fn alternating(id: &str, prefix: &str, n: usize) -> Link {
        let turns = (0..n)
            .map(|i| Turn {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                text: format!("<{prefix}-{i:03}>"),
            })
            .collect();
        Link { id: id.into(), transcript: Some(Transcript { name: format!("n-{id}"), turns }) }
    }

    #[test]
    fn a_chain_carries_every_turn_of_every_session_once_in_order() {
        let links = [
            alternating("s-a", "A", 200),
            alternating("s-b", "B", 150),
            alternating("s-c", "C", 12),
        ];
        let out = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        assert!(!out.truncated);
        assert_eq!((out.turns, out.omitted), (362, 0));
        let mut at = 0;
        for (prefix, n) in [("A", 200), ("B", 150), ("C", 12)] {
            for i in 0..n {
                let needle = format!("<{prefix}-{i:03}>");
                assert_eq!(out.markdown.matches(&needle).count(), 1, "{needle}");
                let found = out.markdown.find(&needle).unwrap();
                assert!(found > at, "{needle} out of order");
                at = found;
            }
        }
        assert!(out.markdown.contains("**User:**\n\n<A-000>"));
        assert!(out.markdown.contains("**User:**\n\n<A-100>"));
        let file = followup_envelope(out.sessions.as_deref().unwrap()).file.expect("file");
        for needle in ["<A-000>", "<A-100>", "<A-199>", "<B-149>", "<C-011>"] {
            assert_eq!(file.matches(needle).count(), 1, "{needle}");
        }
    }

    #[test]
    fn a_root_ask_over_8_kib_arrives_whole() {
        let ask = format!("start{}end", "q".repeat(20 * 1024));
        let links = [
            link("s-a", &[(Role::User, ask.as_str()), (Role::Assistant, "ok")]),
            link("s-b", &[(Role::User, "next")]),
        ];
        let out = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        let sessions = out.sessions.as_deref().expect("sessions");
        assert!(sessions[0].markdown.starts_with(&format!("**User:**\n\n{ask}\n\n")));
        let file = followup_envelope(sessions).file.expect("file");
        assert!(file.contains(&ask));
    }

    fn sized(id: &str, ask: &str, n: usize) -> Link {
        let body = "z".repeat(1024);
        let mut turns = vec![Turn { role: Role::User, text: ask.to_owned() }];
        turns.extend((0..n).map(|i| Turn {
            role: if i % 2 == 0 { Role::Assistant } else { Role::User },
            text: format!("[{id} {i:02}] {body}"),
        }));
        Link { id: id.into(), transcript: Some(Transcript { name: id.into(), turns }) }
    }

    #[test]
    fn only_past_the_ceiling_are_turns_elided_keeping_the_root_ask_and_the_newest_session() {
        let root_ask = format!("root ask {}", "r".repeat(10 * 1024));
        let links: Vec<Link> = (0..5)
            .map(|i| sized(&format!("s{i}"), if i == 0 { root_ask.as_str() } else { "ask" }, 40))
            .collect();
        let under = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        assert!(!under.truncated && !under.markdown.contains("turns elided"));

        let ceiling = 64 * 1024;
        let out = assemble(&links, ceiling);
        assert!(out.markdown.len() <= ceiling, "{} > {ceiling}", out.markdown.len());
        assert!(out.truncated);
        assert!(out.markdown.contains("turns elided here: the follow-up chain is over its 64 KiB"));
        let sessions = out.sessions.as_deref().expect("sessions");
        assert!(sessions[0].markdown.starts_with(&format!("**User:**\n\n{root_ask}\n\n")));
        assert!(sessions[0].omitted > 0);
        let newest = &sessions[4];
        assert_eq!((newest.omitted, newest.truncated), (0, false));
        assert!(!newest.markdown.contains("turns elided"));
        for i in 0..40 {
            assert!(newest.markdown.contains(&format!("[s4 {i:02}]")), "s4 turn {i}");
        }
        let cut = &sessions[0].markdown;
        let first = cut.find("[s0 00]");
        let last = cut.find("[s0 39]");
        let mark = cut.find("turns elided").expect("marker in the root");
        assert!(first.is_none_or(|f| f < mark) && last.is_none_or(|l| l > mark), "{cut}");
    }

    #[test]
    fn the_inline_tail_is_the_newest_turns_within_the_inline_budget() {
        let links: Vec<Link> = (0..3).map(|i| sized(&format!("s{i}"), "ask", 120)).collect();
        let out = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        let sessions = out.sessions.as_deref().expect("sessions");
        let envelope = followup_envelope(sessions);
        assert!(envelope.inline.len() <= INLINE_MAX_BYTES, "{}", envelope.inline.len());
        let tail = sessions[2].tail.as_deref().expect("newest tail");
        assert!(tail.starts_with("… ") && tail.contains("earlier turns are in the file"));
        assert!(tail.ends_with(&format!("[s2 119] {}", "z".repeat(1024))));
        assert!(sessions[0].tail.is_none());
        assert!(envelope.inline.contains(BRIEF_FILE_NAME));
        assert!(envelope.inline.contains("Read all of it"));
        assert!(envelope.file.expect("file").contains("[s0 00]"));
    }

    /// Shared with `webui/src/lib/followup.test.ts`: both sides build this.
    #[test]
    fn the_envelope_is_the_one_the_webui_builds() {
        let links = [
            link("s-a", &[(Role::User, "ask A"), (Role::Assistant, "answer A")]),
            hidden("s-x"),
            Link {
                id: "s-c".into(),
                transcript: Some(Transcript {
                    name: "c \"x\"".into(),
                    turns: vec![Turn { role: Role::User, text: "ask C".into() }],
                }),
            },
        ];
        let out = assemble(&links, DEFAULT_CHAIN_MAX_BYTES);
        let envelope = followup_envelope(out.sessions.as_deref().unwrap());
        assert_eq!(
            envelope.inline,
            "<cctui-previous-sessions>\n\
             <cctui-previous-conversation n=\"1\" session=\"n-s-a\" id=\"s-a\">\n\
             In the attached file previous-conversation.md.\n\n\
             **User:**\n\nask A\n\n**Assistant:**\n\nanswer A\n\
             </cctui-previous-conversation>\n\
             <cctui-previous-conversation n=\"2\" id=\"s-x\">\n\
             In the attached file previous-conversation.md.\n\
             </cctui-previous-conversation>\n\
             <cctui-previous-conversation n=\"3\" session=\"c &quot;x&quot;\" id=\"s-c\">\n\
             The whole conversation is in the attached file previous-conversation.md. Read all \
             of it before you continue: it can be longer than one Read call returns, so keep \
             reading until the end of the file.\n\n\
             **User:**\n\nask C\n\
             </cctui-previous-conversation>\n\
             </cctui-previous-sessions>"
        );
        assert_eq!(
            envelope.file.as_deref(),
            Some(
                "<cctui-previous-sessions>\n\
                 <cctui-previous-conversation n=\"1\" session=\"n-s-a\" id=\"s-a\">\n\
                 **User:**\n\nask A\n\n**Assistant:**\n\nanswer A\n\
                 </cctui-previous-conversation>\n\
                 <cctui-previous-conversation n=\"2\" id=\"s-x\">\n\
                 (session s-x could not be included)\n\
                 </cctui-previous-conversation>\n\
                 <cctui-previous-conversation n=\"3\" session=\"c &quot;x&quot;\" id=\"s-c\">\n\
                 **User:**\n\nask C\n\
                 </cctui-previous-conversation>\n\
                 </cctui-previous-sessions>\n"
            )
        );
    }

    #[test]
    fn a_lone_session_stays_inline_until_it_outgrows_the_inline_budget() {
        let small = assemble(&[link("s-a", &[(Role::User, "q")])], DEFAULT_CHAIN_MAX_BYTES);
        let envelope = followup_envelope(small.sessions.as_deref().unwrap());
        assert_eq!(
            envelope.inline,
            "<cctui-previous-conversation session=\"n-s-a\" id=\"s-a\">\n**User:**\n\nq\n\
             </cctui-previous-conversation>"
        );
        assert_eq!(envelope.file, None);
        let big = assemble(&[sized("s-a", "ask", 80)], DEFAULT_CHAIN_MAX_BYTES);
        let envelope = followup_envelope(big.sessions.as_deref().unwrap());
        assert!(envelope.inline.len() <= INLINE_MAX_BYTES);
        assert!(envelope.inline.contains("Read all of it") && envelope.inline.contains("[s-a 79]"));
        let file = envelope.file.expect("file");
        assert!(file.contains("**User:**\n\nask") && file.contains("[s-a 00]"));
    }

    #[test]
    fn an_old_client_chain_request_still_gets_a_valid_brief() {
        let old: BriefQuery =
            serde_json::from_value(json!({"chain": "1", "max_bytes": 512, "max_turns": 4}))
                .unwrap();
        assert!(old.chain());
        let out = assemble(&[alternating("s-a", "A", 200)], DEFAULT_CHAIN_MAX_BYTES);
        assert_eq!((out.turns, out.omitted, out.truncated), (200, 0, false));
        let json = serde_json::to_value(&out).unwrap();
        let back: BriefResponse = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.markdown, out.markdown);
        for key in ["markdown", "turns", "omitted", "truncated", "sessions"] {
            assert!(json.get(key).is_some(), "{key}");
        }
        let session = &json["sessions"][0];
        for key in ["id", "name", "markdown", "turns", "omitted", "truncated"] {
            assert!(session.get(key).is_some(), "{key}");
        }
        let legacy = json!({"id": "s", "name": null, "markdown": "m", "turns": 1, "omitted": 0,
            "truncated": false});
        let parsed: cctui_proto::api::BriefSession = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.tail, None);
    }

    #[test]
    fn the_chain_ceiling_falls_back_on_anything_unusable() {
        assert_eq!(parse_chain_max_bytes(None), DEFAULT_CHAIN_MAX_BYTES);
        assert_eq!(parse_chain_max_bytes(Some(" ")), DEFAULT_CHAIN_MAX_BYTES);
        assert_eq!(parse_chain_max_bytes(Some("nope")), DEFAULT_CHAIN_MAX_BYTES);
        assert_eq!(parse_chain_max_bytes(Some("1024")), DEFAULT_CHAIN_MAX_BYTES);
        assert_eq!(parse_chain_max_bytes(Some(" 8388608 ")), 8 * 1024 * 1024);
    }

    #[test]
    fn an_old_style_brief_carries_no_sessions() {
        let turns = collect_turns("claude-code", vec![user("q"), assistant("a")]);
        assert!(render(&turns, Caps::default()).sessions.is_none());
    }
}
