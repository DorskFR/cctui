use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;

use cctui_proto::injected::{InjectedKind, split_injected};
use cctui_proto::thread::{
    FollowupReason, InjectedFlag, ThreadConversationPage, ThreadCursor, ThreadEntry,
    ThreadGeneration,
};

use crate::context::SessionsContext;
use crate::error::AppError;
use crate::routes::sessions::{
    ConversationOrder, ConversationQuery, client_events, renderable_rows,
};

pub use cctui_server_kernel::sessions_types::{Reader, parse_cursor};

pub const DEFAULT_PAGE: i64 = 200;
const MAX_PAGE: i64 = 10_000;
const FIRST_TURN_SCAN: i64 = 20;
const USER_PREFIX: &str = "▷ User:";

pub struct Member {
    pub session_id: String,
    pub generation: u32,
    pub adapter_id: String,
    pub name: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub metadata: Value,
    pub readable: bool,
}

/// `id, adapter_id, session_name, registered_at, metadata`, in chain order.
type MemberRow = (String, Option<String>, Option<String>, DateTime<Utc>, Option<Value>);

const LINE_SQL: &str = "SELECT s.id, s.adapter_id, s.session_name, s.registered_at, s.metadata \
     FROM unnest($1::text[]) WITH ORDINALITY AS c(id, n) JOIN sessions s ON s.id = c.id \
     ORDER BY c.n";

