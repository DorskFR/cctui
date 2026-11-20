use cctui_proto::injected::PREVIOUS_CONVERSATION;
use serde_json::json;

use super::{DEFAULT_INSTRUCTION, FollowupPrompt, followup_from_brief, render_followup_prompt};
use crate::brief::{self, Link, Role, Transcript, Turn, collect_turns};

fn turn(role: Role, text: impl Into<String>) -> Turn {
    Turn { role, text: text.into() }
}

fn session(id: &str, name: &str, turns: Vec<Turn>) -> Link {
    Link { id: id.into(), transcript: Some(Transcript { name: name.into(), turns }) }
}

fn followup(links: &[Link], instruction: &str) -> FollowupPrompt {
    followup_from_brief(&brief::assemble(links, brief::DEFAULT_CHAIN_MAX_BYTES), instruction)
}

#[test]
fn the_server_prompt_wraps_the_parent_brief_then_the_instruction() {
    let turns = vec![turn(Role::User, "q"), turn(Role::Assistant, "a")];
    let out = followup(&[session("s-a", "wave \"3\" <x>", turns.clone())], " next step ");
    assert_eq!(out.file, None);
    assert_eq!(
        out.prompt,
        "<cctui-previous-conversation session=\"wave &quot;3&quot; &lt;x>\" id=\"s-a\">\n\
         **User:**\n\nq\n\n**Assistant:**\n\na\n</cctui-previous-conversation>\n\nnext step"
    );
    let blank = followup(&[session("s-a", "w", turns)], "  ");
    assert!(blank.prompt.ends_with(&format!("\n\n{DEFAULT_INSTRUCTION}")), "{}", blank.prompt);
}

#[test]
fn a_lone_parent_over_the_inline_budget_goes_whole_to_the_file() {
    let many: Vec<Turn> = (0..200)
        .map(|i| {
            let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
            turn(role, format!("t{i:03} {}", "y".repeat(512)))
        })
        .collect();
    let out = followup(&[session("s", &"n".repeat(10_000), many)], "go");
    let file = out.file.expect("file");
    for i in 0..200 {
        assert_eq!(file.matches(&format!("t{i:03} ")).count(), 1, "t{i:03}");
    }
    assert!(!file.contains("omitted"), "no turn cap");
    let close = format!("</{PREVIOUS_CONVERSATION}>");
    let inline = &out.prompt[..out.prompt.find(&close).expect("closing tag") + close.len()];
    assert!(inline.len() <= brief::INLINE_MAX_BYTES, "{}", inline.len());
    assert!(inline.contains(brief::BRIEF_FILE_NAME) && inline.contains("Read all of it"));
    assert!(inline.contains("t199 ") && !inline.contains("t000 "));
    assert!(out.prompt.ends_with("\n\ngo"));
}

#[test]
fn a_follow_up_of_a_server_rendered_follow_up_collapses_to_one_line() {
    let parent = [session("s-a", "a", vec![turn(Role::User, "old ask")])];
    let first = followup(&parent, "Now add tests.");
    let rows = vec![("message", json!({"role": "user", "text": first.prompt}))];
    let turns = collect_turns("claude-code", rows);
    assert_eq!(turns, [turn(Role::User, "(continued from session \"a\" s-a)\n\nNow add tests.")]);
}

#[test]
fn the_server_chain_prompt_points_at_the_file_and_collapses_per_session() {
    let links = [
        session(
            "s-a",
            "a \"x\"",
            vec![turn(Role::User, "ask A"), turn(Role::Assistant, "answer A")],
        ),
        Link { id: "s-x".into(), transcript: None },
        session("s-c", "c", vec![turn(Role::User, "(continued from session \"x\" s-x)\n\nask C")]),
    ];
    let out = followup(&links, " go ");
    let file = out.file.expect("a chain always ships the file");
    assert!(file.contains("(session s-x could not be included)"), "{file}");
    assert!(file.contains("**User:**\n\nask A\n\n**Assistant:**\n\nanswer A"), "{file}");
    assert!(out.prompt.starts_with(
        "<cctui-previous-sessions>\n\
         <cctui-previous-conversation n=\"1\" session=\"a &quot;x&quot;\" id=\"s-a\">\n\
         In the attached file previous-conversation.md."
    ));
    assert!(out.prompt.ends_with("</cctui-previous-sessions>\n\ngo"), "{}", out.prompt);
    let rows = vec![("message", json!({"role": "user", "text": out.prompt}))];
    let turns = collect_turns("claude-code", rows);
    assert_eq!(
        turns,
        [turn(
            Role::User,
            "(continued from session \"a \"x\"\" s-a)\n(continued from session \"\" s-x)\n\
             (continued from session \"c\" s-c)\n\ngo"
        )]
    );
}

