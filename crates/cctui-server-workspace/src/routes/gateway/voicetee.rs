//! Live-voice text read off model responses: a claude-code background
//! session's transcript carries only finished blocks.

use axum::body::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use serde_json::Value;

use super::toolguard::{SseEvent, event_end};
use crate::bus::{Bus, VoiceEvent};

const MAX_USER_TEXT: usize = 16 * 1024;

#[cfg(test)]
use cctui_server_core::gateway::proxy::teeable_request;

pub fn watched_session(bus: &Bus, session_id: Option<String>) -> Option<String> {
    session_id.filter(|s| bus.voice_watched(s))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceTee {
    session_id: String,
    request_id: String,
    user_text: String,
}

impl VoiceTee {
    pub fn from_request(session_id: String, body: &Value) -> Option<Self> {
        let streamed = body.get("stream").and_then(Value::as_bool) == Some(true);
        let tools = body.get("tools").and_then(Value::as_array).is_some_and(|t| !t.is_empty());
        if !(streamed && tools) {
            return None;
        }
        let user_text = latest_user_text(body)?;
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        Some(Self { session_id, request_id, user_text })
    }
}

enum UserContent {
    ToolResults,
    Text(String),
}

fn user_content(content: Option<&Value>) -> UserContent {
    match content {
        Some(Value::String(s)) => UserContent::Text(s.clone()),
        Some(Value::Array(blocks)) => {
            let is = |b: &Value, kind: &str| b.get("type").and_then(Value::as_str) == Some(kind);
            if blocks.iter().any(|b| is(b, "tool_result")) {
                return UserContent::ToolResults;
            }
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| is(b, "text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            UserContent::Text(texts.join("\n"))
        }
        _ => UserContent::Text(String::new()),
    }
}

/// The latest user message without a tool result, so every call of one agent
/// loop names the same prompt. A long one keeps its tail, past the reminders.
pub fn latest_user_text(body: &Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;
    let mut parts: Vec<String> = Vec::new();
    let mut found = false;
    for m in messages.iter().rev() {
        let content = (m.get("role").and_then(Value::as_str) == Some("user"))
            .then(|| user_content(m.get("content")));
        match content {
            Some(UserContent::Text(text)) => {
                found = true;
                parts.push(text);
            }
            _ if found => break,
            _ => {}
        }
    }
    parts.reverse();
    let joined = parts.join("\n");
    let text = tail(joined.trim(), MAX_USER_TEXT);
    (!text.is_empty()).then(|| text.to_owned())
}

fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[derive(Default)]
pub struct TextTee {
    pending: Vec<u8>,
}

impl TextTee {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<(u64, String)> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = event_end(&self.pending) {
            let raw: Vec<u8> = self.pending.drain(..end).collect();
            if raw.windows(12).any(|w| w == b"\"text_delta\"")
                && let Some(delta) = text_delta(&SseEvent::parse(raw))
            {
                out.push(delta);
            }
        }
        out
    }
}

fn text_delta(ev: &SseEvent) -> Option<(u64, String)> {
    let v = ev.json()?;
    if v.get("type").and_then(Value::as_str) != Some("content_block_delta") {
        return None;
    }
    let delta = v.get("delta")?;
    if delta.get("type").and_then(Value::as_str) != Some("text_delta") {
        return None;
    }
    let text = delta.get("text").and_then(Value::as_str).filter(|t| !t.is_empty())?;
    Some((v.get("index")?.as_u64()?, text.to_owned()))
}

pub fn tee_stream<S, E>(
    inner: S,
    bus: Bus,
    tee: VoiceTee,
) -> impl Stream<Item = Result<Bytes, E>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let VoiceTee { session_id, request_id, user_text } = tee;
    bus.publish_voice(VoiceEvent::Request {
        session_id: session_id.clone(),
        request_id: request_id.clone(),
        user_text,
    });
    let mut reader = TextTee::default();
    inner.map(move |chunk| {
        if let Ok(bytes) = &chunk {
            for (index, delta) in reader.push(bytes) {
                bus.publish_voice(VoiceEvent::Delta {
                    session_id: session_id.clone(),
                    item_id: format!("gw:{request_id}:{index}"),
                    delta,
                });
            }
        }
        chunk
    })
}

