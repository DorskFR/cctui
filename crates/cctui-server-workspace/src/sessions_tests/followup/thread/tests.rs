use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Path, Query, State};
use cctui_proto::thread::{ThreadConversationPage, ThreadCursor, ThreadEntry};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Reader, conversation};
use crate::auth::{AuthContext, Scope};
use crate::routes::sessions::{
    ConversationQuery, InlineImagesQuery, ThreadConversationQuery, get_conversation,
    renderable_rows,
};
use crate::state::AppState;

struct Fixture {
    pool: sqlx::PgPool,
    state: AppState,
    owner: Uuid,
    machine: Uuid,
    stranger: Uuid,
    stranger_machine: Uuid,
}

async fn fixture(name: &str) -> Option<Fixture> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (owner, machine, stranger, stranger_machine) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    for (uid, mid) in [(owner, machine), (stranger, stranger_machine)] {
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("line-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed machine");
    }
    let state = AppState::for_test(pool.clone());
    Some(Fixture { pool, state, owner, machine, stranger, stranger_machine })
}

impl Fixture {
    async fn session(
        &self,
        stranger: bool,
        parent: Option<&str>,
        status: &str,
        age_mins: i32,
    ) -> String {
        let (uid, mid) = if stranger {
            (self.stranger, self.stranger_machine)
        } else {
            (self.owner, self.machine)
        };
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, parent_id, machine_id, machine_uuid, user_id, working_dir, \
               status, adapter_id, session_name, metadata, registered_at) \
             VALUES ($1, $2, $3, $4, $5, '/w', $6, 'claude-code', $7, \
               CASE WHEN $2::text IS NULL THEN '{}'::jsonb \
                    ELSE '{\"relation\": \"followup\"}'::jsonb END, \
               now() - make_interval(mins => $8))",
        )
        .bind(&id)
        .bind(parent)
        .bind(mid.to_string())
        .bind(mid)
        .bind(uid)
        .bind(status)
        .bind(format!("name-{age_mins}"))
        .bind(age_mins)
        .execute(&self.pool)
        .await
        .expect("seed session");
        id
    }

    async fn events(&self, sid: &str, rows: &[(&str, Value)]) {
        for (event_type, payload) in rows {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
            )
            .bind(sid)
            .bind(event_type)
            .bind(payload)
            .execute(&self.pool)
            .await
            .expect("seed event");
        }
    }

    async fn cleanup(&self) {
        for uid in [self.owner, self.stranger] {
            for sql in [
                "DELETE FROM stream_events WHERE session_id IN \
                 (SELECT id FROM sessions WHERE user_id = $1)",
                "DELETE FROM sessions WHERE user_id = $1",
                "DELETE FROM machines WHERE user_id = $1",
                "DELETE FROM users WHERE id = $1",
            ] {
                sqlx::query(sql).bind(uid).execute(&self.pool).await.ok();
            }
        }
    }

    fn ctx(&self) -> AuthContext {
        AuthContext {
            user_id: self.owner,
            key_id: self.owner,
            machine_id: None,
            scopes: std::iter::once(Scope::Read).collect(),
        }
    }
}

fn user(text: &str) -> (&'static str, Value) {
    ("message", json!({"role": "user", "text": text}))
}

fn assistant(text: &str) -> (&'static str, Value) {
    ("message", json!({"role": "assistant", "text": text, "message_id": format!("m-{text}")}))
}