async fn readable(reader: Reader<'_>, id: &str, pool: &PgPool) -> Result<bool, AppError> {
    match reader {
        Reader::Caller(ctx) => match crate::authz::authorize_session_read(ctx, id, pool).await {
            Ok(()) => Ok(true),
            Err(StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => Ok(false),
            Err(status) => Err(AppError::new(status, "session authz failed")),
        },
        Reader::Owner(None) => Ok(false),
        Reader::Owner(owner) => Ok(crate::authz::session_owner(id, pool).await? == owner),
    }
}

/// Archived generations included; empty when `id` is unknown.
pub async fn line_of(pool: &PgPool, reader: Reader<'_>, id: &str) -> Result<Vec<Member>, AppError> {
    let ids = super::chain_of(pool, id).await?;
    let rows: Vec<MemberRow> = sqlx::query_as(LINE_SQL).bind(&ids).fetch_all(pool).await?;
    let mut line = Vec::with_capacity(rows.len());
    for (generation, row) in (1..).zip(rows) {
        let (session_id, adapter_id, name, registered_at, metadata) = row;
        let readable = session_id == id || readable(reader, &session_id, pool).await?;
        line.push(Member {
            generation,
            adapter_id: adapter_id.unwrap_or_else(|| "claude-code".to_owned()),
            name: name.filter(|_| readable),
            registered_at,
            metadata: metadata.filter(|_| readable).unwrap_or(Value::Null),
            readable,
            session_id,
        });
    }
    Ok(line)
}

const fn flag(kind: InjectedKind) -> InjectedFlag {
    match kind {
        InjectedKind::SessionContext => InjectedFlag::SessionContext,
        InjectedKind::PreviousConversation => InjectedFlag::PreviousSessions,
    }
}

fn marker(prev: &Member, generation: &Member) -> ThreadEntry {
    let meta = |key: &str| generation.metadata.get(key).and_then(Value::as_str);
    ThreadEntry::FollowupMarker {
        generation: generation.generation,
        from: prev.session_id.clone(),
        to: generation.session_id.clone(),
        at: generation.registered_at,
        reason: meta("followup_reason").and_then(FollowupReason::parse),
        instruction: meta("followup_instruction").map(str::to_owned),
    }
}

async fn first_turn_seq(pool: &PgPool, generation: &Member) -> Result<Option<i64>, AppError> {
    let q = ConversationQuery {
        limit: Some(FIRST_TURN_SCAN),
        order: ConversationOrder::Asc,
        ..Default::default()
    };
    let rows = renderable_rows(pool, &generation.session_id, &generation.adapter_id, &q).await?;
    Ok(rows
        .iter()
        .find(|(_, v, _, _)| matches!(crate::normalize::client_category(v).0, "user" | "context"))
        .map(|(seq, _, _, _)| *seq))
}

/// One stored event as entries. A first turn of generation 2+ splits into its
/// injected blocks, flagged, then the user's own words as a plain user turn.
fn event_entries(generation: &Member, mut event: Value, first_turn: bool) -> Vec<ThreadEntry> {
    let entry = |injected, event| ThreadEntry::Event {
        session_id: generation.session_id.clone(),
        generation: generation.generation,
        injected,
        event,
    };
    let content = if first_turn { event.get("content").and_then(Value::as_str) } else { None };
    let split = content
        .and_then(|content| content.strip_prefix(USER_PREFIX))
        .map(|body| {
            let (blocks, prose) = split_injected(body.trim_start());
            let blocks: Vec<(InjectedFlag, String)> =
                blocks.iter().map(|b| (flag(b.kind), b.body.to_owned())).collect();
            (blocks, prose.trim().to_owned())
        })
        .filter(|(blocks, _)| !blocks.is_empty());
    let Some((blocks, prose)) = split else { return vec![entry(None, event)] };
    let mut out: Vec<ThreadEntry> = blocks
        .into_iter()
        .map(|(kind, body)| {
            let mut injected = event.clone();
            injected["content"] = json!(body);
            injected["meta"] = json!(true);
            entry(Some(kind), injected)
        })
        .collect();
    if !prose.is_empty() {
        event["content"] = json!(format!("{USER_PREFIX} {prose}"));
        out.push(entry(None, event));
    }
    out
}

struct Walk {
    newest_first: Vec<ThreadEntry>,
    taken: i64,
    limit: i64,
    last: Option<ThreadCursor>,
}

impl Walk {
    fn push(&mut self, key: ThreadCursor, entries: impl IntoIterator<Item = ThreadEntry>) {
        let mut group: Vec<ThreadEntry> = entries.into_iter().collect();
        group.reverse();
        self.newest_first.extend(group);
        self.taken += 1;
        self.last = Some(key);
    }

    const fn full(&self) -> bool {
        self.taken >= self.limit
    }
}

/// A page of the line: the newest `limit` keys before `cursor`, oldest first,
/// and the cursor of the page before it.
pub async fn page<S: SessionsContext>(
    state: &S,
    line: &[Member],
    cursor: Option<ThreadCursor>,
    limit: i64,
) -> Result<(Vec<ThreadEntry>, Option<ThreadCursor>), AppError> {
    let mut walk =
        Walk { newest_first: Vec::new(), taken: 0, limit: limit.clamp(1, MAX_PAGE), last: None };
    let end = cursor.map_or(line.len(), |c| {
        line.len().min(usize::try_from(c.generation).unwrap_or(usize::MAX))
    });
    for (at, generation) in line[..end].iter().enumerate().rev() {
        let g = generation.generation;
        let bound = cursor.filter(|c| c.generation == g).map(|c| c.seq);
        let admits = |seq: i64| bound.is_none_or(|b| seq < b);
        if generation.readable {
            if admits(1) {
                collect_events(state, generation, bound, &mut walk).await?;
                if walk.full() {
                    break;
                }
            }
        } else if admits(ThreadCursor::PLACEHOLDER_SEQ) {
            let key = ThreadCursor { generation: g, seq: ThreadCursor::PLACEHOLDER_SEQ };
            let session_id = generation.session_id.clone();
            walk.push(key, [ThreadEntry::Unreadable { session_id, generation: g }]);
            if walk.full() {
                break;
            }
        }
        if at > 0 && admits(ThreadCursor::MARKER_SEQ) {
            let key = ThreadCursor { generation: g, seq: ThreadCursor::MARKER_SEQ };
            walk.push(key, [marker(&line[at - 1], generation)]);
            if walk.full() {
                break;
            }
        }
    }
    let next = if walk.full() { walk.last } else { None };
    let mut entries = walk.newest_first;
    entries.reverse();
    Ok((entries, next))
}

async fn collect_events<S: SessionsContext>(
    state: &S,
    generation: &Member,
    before: Option<i64>,
    walk: &mut Walk,
) -> Result<(), AppError> {
    let q = ConversationQuery {
        limit: Some(walk.limit - walk.taken),
        before,
        after: None,
        order: ConversationOrder::Desc,
    };
    let mut rows =
        renderable_rows(state.pool(), &generation.session_id, &generation.adapter_id, &q).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let first_turn = if generation.generation >= 2 {
        first_turn_seq(state.pool(), generation).await?
    } else {
        None
    };
    rows.reverse();
    let keys: Vec<i64> = rows.iter().map(|(seq, _, _, _)| *seq).collect();
    let events = client_events(state, &generation.session_id, rows).await?;
    for (seq, event) in keys.into_iter().zip(events).rev() {
        let entries = event_entries(generation, event, first_turn == Some(seq));
        walk.push(ThreadCursor { generation: generation.generation, seq }, entries);
    }
    Ok(())
}

pub async fn conversation<S: SessionsContext>(
    state: &S,
    reader: Reader<'_>,
    id: &str,
    cursor: Option<ThreadCursor>,
    limit: i64,
) -> Result<Option<ThreadConversationPage>, AppError> {
    let line = line_of(state.pool(), reader, id).await?;
    let Some(root) = line.first() else { return Ok(None) };
    let (entries, next) = page(state, &line, cursor, limit).await?;
    let thread_root_id =
        super::thread_root_of(state.pool(), id).await?.unwrap_or_else(|| root.session_id.clone());
    let (next_session_id, _) = super::followups_of(state.pool(), id).await?;
    let generations = line
        .iter()
        .map(|g| ThreadGeneration {
            session_id: g.session_id.clone(),
            generation: g.generation,
            readable: g.readable,
            name: g.name.clone(),
            registered_at: g.registered_at,
        })
        .collect();
    Ok(Some(ThreadConversationPage {
        thread_root_id,
        head: id.to_owned(),
        next_session_id,
        generations,
        entries,
        next_cursor: next.as_ref().map(ToString::to_string),
    }))
}