#[test]
fn the_server_chain_prompt_keeps_every_turn_in_the_file_and_stays_inline_sized() {
    let links: Vec<Link> = (0..20)
        .map(|i| {
            let role = |t: i32| if t % 2 == 0 { Role::User } else { Role::Assistant };
            let text = |t: i32| format!("s{i:02}t{t:02} {}", "y".repeat(1024));
            let turns = (0..90).map(|t| turn(role(t), text(t))).collect();
            session(&format!("s-{i}"), &format!("n{i}"), turns)
        })
        .collect();
    let out = followup(&links, "go");
    let close = format!("</{}>", cctui_proto::injected::PREVIOUS_SESSIONS);
    let wrapped = out.prompt.find(&close).expect("closing tag") + close.len();
    assert!(wrapped <= brief::INLINE_MAX_BYTES, "{wrapped}");
    assert_eq!(out.prompt.matches("<cctui-previous-conversation n=").count(), 20);
    let file = out.file.expect("file");
    for (i, t) in [(0, 0), (0, 89), (10, 45), (19, 89)] {
        assert_eq!(file.matches(&format!("s{i:02}t{t:02} ")).count(), 1, "s{i:02}t{t:02}");
    }
    assert!(out.prompt.contains("s19t89 ") && !out.prompt.contains("s00t00 "));
}

#[tokio::test]
async fn render_followup_prompt_reads_the_parent_transcript() {
    let Some(url) = crate::routes::gateway::test_db_url("render_followup_prompt") else { return };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let (_, sid) = crate::inbox::store_tests::seed_session(&pool, "brief").await;
    for payload in [
        json!({"role": "user", "text": "hello"}),
        json!({"role": "assistant", "text": "hi there", "message_id": "m1"}),
    ] {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             VALUES ($1, 'message', $2)",
        )
        .bind(&sid)
        .bind(payload)
        .execute(&pool)
        .await
        .unwrap();
    }
    let out = render_followup_prompt(&pool, &sid, "pick it up").await.unwrap().unwrap();
    let prompt = out.prompt;
    assert_eq!(out.file, None);
    let open = format!("<{PREVIOUS_CONVERSATION} session=\"/w\" id=\"{sid}\">");
    assert!(prompt.starts_with(&open), "{prompt}");
    assert!(prompt.contains("**User:**\n\nhello\n\n**Assistant:**\n\nhi there\n"), "{prompt}");
    assert!(prompt.ends_with("\n\npick it up"));
    assert_eq!(render_followup_prompt(&pool, "no-such-session", "x").await.unwrap(), None);
}

mod chain {
    use axum::extract::{Path, Query, State};
    use axum::{Extension, Json};
    use cctui_proto::api::BriefResponse;
    use serde_json::json;
    use uuid::Uuid;

    use crate::auth::AuthContext;
    use crate::brief::{BriefQuery, session_brief};
    use crate::state::AppState;

