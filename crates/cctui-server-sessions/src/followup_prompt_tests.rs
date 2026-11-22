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
    let Some(url) = cctui_server_kernel::test_support::db_url("render_followup_prompt") else {
        return;
    };
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
