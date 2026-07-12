//! Slice a Claude transcript JSONL into a standalone child transcript for a
//! subset fork (CCT-553). The parent's real history lives at
//! `~/.claude/projects/<encoded-cwd>/<parent>.jsonl`; to fork a slice we
//! materialize the kept lines as the child's own `<child>.jsonl` and resume
//! that file directly (no `--fork-session`).
//!
//! Fidelity is the hard part: the slice must not leave a `tool_use` without its
//! `tool_result` (or vice-versa) and the `parentUuid` chain must stay linear,
//! or claude misbehaves on resume. The kept lines are therefore repaired
//! (orphan tool blocks stripped) and re-linked (each conversation line's
//! `parentUuid` re-pointed at the previous kept conversation line).

use std::collections::HashSet;

use serde_json::{Value, json};

use cctui_proto::adapter::{ForkExtract, ForkMode};

/// A conversation line carries a `uuid`; header/meta lines (custom-title,
/// agent-name, …) do not and are never part of the parentUuid chain.
fn is_conversation(line: &Value) -> bool {
    matches!(line.get("type").and_then(Value::as_str), Some("user" | "assistant"))
}

fn message_id(line: &Value) -> Option<&str> {
    line.get("message").and_then(|m| m.get("id")).and_then(Value::as_str)
}

/// A user line that opens a new turn: type `user` whose content carries no
/// `tool_result` block (tool-result user lines continue the current turn).
fn is_turn_start(line: &Value) -> bool {
    if line.get("type").and_then(Value::as_str) != Some("user") {
        return false;
    }
    !has_tool_result(line)
}

fn has_tool_result(line: &Value) -> bool {
    line.get("message").and_then(|m| m.get("content")).and_then(Value::as_array).is_some_and(
        |blocks| {
            blocks.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        },
    )
}

/// Index of the last conversation line whose assistant `message_id` == `anchor`.
fn anchor_index(lines: &[Value], anchor: &str) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_conversation(l))
        .filter(|(_, l)| message_id(l) == Some(anchor))
        .map(|(i, _)| i)
        .next_back()
}

/// The set of conversation-line indices kept for a `selected` fork: every line
/// belonging to a turn that contains at least one selected message. A turn runs
/// from a `is_turn_start` user line up to (excluding) the next one.
fn selected_indices(lines: &[Value], selected: &HashSet<&str>) -> HashSet<usize> {
    let mut kept = HashSet::new();
    let mut turn: Vec<usize> = Vec::new();
    let mut turn_hit = false;
    let flush = |turn: &mut Vec<usize>, hit: &mut bool, kept: &mut HashSet<usize>| {
        if *hit {
            kept.extend(turn.iter().copied());
        }
        turn.clear();
        *hit = false;
    };
    for (i, line) in lines.iter().enumerate() {
        if !is_conversation(line) {
            continue;
        }
        if is_turn_start(line) {
            flush(&mut turn, &mut turn_hit, &mut kept);
        }
        turn.push(i);
        if message_id(line).is_some_and(|m| selected.contains(m)) {
            turn_hit = true;
        }
    }
    flush(&mut turn, &mut turn_hit, &mut kept);
    kept
}

/// Strip orphaned `tool_use` / `tool_result` blocks so no half-pair survives
/// the cut. A pair is valid only when BOTH its `tool_use` and matching
/// `tool_result` are in the kept set; any block whose partner was dropped is
/// removed, and a user line left with no content is discarded entirely.
fn repair_tool_pairs(lines: Vec<Value>) -> Vec<Value> {
    let mut uses: HashSet<String> = HashSet::new();
    let mut results: HashSet<String> = HashSet::new();
    for line in &lines {
        for_each_block(line, |b| match b.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                if let Some(id) = b.get("id").and_then(Value::as_str) {
                    uses.insert(id.to_owned());
                }
            }
            Some("tool_result") => {
                if let Some(id) = b.get("tool_use_id").and_then(Value::as_str) {
                    results.insert(id.to_owned());
                }
            }
            _ => {}
        });
    }
    let valid: HashSet<&String> = uses.intersection(&results).collect();

    let mut out = Vec::with_capacity(lines.len());
    for mut line in lines {
        let had_blocks =
            line.get("message").and_then(|m| m.get("content")).and_then(Value::as_array).is_some();
        if let Some(blocks) =
            line.get_mut("message").and_then(|m| m.get_mut("content")).and_then(Value::as_array_mut)
        {
            blocks.retain(|b| match b.get("type").and_then(Value::as_str) {
                Some("tool_use") => b
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| valid.contains(&id.to_owned())),
                Some("tool_result") => b
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| valid.contains(&id.to_owned())),
                _ => true,
            });
            if had_blocks && blocks.is_empty() {
                continue;
            }
        }
        out.push(line);
    }
    out
}

fn for_each_block(line: &Value, mut f: impl FnMut(&Value)) {
    if let Some(blocks) =
        line.get("message").and_then(|m| m.get("content")).and_then(Value::as_array)
    {
        for b in blocks {
            f(b);
        }
    }
}

