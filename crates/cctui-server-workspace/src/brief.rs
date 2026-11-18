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
//! `?chain=1` renders every session of the follow-up line ending at `id`, root
//! first, under one byte cap: the root's first ask always stays, the newest
//! sessions are filled first, and an older one that no longer fits shrinks to
//! its first ask and last answer, then to one line.

use std::collections::HashSet;
use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use cctui_proto::api::{ApiError, BriefResponse, BriefSession};
use cctui_proto::injected::{InjectedBlock, InjectedKind, PREVIOUS_SESSIONS, split_injected};

use crate::auth::AuthContext;
use crate::state::AppState;

pub const DEFAULT_MAX_TURNS: usize = 60;
pub const DEFAULT_MAX_BYTES: usize = 48 * 1024;
const USER_PREFIX: &str = "▷ User: ";
const CONTINUED_PREFIX: &str = "(continued from session \"";
const ROOT_ASK_MAX_BYTES: usize = 8 * 1024;
const MIN_TAIL_BYTES: usize = 4 * 1024;
const MIN_SKETCH_BYTES: usize = 512;
const MIN_SKETCH_SHARE: usize = 64;
const ELISION_SLACK: usize = 64;

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

fn cut_tail(text: &str, keep: usize) -> String {
    if text.len() <= keep {
        return text.to_owned();
    }
    let mut end = keep;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
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

/// The first ask and the last answer of `turns`, clipped to share `budget`.
fn sketch(turns: &[Turn], budget: usize) -> Option<BriefResponse> {
    let first_user = turns.iter().position(|t| t.role == Role::User);
    let last_assistant = turns.iter().rposition(|t| t.role == Role::Assistant);
    let mut picked: Vec<usize> = [first_user, last_assistant].into_iter().flatten().collect();
    picked.sort_unstable();
    picked.dedup();
    let omitted = turns.len() - picked.len();
    let note = if omitted > 0 { format!("… {omitted} turns omitted\n\n") } else { String::new() };
    let fixed = note.len()
        + picked.iter().map(|&i| turns[i].role.heading().len() + 4 + "…".len()).sum::<usize>();
    let share = budget.saturating_sub(fixed) / picked.len().max(1);
    if picked.is_empty() || share < MIN_SKETCH_SHARE {
        return None;
    }
    let mut markdown = String::new();
    for (n, &i) in picked.iter().enumerate() {
        if n == 1 {
            markdown.push_str(&note);
        }
        let turn = &turns[i];
        let text = match turn.role {
            Role::User => cut_tail(&turn.text, share),
            Role::Assistant => cut_head(&turn.text, share),
        };
        markdown.push_str(turn.role.heading());
        markdown.push_str("\n\n");
        markdown.push_str(&text);
        markdown.push_str("\n\n");
    }
    if picked.len() == 1 {
        markdown.push_str(&note);
    }
    Some(BriefResponse {
        markdown: markdown.trim_end().to_owned(),
        turns: count(picked.len()),
        omitted: count(omitted),
        truncated: true,
        sessions: None,
    })
}

/// `turns` within `budget` bytes: the newest turns, else a sketch, else one line.
fn fit(turns: &[Turn], budget: usize, max_turns: usize) -> BriefResponse {
    if turns.is_empty() || budget >= MIN_TAIL_BYTES {
        let max_bytes = budget.saturating_sub(ELISION_SLACK);
        return render(turns, Caps { max_turns, max_bytes });
    }
    if budget >= MIN_SKETCH_BYTES
        && let Some(sketched) = sketch(turns, budget)
    {
        return sketched;
    }
    BriefResponse {
        markdown: format!("… {} turns omitted", turns.len()),
        turns: 0,
        omitted: count(turns.len()),
        truncated: true,
        sessions: None,
    }
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

/// The first user turn of `turns`, taken out and clipped.
fn take_ask(turns: &mut Vec<Turn>) -> Option<Turn> {
    let at = turns.iter().position(|t| t.role == Role::User)?;
    let mut ask = turns.remove(at);
    ask.text = cut_tail(&ask.text, ROOT_ASK_MAX_BYTES);
    Some(ask)
}

fn with_ask(ask: &Turn, part: &mut BriefSession) {
    let mut markdown = format!("{}\n\n{}", ask.role.heading(), ask.text);
    if !part.markdown.is_empty() {
        markdown.push_str("\n\n");
        markdown.push_str(&part.markdown);
    }
    part.markdown = markdown;
    part.turns = part.turns.saturating_add(1);
}

/// The chain `links` (root first) as one brief within `caps.max_bytes`.
pub fn assemble(links: &[Link], caps: Caps) -> BriefResponse {
    let included: HashSet<&str> =
        links.iter().filter(|l| l.transcript.is_some()).map(|l| l.id.as_str()).collect();
    let mut turns: Vec<Option<Vec<Turn>>> = links
        .iter()
        .map(|l| {
            l.transcript.as_ref().map(|t| {
                t.turns
                    .iter()
                    .filter_map(|turn| {
                        let text = without_continued(&turn.text, &included);
                        (!text.is_empty()).then_some(Turn { role: turn.role, text })
                    })
                    .collect()
            })
        })
        .collect();
    let ask = turns.first_mut().and_then(Option::as_mut).and_then(take_ask);
    let headings: Vec<String> =
        links.iter().enumerate().map(|(i, link)| heading(i + 1, link)).collect();
    let reserved = headings.iter().map(|h| h.len() + 4).sum::<usize>()
        + links.iter().map(|l| unavailable(&l.id).len()).sum::<usize>()
        + ask.as_ref().map_or(0, turn_len);
    let mut remaining = caps.max_bytes.saturating_sub(reserved);
    let mut parts: Vec<BriefSession> = Vec::with_capacity(links.len());
    for (link, kept) in links.iter().zip(&turns).rev() {
        let budget = remaining + unavailable(&link.id).len();
        let part = match (&link.transcript, kept) {
            (Some(t), Some(kept)) => {
                let out = fit(kept, budget, caps.max_turns);
                BriefSession {
                    id: link.id.clone(),
                    name: Some(t.name.clone()),
                    markdown: out.markdown,
                    turns: out.turns,
                    omitted: out.omitted,
                    truncated: out.truncated,
                }
            }
            _ => BriefSession {
                id: link.id.clone(),
                name: None,
                markdown: unavailable(&link.id),
                turns: 0,
                omitted: 0,
                truncated: false,
            },
        };
        remaining = budget.saturating_sub(part.markdown.len());
        parts.push(part);
    }
    parts.reverse();
    if let (Some(ask), Some(root)) = (&ask, parts.first_mut()) {
        with_ask(ask, root);
    }
    let markdown = parts
        .iter()
        .zip(&headings)
        .map(|(part, heading)| format!("{heading}\n\n{}", part.markdown))
        .collect::<Vec<_>>()
        .join("\n\n");
    BriefResponse {
        markdown,
        turns: parts.iter().fold(0, |n, p| n.saturating_add(p.turns)),
        omitted: parts.iter().fold(0, |n, p| n.saturating_add(p.omitted)),
        truncated: parts.iter().any(|p| p.truncated),
        sessions: Some(parts),
    }
}

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
        return Ok(Json(assemble(&links, q.caps())));
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

    use super::{Caps, Link, Role, Transcript, Turn, assemble, collect_turns, render};

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
        let out = assemble(&links, Caps::default());
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
        let out = assemble(&links, Caps::default());
        let sessions = out.sessions.as_deref().expect("sessions");
        assert_eq!(sessions[0].name, None);
        assert_eq!(sessions[0].markdown, "(session s-x could not be included)");
        assert_eq!(
            sessions[1].markdown,
            "**User:**\n\n(continued from session \"x\" s-x)\n\nask Y"
        );
        assert!(out.markdown.starts_with("### Session 1 (s-x)\n\n(session s-x could not"));
    }

    #[test]
    fn a_long_chain_over_the_cap_keeps_the_root_ask_and_the_parent_tail() {
        let body = "x".repeat(2048);
        let links: Vec<Link> = (0..20)
            .map(|i| {
                let id = format!("s-{i:02}");
                let mut turns: Vec<(Role, String)> = vec![(Role::User, format!("ask {i}"))];
                for t in 0..10 {
                    turns.push((Role::Assistant, format!("{body} s{i} t{t}")));
                    turns.push((Role::User, format!("{body} s{i} u{t}")));
                }
                turns.push((Role::Assistant, format!("last of {i}")));
                let turns = turns.into_iter().map(|(role, text)| Turn { role, text }).collect();
                Link { id, transcript: Some(Transcript { name: format!("n{i}"), turns }) }
            })
            .collect();
        let cap = 32 * 1024;
        let out = assemble(&links, Caps { max_turns: 60, max_bytes: cap });
        assert!(out.markdown.len() <= cap, "{} > {cap}", out.markdown.len());
        assert!(out.truncated);
        let sessions = out.sessions.as_deref().expect("sessions");
        assert_eq!(sessions.len(), 20);
        assert!(sessions[0].markdown.starts_with("**User:**\n\nask 0"), "{}", sessions[0].markdown);
        assert!(sessions[19].markdown.ends_with("last of 19"), "{}", sessions[19].markdown);
        assert!(sessions[19].markdown.contains("s19 u9"));
        assert!(sessions.iter().any(|s| s.markdown.contains("turns omitted")));
        assert!(sessions[1..19].iter().all(|s| s.omitted > 0));
    }

    #[test]
    fn an_old_style_brief_carries_no_sessions() {
        let turns = collect_turns("claude-code", vec![user("q"), assistant("a")]);
        assert!(render(&turns, Caps::default()).sessions.is_none());
    }
}