fn tool(name: &str, id: &str) -> [(&'static str, Value); 2] {
    [
        ("tool_use", json!({"tool": name, "input": {"command": id}, "id": id})),
        (
            "tool_use",
            json!({"kind": "tool_result", "content": format!("out {id}"), "tool_use_id": id}),
        ),
    ]
}

fn first_prompt(prev: &str, instruction: &str) -> String {
    format!(
        "<cctui-session-context>\nsession: lane\n</cctui-session-context>\n\n\
         <cctui-previous-sessions>\n\
         <cctui-previous-conversation n=\"1\" session=\"prev\" id=\"{prev}\">\nbrief of {prev}\n\
         </cctui-previous-conversation>\n</cctui-previous-sessions>\n\n{instruction}"
    )
}

/// A → B → C, and B2 a later sibling follow-up of A. A is archived.
struct Line {
    a: String,
    b: String,
    c: String,
    b2: String,
}

async fn seed_line(f: &Fixture, a_by_stranger: bool) -> Line {
    let a = f.session(a_by_stranger, None, "archived", 60).await;
    let b = f.session(false, Some(&a), "archived", 50).await;
    let c = f.session(false, Some(&b), "active", 40).await;
    let b2 = f.session(false, Some(&a), "active", 30).await;
    let [a_call, a_result] = tool("Bash", "a1");
    f.events(&a, &[user("plan it"), a_call, a_result, assistant("planned")]).await;
    let [b_call, b_result] = tool("Edit", "b1");
    f.events(&b, &[user(&first_prompt(&a, "build it")), b_call, b_result, assistant("built")])
        .await;
    f.events(&b2, &[user(&first_prompt(&a, "other branch")), assistant("forked")]).await;
    f.events(&c, &[user(&first_prompt(&b, "test it")), assistant("tested")]).await;
    Line { a, b, c, b2 }
}

fn describe(entries: &[ThreadEntry], names: &HashMap<&str, &str>) -> Vec<String> {
    let name = |id: &str| names.get(id).copied().unwrap_or("?").to_owned();
    entries
        .iter()
        .map(|e| match e {
            ThreadEntry::Event { session_id, injected: Some(flag), .. } => {
                format!("{}:injected {flag:?}", name(session_id))
            }
            ThreadEntry::Event { session_id, event, .. } => {
                let s = |k: &str| event[k].as_str().unwrap_or_default().to_owned();
                let what = match s("type").as_str() {
                    "tool_call" => format!("tool {}", s("tool")),
                    "tool_result" => "result".to_owned(),
                    _ => s("content"),
                };
                format!("{}:{what}", name(session_id))
            }
            ThreadEntry::FollowupMarker { from, to, .. } => {
                format!("marker {}->{}", name(from), name(to))
            }
            ThreadEntry::Unreadable { session_id, .. } => {
                format!("unreadable {}", name(session_id))
            }
        })
        .collect()
}

fn names(line: &Line) -> HashMap<&str, &str> {
    HashMap::from([
        (line.a.as_str(), "A"),
        (line.b.as_str(), "B"),
        (line.c.as_str(), "C"),
        (line.b2.as_str(), "B2"),
    ])
}

async fn whole(f: &Fixture, id: &str) -> ThreadConversationPage {
    let ctx = f.ctx();
    let page = conversation(&f.state, Reader::Caller(&ctx), id, None, 10_000).await.unwrap();
    let page = page.expect("known session");
    assert!(page.next_cursor.is_none(), "one page holds the whole line");
    page
}

#[tokio::test]
async fn a_line_from_its_head_collates_every_generation_with_tools_and_markers() {
    let Some(f) = fixture("thread_conversation_line").await else { return };
    let line = seed_line(&f, false).await;
    let names = names(&line);

    let page = whole(&f, &line.c).await;
    assert_eq!(
        describe(&page.entries, &names),
        [
            "A:▷ User: plan it",
            "A:tool Bash",
            "A:result",
            "A:planned",
            "marker A->B",
            "B:injected SessionContext",
            "B:injected PreviousSessions",
            "B:▷ User: build it",
            "B:tool Edit",
            "B:result",
            "B:built",
            "marker B->C",
            "C:injected SessionContext",
            "C:injected PreviousSessions",
            "C:▷ User: test it",
            "C:tested",
        ],
        "archived A is included, B2 is not on the line"
    );
    assert_eq!(page.head, line.c);
    assert_eq!(page.thread_root_id, line.a, "an unstamped line falls back to its chain root");
    assert_eq!(page.next_session_id, None);
    let generations: Vec<(&str, u32, bool)> = page
        .generations
        .iter()
        .map(|g| (g.session_id.as_str(), g.generation, g.readable))
        .collect();
    assert_eq!(
        generations,
        [(line.a.as_str(), 1, true), (line.b.as_str(), 2, true), (line.c.as_str(), 3, true)]
    );
    let instruction = page.entries.iter().find_map(|e| match e {
        ThreadEntry::Event { injected: None, event, generation: 3, .. }
            if event["content"] == "▷ User: test it" =>
        {
            Some(event.clone())
        }
        _ => None,
    });
    let instruction = instruction.expect("C's instruction is a plain user turn");
    assert_eq!(crate::normalize::client_category(&instruction).0, "user");
    assert!(instruction["seq"].as_i64().is_some() && instruction["ts"].as_i64().is_some());

    let from_b = whole(&f, &line.b).await;
    assert_eq!(
        describe(&from_b.entries, &names),
        [
            "A:▷ User: plan it",
            "A:tool Bash",
            "A:result",
            "A:planned",
            "marker A->B",
            "B:injected SessionContext",
            "B:injected PreviousSessions",
            "B:▷ User: build it",
            "B:tool Edit",
            "B:result",
            "B:built",
        ]
    );
    assert_eq!(from_b.head, line.b);
    assert_eq!(from_b.next_session_id.as_deref(), Some(line.c.as_str()));

    let ctx = f.ctx();
    let unknown = conversation(&f.state, Reader::Caller(&ctx), "no-such", None, 10).await.unwrap();
    assert!(unknown.is_none());
    f.cleanup().await;
}

#[tokio::test]
async fn every_page_concatenated_is_the_whole_line_without_gap_or_duplicate() {
    let Some(f) = fixture("thread_conversation_paging").await else { return };
    let line = seed_line(&f, false).await;
    let names = names(&line);
    let all = whole(&f, &line.c).await;
    let ctx = f.ctx();

    for limit in 1..=6 {
        let mut pages: Vec<Vec<ThreadEntry>> = Vec::new();
        let mut cursor: Option<ThreadCursor> = None;
        loop {
            let page = conversation(&f.state, Reader::Caller(&ctx), &line.c, cursor, limit)
                .await
                .unwrap()
                .unwrap();
            pages.push(page.entries);
            let Some(next) = page.next_cursor else { break };
            cursor = Some(ThreadCursor::parse(&next).expect("a cursor the route parses"));
            assert!(pages.len() < 100, "paging must terminate at limit {limit}");
        }
        let joined: Vec<ThreadEntry> = pages.into_iter().rev().flatten().collect();
        assert_eq!(describe(&joined, &names), describe(&all.entries, &names), "limit {limit}");
        assert_eq!(joined, all.entries, "limit {limit}");
    }

    for (generation, sid) in [(1, &line.a), (2, &line.b), (3, &line.c)] {
        let q = ConversationQuery { limit: None, ..Default::default() };
        let mut own: Vec<i64> = renderable_rows(&f.pool, sid, "claude-code", &q)
            .await
            .unwrap()
            .into_iter()
            .map(|(seq, _, _, _)| seq)
            .collect();
        own.reverse();
        let mut collated: Vec<i64> = all
            .entries
            .iter()
            .filter(|e| matches!(e, ThreadEntry::Event { .. }) && e.generation() == generation)
            .map(|e| e.cursor().seq)
            .collect();
        collated.dedup();
        assert_eq!(collated, own, "generation {generation} is its own conversation");
    }
    let markers =
        all.entries.iter().filter(|e| matches!(e, ThreadEntry::FollowupMarker { .. })).count();
    assert_eq!(markers, 2);
    f.cleanup().await;
}

async fn thread_route(
    f: &Fixture,
    id: &str,
    cursor: Option<&str>,
) -> Result<axum::response::Response, crate::error::AppError> {
    get_conversation(
        State(f.state.clone()),
        Extension(f.ctx()),
        Path(id.to_owned()),
        Query(ConversationQuery::default()),
        Query(InlineImagesQuery::default()),
        Query(ThreadConversationQuery {
            thread: Some("1".to_owned()),
            cursor: cursor.map(str::to_owned),
        }),
        axum::http::HeaderMap::new(),
    )
    .await
}

#[tokio::test]
async fn an_unreadable_generation_is_a_placeholder_and_the_route_answers_200() {
    let Some(f) = fixture("thread_conversation_unreadable").await else { return };
    let line = seed_line(&f, true).await;
    let names = names(&line);

    let response = thread_route(&f, &line.c, None).await.expect("the thread answers");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let page: ThreadConversationPage = serde_json::from_slice(&body).unwrap();

    let described = describe(&page.entries, &names);
    assert_eq!(described[..2], ["unreadable A", "marker A->B"]);
    assert!(described.iter().all(|d| !d.starts_with("A:")), "{described:?}");
    assert_eq!(described.last().map(String::as_str), Some("C:tested"));
    let a = &page.generations[0];
    assert!(!a.readable && a.name.is_none(), "{a:?}");
    assert!(page.generations[1..].iter().all(|g| g.readable));

    let bad = thread_route(&f, &line.c, Some("nope")).await;
    assert!(bad.is_err(), "a malformed cursor is refused");
    f.cleanup().await;
}

#[tokio::test]
async fn claiming_a_followup_records_the_line_root() {
    let Some(f) = fixture("thread_root_on_claim").await else { return };
    let root_of = |id: String| {
        let pool = f.pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT thread_root_id FROM sessions WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let intend = |child: String, parent: String| {
        let pool = f.pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO session_followup_intents (spawn_key, parent_session_id) \
                 VALUES ($1, $2)",
            )
            .bind(&child)
            .bind(&parent)
            .execute(&pool)
            .await
            .unwrap();
            crate::followup::claim_intent(&pool, &child, None).await;
        }
    };

    let a = f.session(false, None, "active", 30).await;
    let b = f.session(false, None, "active", 20).await;
    let c = f.session(false, None, "active", 10).await;
    assert_eq!(root_of(a.clone()).await, None, "a lone session has no line yet");
    intend(b.clone(), a.clone()).await;
    assert_eq!(root_of(a.clone()).await.as_deref(), Some(a.as_str()), "A becomes the root");
    assert_eq!(root_of(b.clone()).await.as_deref(), Some(a.as_str()));
    intend(c.clone(), b).await;
    assert_eq!(root_of(c.clone()).await.as_deref(), Some(a.as_str()), "C joins A's line");

    let legacy_root = f.session(false, None, "archived", 60).await;
    let legacy = f.session(false, Some(&legacy_root), "archived", 50).await;
    let next = f.session(false, None, "active", 5).await;
    intend(next.clone(), legacy.clone()).await;
    assert_eq!(root_of(next).await.as_deref(), Some(legacy_root.as_str()));
    assert_eq!(root_of(legacy).await.as_deref(), Some(legacy_root.as_str()));
    let page = whole(&f, &c).await;
    assert_eq!(page.thread_root_id, a);
    f.cleanup().await;
}