pub fn maybe_tee<E: Send + 'static>(
    stream: BoxStream<'static, Result<Bytes, E>>,
    bus: &Bus,
    tee: Option<VoiceTee>,
    readable_sse: bool,
) -> BoxStream<'static, Result<Bytes, E>> {
    match tee.filter(|_| readable_sse) {
        Some(tee) => tee_stream(stream, bus.clone(), tee).boxed(),
        None => stream,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(v: &Value) -> String {
        format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())
    }

    fn fixture() -> Vec<u8> {
        let events = [
            json!({"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"x"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"ping"}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello, "}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"wörld."}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t","name":"Bash","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"text_delta\":"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"1}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ];
        events.iter().map(ev).collect::<String>().into_bytes()
    }

    fn bus() -> Bus {
        Bus::new(Box::new(crate::bus::NoopTransport))
    }

    fn tee() -> VoiceTee {
        VoiceTee { session_id: "s".into(), request_id: "r1".into(), user_text: "go".into() }
    }

    #[test]
    fn only_text_deltas_are_read_whatever_the_chunking() {
        let body = fixture();
        for size in [1, 3, 7, 64, body.len()] {
            let mut reader = TextTee::default();
            let got: Vec<(u64, String)> = body.chunks(size).flat_map(|c| reader.push(c)).collect();
            assert_eq!(
                got,
                vec![(1, "Hello, ".to_owned()), (1, "wörld.".to_owned())],
                "chunk size {size}"
            );
        }
    }

    #[tokio::test]
    async fn a_teed_stream_forwards_bytes_unchanged_and_publishes_its_text() {
        let bus = bus();
        let mut rx = bus.subscribe_voice();
        let body = fixture();
        let chunks: Vec<Result<Bytes, ()>> =
            body.chunks(5).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
        let out: Vec<Result<Bytes, ()>> =
            tee_stream(futures_util::stream::iter(chunks), bus.clone(), tee()).collect().await;
        let forwarded: Vec<u8> = out.into_iter().flat_map(|c| c.unwrap().to_vec()).collect();
        assert_eq!(forwarded, body);
        let request = VoiceEvent::Request {
            session_id: "s".into(),
            request_id: "r1".into(),
            user_text: "go".into(),
        };
        assert_eq!(rx.try_recv().unwrap(), request);
        let delta = |d: &str| VoiceEvent::Delta {
            session_id: "s".into(),
            item_id: "gw:r1:1".into(),
            delta: d.into(),
        };
        assert_eq!(rx.try_recv().unwrap(), delta("Hello, "));
        assert_eq!(rx.try_recv().unwrap(), delta("wörld."));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn an_unwatched_session_is_not_teed() {
        let bus = bus();
        let mut rx = bus.subscribe_voice();
        assert_eq!(watched_session(&bus, Some("s".into())), None);
        assert_eq!(watched_session(&bus, None), None);
        let body = Bytes::from(fixture());
        let inner = futures_util::stream::iter(vec![Ok::<_, ()>(body.clone())]).boxed();
        let out: Vec<Result<Bytes, ()>> = maybe_tee(inner, &bus, None, true).collect().await;
        assert_eq!(out, vec![Ok(body.clone())]);
        let inner = futures_util::stream::iter(vec![Ok::<_, ()>(body)]).boxed();
        let _: Vec<Result<Bytes, ()>> = maybe_tee(inner, &bus, Some(tee()), false).collect().await;
        assert!(rx.try_recv().is_err(), "nothing published");

        bus.deliver_local(crate::bus::BusEvent::Voice(VoiceEvent::Watch {
            session_id: "s".into(),
            watcher: uuid::Uuid::new_v4(),
            on: true,
        }));
        assert_eq!(watched_session(&bus, Some("s".into())).as_deref(), Some("s"));
    }

    #[test]
    fn only_streamed_agent_calls_with_tools_are_teed() {
        let msgs = json!([{"role":"user","content":"hi"}]);
        let tools = json!([{"name":"Bash"}]);
        let req = |body: &Value| VoiceTee::from_request("s".into(), body);
        assert!(req(&json!({"stream":true,"tools":tools,"messages":msgs})).is_some());
        assert!(req(&json!({"stream":false,"tools":tools,"messages":msgs})).is_none());
        let untooled = json!({"stream":true,"tools":[],"messages":msgs});
        assert!(req(&untooled).is_none(), "title generation has no tools");
        assert!(req(&json!({"stream":true,"messages":msgs})).is_none());
        let post = axum::http::Method::POST;
        assert!(teeable_request(&post, "/v1/messages"));
        assert!(!teeable_request(&post, "/v1/messages/count_tokens"));
        assert!(!teeable_request(&axum::http::Method::GET, "/v1/messages"));
    }

    fn user_text(messages: &Value) -> Option<String> {
        latest_user_text(&json!({ "messages": messages }))
    }

    #[test]
    fn the_latest_user_text_is_the_prompt_the_loop_answers() {
        let plain = json!([{"role":"user","content":"plain"}]);
        assert_eq!(user_text(&plain).as_deref(), Some("plain"));
        let wrapped = json!([
            {"role":"user","content":"old"},
            {"role":"assistant","content":[{"type":"text","text":"ok"}]},
            {"role":"user","content":[
                {"type":"text","text":"<system-reminder>be nice</system-reminder>"},
                {"type":"image","source":{}},
                {"type":"text","text":"fix the bug"}
            ]}
        ]);
        assert_eq!(
            user_text(&wrapped).as_deref(),
            Some("<system-reminder>be nice</system-reminder>\nfix the bug")
        );
        let continued = json!([
            {"role":"user","content":"<system-reminder>r</system-reminder>"},
            {"role":"user","content":[{"type":"text","text":"fix the bug"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{}}]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t2","content":"ok"},
                {"type":"text","text":"<system-reminder>todo</system-reminder>"}
            ]}
        ]);
        assert_eq!(
            user_text(&continued).as_deref(),
            Some("<system-reminder>r</system-reminder>\nfix the bug")
        );
        let result = json!({"type":"tool_result","tool_use_id":"t","content":"x"});
        let only_tools = json!([{"role":"user","content":[result]}]);
        assert_eq!(user_text(&only_tools), None);
    }

    #[test]
    fn a_long_user_text_keeps_its_tail_on_a_char_boundary() {
        let long = format!("{}é tail", "x".repeat(MAX_USER_TEXT));
        let got = user_text(&json!([{"role":"user","content":long}])).unwrap();
        assert!(got.len() <= MAX_USER_TEXT);
        assert!(got.ends_with("é tail"));
    }
}