    async fn owner(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
        let (uid, mid) = (Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("chain-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(pool)
            .await
            .expect("seed machine");
        (uid, mid)
    }

    async fn session(
        pool: &sqlx::PgPool,
        (uid, mid): (Uuid, Uuid),
        name: &str,
        parent: Option<&str>,
        texts: &[(&str, String)],
    ) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, user_id, \
             adapter_id, session_name, parent_id, metadata) \
             VALUES ($1, $2, $3, '/w', 'active', $4, 'claude-code', $5, $6, \
             CASE WHEN $6::text IS NULL THEN '{}'::jsonb \
             ELSE jsonb_build_object('relation', 'followup') END)",
        )
        .bind(&sid)
        .bind(mid.to_string())
        .bind(mid)
        .bind(uid)
        .bind(name)
        .bind(parent)
        .execute(pool)
        .await
        .expect("seed session");
        for (role, text) in texts {
            let payload = if *role == "user" {
                json!({"role": "user", "text": text})
            } else {
                json!({"role": "assistant", "text": text, "message_id": "m"})
            };
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) \
                 VALUES ($1, 'message', $2)",
            )
            .bind(&sid)
            .bind(payload)
            .execute(pool)
            .await
            .expect("seed event");
        }
        sid
    }

    fn previous(n: usize, name: &str, id: &str) -> String {
        format!(
            "<cctui-previous-conversation n=\"{n}\" session=\"{name}\" id=\"{id}\">\n\
             **User:**\n\nold\n</cctui-previous-conversation>\n"
        )
    }

    async fn brief(
        state: &AppState,
        uid: Uuid,
        id: &str,
        chain: bool,
    ) -> Result<BriefResponse, u16> {
        let ctx = AuthContext {
            user_id: uid,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::collections::BTreeSet::new(),
        };
        let q = BriefQuery { chain: chain.then(|| "1".to_owned()), ..BriefQuery::default() };
        session_brief(State(state.clone()), Extension(ctx), Path(id.to_owned()), Query(q))
            .await
            .map(|Json(out)| out)
            .map_err(|(status, _)| status.as_u16())
    }

    fn user(text: impl Into<String>) -> (&'static str, String) {
        ("user", text.into())
    }

    fn assistant(text: impl Into<String>) -> (&'static str, String) {
        ("assistant", text.into())
    }

    #[tokio::test]
    async fn a_chain_brief_walks_the_followup_line_only() {
        let name = "a_chain_brief_walks_the_followup_line_only";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let me = owner(&pool).await;
        let other = owner(&pool).await;
        let root = session(&pool, me, "a", None, &[user("ask A"), assistant("answer A")]).await;
        let b_ask = format!("{}\nask B", previous(1, "a", &root));
        let middle_turns = [user(b_ask), assistant("answer B")];
        let middle = session(&pool, me, "b", Some(&root), &middle_turns).await;
        let sibling = session(&pool, me, "b2", Some(&root), &[user("ask B2")]).await;
        let c_ask = format!(
            "<cctui-previous-sessions>\n{}{}</cctui-previous-sessions>\n\nask C",
            previous(1, "a", &root),
            previous(2, "b", &middle)
        );
        let parent_turns = [user(c_ask), assistant("answer C")];
        let parent = session(&pool, me, "c", Some(&middle), &parent_turns).await;
        let hidden = session(&pool, other, "x", None, &[user("secret ask")]).await;
        let y_ask = format!("{}\nask Y", previous(1, "x", &hidden));
        let heir = session(&pool, me, "y", Some(&hidden), &[user(y_ask)]).await;
        let state = AppState::for_test(pool.clone());

        let out = brief(&state, me.0, &parent, true).await.expect("chain brief");
        let sessions = out.sessions.expect("sessions");
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, [root.as_str(), middle.as_str(), parent.as_str()]);
        assert!(sessions[0].markdown.starts_with("**User:**\n\nask A"), "{}", sessions[0].markdown);
        for needle in ["ask A", "answer A", "ask B", "answer B", "ask C", "answer C"] {
            assert_eq!(out.markdown.matches(needle).count(), 1, "{needle}: {}", out.markdown);
        }
        assert!(!out.markdown.contains("ask B2"), "{}", out.markdown);
        assert!(!out.markdown.contains("previous-conversation"), "{}", out.markdown);
        assert!(!out.markdown.contains("(continued from"), "{}", out.markdown);

        let legacy = brief(&state, me.0, &parent, false).await.expect("plain brief");
        assert!(legacy.sessions.is_none());
        assert!(legacy.markdown.contains("ask C") && !legacy.markdown.contains("answer A"));
        assert_eq!(legacy.markdown.matches("(continued from session").count(), 2);

        let out = brief(&state, me.0, &heir, true).await.expect("unreadable ancestor is still 200");
        let sessions = out.sessions.expect("sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, None);
        assert_eq!(sessions[0].markdown, format!("(session {hidden} could not be included)"));
        assert!(!out.markdown.contains("secret ask"), "{}", out.markdown);
        assert!(
            sessions[1].markdown.contains(&format!("(continued from session \"x\" {hidden})")),
            "{}",
            sessions[1].markdown
        );

        assert_eq!(brief(&state, me.0, "no-such-session", true).await.err(), Some(404));

        let out = super::render_followup_prompt(&pool, &parent, "next").await.unwrap().unwrap();
        let served = brief(&state, me.0, &parent, true).await.expect("chain brief");
        assert_eq!(out, super::followup_from_brief(&served, "next"), "same as the webui path");
        let (prompt, file) = (out.prompt, out.file.expect("file"));
        assert!(prompt.starts_with("<cctui-previous-sessions>\n"), "{prompt}");
        assert!(prompt.ends_with("</cctui-previous-sessions>\n\nnext"), "{prompt}");
        assert_eq!(file.matches("ask A").count(), 1, "{file}");
        assert!(file.contains("answer C") && !file.contains("ask B2"), "{file}");
        let out = super::render_followup_prompt(&pool, &heir, "next").await.unwrap().unwrap();
        let file = out.file.expect("file");
        assert!(file.contains(&format!("(session {hidden} could not be included)")), "{file}");
        assert!(!file.contains("secret ask") && !out.prompt.contains("secret ask"), "{file}");

        let all = [heir, hidden, parent, sibling, middle, root];
        sqlx::query("DELETE FROM stream_events WHERE session_id = ANY($1)")
            .bind(&all[..])
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM sessions WHERE id = ANY($1)")
            .bind(&all[..])
            .execute(&pool)
            .await
            .ok();
        for (uid, mid) in [me, other] {
            sqlx::query("DELETE FROM machines WHERE id = $1").bind(mid).execute(&pool).await.ok();
            sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
        }
    }
}