/// Re-link the parentUuid chain linearly across kept conversation lines and
/// rewrite every `sessionId`/`session_id` to the child id. Header/meta lines
/// (no `uuid`) keep their place but stay out of the chain.
fn relink(mut lines: Vec<Value>, child_session_id: &str) -> Vec<Value> {
    let mut prev: Option<Value> = None;
    for line in &mut lines {
        if let Some(obj) = line.as_object_mut() {
            for key in ["sessionId", "session_id"] {
                if obj.contains_key(key) {
                    obj.insert(key.to_owned(), json!(child_session_id));
                }
            }
        }
        if line.get("uuid").is_some() {
            let parent = prev.clone().unwrap_or(Value::Null);
            if let Some(obj) = line.as_object_mut() {
                obj.insert("parentUuid".to_owned(), parent);
            }
            prev = line.get("uuid").cloned();
        }
    }
    lines
}

/// Slice a parsed transcript (one `Value` per JSONL line) into the kept lines
/// for a child fork. Header/meta lines preceding the first kept conversation
/// line are retained (they carry session identity); trailing meta is dropped.
///
/// Errors when the anchor / selection resolves to nothing.
pub fn slice_transcript(
    lines: &[Value],
    extract: &ForkExtract,
    child_session_id: &str,
) -> anyhow::Result<Vec<Value>> {
    let conv: Vec<usize> =
        lines.iter().enumerate().filter(|(_, l)| is_conversation(l)).map(|(i, _)| i).collect();

    let keep_conv: HashSet<usize> = match extract.mode {
        ForkMode::UpTo => {
            let anchor = extract
                .anchor_message_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("up_to fork requires an anchor_message_id"))?;
            let idx = anchor_index(lines, anchor)
                .ok_or_else(|| anyhow::anyhow!("anchor message not found in transcript"))?;
            conv.iter().copied().filter(|&i| i <= idx).collect()
        }
        ForkMode::After => {
            let anchor = extract
                .anchor_message_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("after fork requires an anchor_message_id"))?;
            let idx = anchor_index(lines, anchor)
                .ok_or_else(|| anyhow::anyhow!("anchor message not found in transcript"))?;
            conv.iter().copied().filter(|&i| i > idx).collect()
        }
        ForkMode::Selected => {
            if extract.selected_message_ids.is_empty() {
                anyhow::bail!("selected fork requires at least one message id");
            }
            let set: HashSet<&str> =
                extract.selected_message_ids.iter().map(String::as_str).collect();
            selected_indices(lines, &set)
        }
    };

    if keep_conv.is_empty() {
        anyhow::bail!("fork selection kept no messages");
    }

    let first_kept = keep_conv.iter().copied().min().unwrap_or(0);

    let mut kept: Vec<Value> = lines
        .iter()
        .enumerate()
        .filter(|&(i, l)| if is_conversation(l) { keep_conv.contains(&i) } else { i < first_kept })
        .map(|(_, l)| l.clone())
        .collect();

    // A resumable transcript must open on a user turn; drop leading assistant /
    // tool-result lines the cut may have exposed (after/selected modes).
    while kept.first().is_some_and(|l| is_conversation(l) && !is_turn_start(l)) {
        kept.remove(0);
    }

    let kept = repair_tool_pairs(kept);
    if !kept.iter().any(is_conversation) {
        anyhow::bail!("fork selection kept no usable messages after repair");
    }
    Ok(relink(kept, child_session_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(uuid: &str, parent: &str, msg_id: &str, text: &str) -> Value {
        json!({
            "type": "assistant",
            "uuid": uuid,
            "parentUuid": parent,
            "sessionId": "parent-sess",
            "message": { "id": msg_id, "role": "assistant", "content": [{"type":"text","text":text}] }
        })
    }

    fn assistant_tool(uuid: &str, parent: &str, msg_id: &str, tool_id: &str) -> Value {
        json!({
            "type": "assistant",
            "uuid": uuid,
            "parentUuid": parent,
            "sessionId": "parent-sess",
            "message": { "id": msg_id, "role": "assistant",
                "content": [{"type":"tool_use","id":tool_id,"name":"Bash","input":{}}] }
        })
    }

    fn user(uuid: &str, parent: &str, text: &str) -> Value {
        json!({
            "type": "user",
            "uuid": uuid,
            "parentUuid": parent,
            "sessionId": "parent-sess",
            "message": { "role": "user", "content": text }
        })
    }

    fn tool_result(uuid: &str, parent: &str, tool_id: &str) -> Value {
        json!({
            "type": "user",
            "uuid": uuid,
            "parentUuid": parent,
            "sessionId": "parent-sess",
            "message": { "role": "user",
                "content": [{"type":"tool_result","tool_use_id":tool_id,"content":"ok"}] }
        })
    }

    fn header() -> Value {
        json!({ "type": "custom-title", "customTitle": "x", "sessionId": "parent-sess" })
    }

    // U1 -> A(m1) -> U2 -> A(m2) -> U3 -> A(m3)
    fn chain() -> Vec<Value> {
        vec![
            header(),
            user("u1", "", "hi"),
            assistant("a1", "u1", "m1", "r1"),
            user("u2", "a1", "more"),
            assistant("a2", "u2", "m2", "r2"),
            user("u3", "a2", "again"),
            assistant("a3", "u3", "m3", "r3"),
        ]
    }

    fn extract(mode: ForkMode, anchor: Option<&str>, sel: &[&str]) -> ForkExtract {
        ForkExtract {
            mode,
            anchor_message_id: anchor.map(str::to_owned),
            selected_message_ids: sel.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn ids(lines: &[Value]) -> Vec<String> {
        lines
            .iter()
            .filter(|l| is_conversation(l))
            .map(|l| {
                message_id(l).map_or_else(
                    || {
                        l.get("message")
                            .and_then(|m| m.get("content"))
                            .and_then(Value::as_str)
                            .unwrap_or("<blocks>")
                            .to_owned()
                    },
                    str::to_owned,
                )
            })
            .collect()
    }

    #[test]
    fn up_to_keeps_prefix_and_drops_rest() {
        let out =
            slice_transcript(&chain(), &extract(ForkMode::UpTo, Some("m2"), &[]), "child").unwrap();
        // header + u1 + m1 + u2 + m2
        assert_eq!(out.len(), 5);
        assert!(message_id(out.last().unwrap()) == Some("m2"));
        // header + conversation session ids rewritten
        for l in &out {
            assert_eq!(l.get("sessionId").and_then(Value::as_str), Some("child"));
        }
        // chain: first conv line roots at null, then links linearly
        assert_eq!(out[1].get("parentUuid"), Some(&Value::Null));
        assert_eq!(
            out[2].get("parentUuid").and_then(Value::as_str),
            out[1].get("uuid").and_then(Value::as_str)
        );
    }

    #[test]
    fn after_keeps_suffix_starting_on_a_user_turn() {
        let out = slice_transcript(&chain(), &extract(ForkMode::After, Some("m1"), &[]), "child")
            .unwrap();
        // drops header (trailing meta not before first kept) + u1 + m1; keeps u2 m2 u3 m3
        assert_eq!(ids(&out), vec!["more", "m2", "again", "m3"]);
        // first conversation line opens on a user turn, re-rooted at null
        let first_conv = out.iter().find(|l| is_conversation(l)).unwrap();
        assert_eq!(first_conv.get("type").and_then(Value::as_str), Some("user"));
        assert_eq!(first_conv.get("parentUuid"), Some(&Value::Null));
    }

    #[test]
    fn selected_keeps_whole_turns_of_selected_messages() {
        let out =
            slice_transcript(&chain(), &extract(ForkMode::Selected, None, &["m1", "m3"]), "child")
                .unwrap();
        // turn(m1) = u1,a1 ; turn(m3) = u3,a3 ; turn(m2)=u2,a2 dropped
        assert_eq!(ids(&out), vec!["hi", "m1", "again", "m3"]);
    }

    #[test]
    fn up_to_strips_tool_use_without_result() {
        // U1 -> A(m1, tool t1) -> tool_result(t1) -> A(m2)
        let lines = vec![
            user("u1", "", "hi"),
            assistant_tool("a1", "u1", "m1", "t1"),
            tool_result("tr1", "a1", "t1"),
            assistant("a2", "tr1", "m2", "done"),
        ];
        // fork up_to m1: keeps u1 + a1(tool t1) but the result comes AFTER m1 and
        // is cut -> the orphan tool_use must be stripped, leaving a1 empty -> gone.
        let out =
            slice_transcript(&lines, &extract(ForkMode::UpTo, Some("m1"), &[]), "child").unwrap();
        assert_eq!(ids(&out), vec!["hi"]);
    }

    #[test]
    fn after_drops_orphan_tool_result() {
        // U1 -> A(m1, tool t1) -> tool_result(t1) -> U2 -> A(m2)
        let lines = vec![
            user("u1", "", "hi"),
            assistant_tool("a1", "u1", "m1", "t1"),
            tool_result("tr1", "a1", "t1"),
            user("u2", "tr1", "next"),
            assistant("a2", "u2", "m2", "done"),
        ];
        // fork after m1: the tool_result for t1 would be orphaned (its tool_use is
        // dropped) -> the leading tool-result line is discarded, opening on U2.
        let out =
            slice_transcript(&lines, &extract(ForkMode::After, Some("m1"), &[]), "child").unwrap();
        assert_eq!(ids(&out), vec!["next", "m2"]);
        assert_eq!(out.first().unwrap().get("type").and_then(Value::as_str), Some("user"));
    }

    #[test]
    fn unknown_anchor_errors() {
        assert!(
            slice_transcript(&chain(), &extract(ForkMode::UpTo, Some("nope"), &[]), "child")
                .is_err()
        );
    }

    #[test]
    fn empty_selection_errors() {
        assert!(
            slice_transcript(&chain(), &extract(ForkMode::Selected, None, &[]), "child").is_err()
        );
    }
}
