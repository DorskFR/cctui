use cctui_crypto::redact::{CompiledPatterns, USER_PLACEHOLDER, compile};
use cctui_proto::adapter::{AdapterCommand, AdapterEvent, EndReason, SessionMeta};
use serde_json::{Value, json};

use super::note_command;
use crate::supervisor::scrub_event;

const SECRET: &str = "hunter2-xyz";
const PROMPT: &str = "log in with <redacted>hunter2-xyz</redacted> please";

fn disabled() -> CompiledPatterns {
    let scrub = compile(false, &[], b"");
    assert!(scrub.is_empty(), "these tests run with scrubbing disabled");
    scrub
}

fn send(local_id: &str, text: &str) {
    note_command(&AdapterCommand::SendMessage { local_id: local_id.to_owned(), text: text.into() });
}

fn message(local_id: &str, payload: Value) -> AdapterEvent {
    AdapterEvent::Message { local_id: local_id.to_owned(), payload, turn_id: None }
}

fn tool(local_id: &str, payload: Value) -> AdapterEvent {
    AdapterEvent::ToolUse { local_id: local_id.to_owned(), payload }
}

fn scrubbed(event: AdapterEvent) -> Value {
    serde_json::to_value(scrub_event(event, &disabled())).unwrap()
}

fn assert_masked(events: Vec<AdapterEvent>) -> Vec<Value> {
    events
        .into_iter()
        .map(|event| {
            let kind = serde_json::to_value(&event).unwrap()["kind"].clone();
            let out = scrubbed(event);
            let text = out.to_string();
            assert!(!text.contains(SECRET), "{SECRET} leaked: {text}");
            assert!(text.contains(USER_PLACEHOLDER), "nothing masked: {text}");
            assert_eq!(out["kind"], kind, "the event kind survives");
            out
        })
        .collect()
}

#[test]
fn claude_echoes_of_a_tagged_value_are_masked_with_scrubbing_disabled() {
    let sid = "user-redact-claude";
    send(sid, PROMPT);
    let out = assert_masked(vec![
        message(sid, json!({"role": "user", "text": PROMPT, "meta": false})),
        message(sid, json!({"role": "assistant", "text": format!("Using {SECRET} now.")})),
        message(sid, json!({"role": "assistant_thinking", "text": format!("pw is {SECRET}")})),
        tool(
            sid,
            json!({
                "tool": "Bash",
                "tool_use_id": "toolu_1",
                "input": {"command": format!("curl -H \"Authorization: Bearer {SECRET}\" x")},
            }),
        ),
        tool(sid, json!({"type": "tool_result", "tool_use_id": "toolu_1", "content": SECRET})),
    ]);
    assert_eq!(out[0]["payload"]["text"], "log in with [REDACTED:user] please");
    assert_eq!(out[0]["payload"]["role"], "user");
    assert_eq!(out[1]["payload"]["text"], "Using [REDACTED:user] now.");
    assert_eq!(out[3]["payload"]["tool"], "Bash");
}

#[test]
fn a_short_value_masks_its_span_but_not_its_echoes() {
    let sid = "user-redact-short";
    let reply: AdapterCommand = serde_json::from_value(json!({
        "kind": "reply",
        "local_id": sid,
        "text": "code <redacted>abc</redacted> then abc",
    }))
    .unwrap();
    note_command(&reply);
    let echo = scrubbed(message(
        sid,
        json!({"role": "user", "text": "code <redacted>abc</redacted> then abc"}),
    ));
    assert_eq!(echo["payload"]["text"], "code [REDACTED:user] then abc");
    let answer = scrubbed(message(sid, json!({"role": "assistant", "text": "abc is short"})));
    assert_eq!(answer["payload"]["text"], "abc is short");
}

