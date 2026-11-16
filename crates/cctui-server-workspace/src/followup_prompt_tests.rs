use cctui_proto::injected::PREVIOUS_CONVERSATION;
use serde_json::json;

use super::{DEFAULT_INSTRUCTION, followup_prompt, render_followup_prompt};
use crate::brief::{self, Role, Turn, collect_turns};

fn turn(role: Role, text: impl Into<String>) -> Turn {
    Turn { role, text: text.into() }
}

fn wrapped_len(prompt: &str) -> usize {
    let close = format!("</{PREVIOUS_CONVERSATION}>");
    prompt.find(&close).expect("closing tag") + close.len()
}

#[test]
fn the_server_prompt_wraps_the_parent_brief_then_the_instruction() {
    let turns = [turn(Role::User, "q"), turn(Role::Assistant, "a")];
    let prompt = followup_prompt("s-a", "wave \"3\" <x>", &turns, " next step ");
    assert_eq!(
        prompt,
        "<cctui-previous-conversation session=\"wave &quot;3&quot; &lt;x>\" id=\"s-a\">\n\
         **User:**\n\nq\n\n**Assistant:**\n\na\n</cctui-previous-conversation>\n\nnext step"
    );
    let blank = followup_prompt("s-a", "w", &turns, "  ");
    assert!(blank.ends_with(&format!("\n\n{DEFAULT_INSTRUCTION}")), "{blank}");
}

#[test]
fn the_wrapped_brief_respects_the_turn_and_byte_caps() {
    let many: Vec<Turn> = (0..200)
        .map(|i| turn(if i % 2 == 0 { Role::User } else { Role::Assistant }, format!("t{i}")))
        .collect();
    let prompt = followup_prompt("s", "n", &many, "go");
    assert!(prompt.contains("… 140 earlier turns omitted"), "{prompt}");
    assert!(prompt.contains("t199") && !prompt.contains("t139\n"));

    let big: Vec<Turn> =
        (0..200).map(|i| turn(Role::User, format!("{i:04}").repeat(256))).collect();
    let prompt = followup_prompt("s", &"n".repeat(10_000), &big, "go");
    assert!(wrapped_len(&prompt) <= brief::DEFAULT_MAX_BYTES, "{}", wrapped_len(&prompt));
    assert!(prompt.contains(&"0199".repeat(256)));
    assert!(prompt.contains("earlier turns omitted"));

    let huge = [turn(Role::Assistant, format!("{}tail", "x".repeat(200 * 1024)))];
    let prompt = followup_prompt("s", "n", &huge, "go");
    assert!(wrapped_len(&prompt) <= brief::DEFAULT_MAX_BYTES, "{}", wrapped_len(&prompt));
    assert!(prompt.contains("tail\n</cctui-previous-conversation>"));
}

#[test]
fn a_follow_up_of_a_server_rendered_follow_up_collapses_to_one_line() {
    let first = followup_prompt("s-a", "a", &[turn(Role::User, "old ask")], "Now add tests.");
    let rows = vec![("message", json!({"role": "user", "text": first}))];
    let turns = collect_turns("claude-code", rows);
    assert_eq!(turns, [turn(Role::User, "(continued from session \"a\" s-a)\n\nNow add tests.")]);
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
    let prompt = render_followup_prompt(&pool, &sid, "pick it up").await.unwrap().unwrap();
    let open = format!("<{PREVIOUS_CONVERSATION} session=\"/w\" id=\"{sid}\">");
    assert!(prompt.starts_with(&open), "{prompt}");
    assert!(prompt.contains("**User:**\n\nhello\n\n**Assistant:**\n\nhi there\n"), "{prompt}");
    assert!(prompt.ends_with("\n\npick it up"));
    assert_eq!(render_followup_prompt(&pool, "no-such-session", "x").await.unwrap(), None);
}