#[test]
fn codex_echoes_of_a_tagged_value_are_masked() {
    let sid = "user-redact-codex-thread";
    send(sid, PROMPT);
    let out = assert_masked(vec![
        message(sid, json!({"type": "userMessage", "content": [{"type": "text", "text": PROMPT}]})),
        message(sid, json!({"type": "agentMessage", "id": "i2", "text": format!("ok {SECRET}")})),
        message(sid, json!({"type": "reasoning", "id": "i3", "summary": [SECRET]})),
        tool(
            sid,
            json!({
                "type": "commandExecution",
                "id": "i4",
                "command": format!("echo {SECRET}"),
                "aggregatedOutput": format!("{SECRET}\n"),
            }),
        ),
    ]);
    assert_eq!(out[0]["payload"]["type"], "userMessage");
    assert_eq!(out[0]["payload"]["content"][0]["text"], "log in with [REDACTED:user] please");
}

#[test]
fn opencode_echoes_are_masked_once_the_spawn_key_names_the_session() {
    let key = uuid::Uuid::new_v4();
    let sid = "user-redact-opencode-ses";
    let spawn: AdapterCommand = serde_json::from_value(json!({
        "kind": "spawn",
        "spec": {"adapter_id": "opencode", "working_dir": "/w", "prompt": PROMPT},
        "session_id": key,
    }))
    .unwrap();
    note_command(&spawn);
    let meta =
        SessionMeta { extra: json!({"spawn_key": key.to_string()}), ..SessionMeta::default() };
    scrubbed(AdapterEvent::SessionStarted { local_id: sid.to_owned(), meta });
    let out = assert_masked(vec![
        message(
            sid,
            json!({
                "type": "text",
                "content": format!("▷ User: {PROMPT}"),
                "role": "user",
                "text": PROMPT,
                "meta": false,
                "line_id": "prt_1",
            }),
        ),
        message(
            sid,
            json!({
                "type": "text",
                "content": SECRET,
                "role": "assistant",
                "text": SECRET,
                "message_id": "msg_1",
            }),
        ),
        tool(
            sid,
            json!({
                "type": "tool_call",
                "tool": "bash",
                "input": {"command": format!("echo {SECRET}")},
                "id": "call_1",
                "tool_use_id": "call_1",
            }),
        ),
        tool(
            sid,
            json!({
                "type": "tool_result",
                "output_summary": SECRET,
                "kind": "tool_result",
                "content": SECRET,
                "tool_use_id": "call_1",
            }),
        ),
    ]);
    assert_eq!(out[0]["payload"]["content"], "▷ User: log in with [REDACTED:user] please");
    assert_eq!(out[1]["payload"]["role"], "assistant");
}

#[test]
fn a_pty_frame_echoing_the_value_is_masked() {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let sid = "user-redact-pty";
    send(sid, PROMPT);
    let data = engine.encode(format!("$ echo {SECRET}\r\n"));
    let out = scrub_event(AdapterEvent::PtyChunk { local_id: sid.to_owned(), data }, &disabled());
    let AdapterEvent::PtyChunk { data, .. } = out else { panic!("not a pty chunk") };
    let text = String::from_utf8(engine.decode(data).unwrap()).unwrap();
    assert_eq!(text, "$ echo [REDACTED:user]\r\n");
}

#[test]
fn the_echo_set_is_dropped_when_the_session_ends() {
    let sid = "user-redact-ended";
    send(sid, PROMPT);
    scrubbed(AdapterEvent::SessionEnded { local_id: sid.to_owned(), reason: EndReason::Completed });
    let late = scrubbed(message(sid, json!({"role": "assistant", "text": SECRET})));
    assert_eq!(late["payload"]["text"], SECRET);
    let span = scrubbed(message(sid, json!({"role": "user", "text": PROMPT})));
    assert_eq!(span["payload"]["text"], "log in with [REDACTED:user] please");
}

#[test]
fn another_sessions_echo_is_left_alone() {
    send("user-redact-owner", PROMPT);
    let other = scrubbed(message("user-redact-bystander", json!({"text": SECRET})));
    assert_eq!(other["payload"]["text"], SECRET);
}
