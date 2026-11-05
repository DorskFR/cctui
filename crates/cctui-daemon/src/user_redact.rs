//! Per-session `<redacted>` values, so an agent echoing one is masked too. A
//! daemon restart forgets them.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cctui_crypto::redact::{self, UserLiterals};
use cctui_proto::adapter::{AdapterCommand, AdapterEvent};

/// Keyed by `local_id`, or by the pre-minted spawn key until `SessionStarted`
/// names the session.
static ECHOES: OnceLock<Mutex<HashMap<String, UserLiterals>>> = OnceLock::new();

/// Text held back from a session's delta stream, with the item it belongs to.
static HELD: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();

/// Values of spawns sent without a pre-minted id, waiting for a session.
static UNKEYED: OnceLock<Mutex<Vec<Unkeyed>>> = OnceLock::new();

/// How long an un-keyed spawn's values wait for their `SessionStarted`.
const UNKEYED_TTL: Duration = Duration::from_mins(5);
const UNKEYED_MAX: usize = 64;

struct Unkeyed {
    working_dir: Option<String>,
    at: Instant,
    literals: UserLiterals,
}

fn lock<T>(cell: &'static OnceLock<Mutex<T>>, init: fn() -> T) -> MutexGuard<'static, T> {
    cell.get_or_init(|| Mutex::new(init())).lock().unwrap_or_else(PoisonError::into_inner)
}

fn echoes() -> MutexGuard<'static, HashMap<String, UserLiterals>> {
    lock(&ECHOES, HashMap::new)
}

fn held() -> MutexGuard<'static, HashMap<String, (String, String)>> {
    lock(&HELD, HashMap::new)
}

fn unkeyed() -> MutexGuard<'static, Vec<Unkeyed>> {
    lock(&UNKEYED, Vec::new)
}

fn learn(key: &str, found: UserLiterals) {
    if !found.is_empty() {
        echoes().entry(key.to_owned()).or_default().merge(found);
    }
}

fn remember(key: &str, text: &str) {
    let mut found = UserLiterals::new();
    found.extend_from_text(text);
    learn(key, found);
}

fn remember_unkeyed(working_dir: Option<&str>, text: &str) {
    let mut literals = UserLiterals::new();
    literals.extend_from_text(text);
    if literals.is_empty() {
        return;
    }
    let entry =
        Unkeyed { working_dir: working_dir.map(str::to_owned), at: Instant::now(), literals };
    let mut pending = unkeyed();
    pending.retain(|u| u.at.elapsed() < UNKEYED_TTL);
    if pending.len() >= UNKEYED_MAX {
        pending.remove(0);
    }
    pending.push(entry);
}

/// A session that started without a spawn key takes the values of every
/// recent un-keyed spawn that could be it. A wrong guess only over-masks.
fn claim_unkeyed(local_id: &str, working_dir: Option<&str>) {
    let mut found = UserLiterals::new();
    {
        let mut pending = unkeyed();
        pending.retain(|u| u.at.elapsed() < UNKEYED_TTL);
        for u in &*pending {
            if u.working_dir.is_none() || u.working_dir.as_deref() == working_dir {
                found.merge(u.literals.clone());
            }
        }
    }
    learn(local_id, found);
}

pub fn note_command(command: &AdapterCommand) {
    match command {
        AdapterCommand::SendMessage { local_id, text }
        | AdapterCommand::Reply { local_id, text, .. } => remember(local_id, text),
        AdapterCommand::Spawn { spec, session_id: Some(key), .. } => {
            if let Some(prompt) = &spec.prompt {
                remember(&key.to_string(), prompt);
            }
        }
        AdapterCommand::Fork { spec, session_id: Some(key), .. } => {
            if let Some(prompt) = &spec.prompt {
                remember(key, prompt);
            }
        }
        AdapterCommand::Spawn { spec, session_id: None, .. }
        | AdapterCommand::Fork { spec, session_id: None, .. } => {
            if let Some(prompt) = &spec.prompt {
                remember_unkeyed(spec.working_dir.as_deref(), prompt);
            }
        }
        _ => {}
    }
}

fn observe(event: &AdapterEvent) {
    match event {
        AdapterEvent::SessionStarted { local_id, meta } => {
            let Some(key) = meta.extra.get("spawn_key").and_then(serde_json::Value::as_str) else {
                claim_unkeyed(local_id, meta.working_dir.as_deref());
                return;
            };
            if key == local_id {
                return;
            }
            let mut map = echoes();
            if let Some(found) = map.remove(key) {
                map.entry(local_id.clone()).or_default().merge(found);
            }
        }
        AdapterEvent::SessionEnded { local_id, .. } => {
            echoes().remove(local_id);
            held().remove(local_id);
        }
        AdapterEvent::TurnEnd { local_id, .. } => {
            held().remove(local_id);
        }
        _ => {}
    }
}

fn literals_of(local_id: &str) -> UserLiterals {
    echoes().get(local_id).cloned().unwrap_or_default()
}

/// Every live session's values: for text no session owns, like the
/// diagnostic traffic rings.
fn all_literals() -> UserLiterals {
    let mut all = UserLiterals::new();
    for literals in echoes().values() {
        all.merge(literals.clone());
    }
    all
}

/// `text` with tagged spans and every session's echoes masked.
#[must_use]
pub fn mask_any(text: &str) -> std::borrow::Cow<'_, str> {
    let (out, _) = redact::mask_user_text(text, &all_literals());
    out
}

/// The part of `delta` that cannot be the start of a tag or an echo split
/// across chunks; the rest waits in front of the item's next delta.
fn mask_delta(local_id: &str, item_id: &str, delta: String) -> String {
    let literals = literals_of(local_id);
    let previous = held().remove(local_id);
    let carry = match previous {
        Some((item, text)) if item == item_id => text,
        _ => String::new(),
    };
    if carry.is_empty() && literals.is_empty() && !delta.contains('<') {
        return delta;
    }
    let (emit, hold) = redact::split_streamed(&(carry + &delta), &literals);
    if !hold.is_empty() {
        held().insert(local_id.to_owned(), (item_id.to_owned(), hold));
    }
    emit
}

fn mask_pty(local_id: &str, data: String) -> String {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let Ok(text) =
        engine.decode(&data).map_err(drop).and_then(|b| String::from_utf8(b).map_err(drop))
    else {
        return data;
    };
    let (out, n) = redact::mask_user_text(&text, &literals_of(local_id));
    if n == 0 { data } else { engine.encode(out.as_bytes()) }
}

/// Runs whatever the scrubber config and allowlist say.
pub fn mask_event(event: AdapterEvent) -> AdapterEvent {
    observe(&event);
    let event = match event {
        AdapterEvent::PtyChunk { local_id, data } => {
            let data = mask_pty(&local_id, data);
            return AdapterEvent::PtyChunk { local_id, data };
        }
        AdapterEvent::TextDelta { local_id, item_id, delta } => {
            let delta = mask_delta(&local_id, &item_id, delta);
            return AdapterEvent::TextDelta { local_id, item_id, delta };
        }
        other => other,
    };
    let Ok(mut value) = serde_json::to_value(&event) else { return event };
    let local_id = value.get("local_id").and_then(serde_json::Value::as_str).unwrap_or_default();
    if !local_id.is_empty() {
        let mut found = UserLiterals::new();
        redact::collect_user_literals(&value, &mut found);
        learn(local_id, found);
    }
    let literals = literals_of(local_id);
    if redact::mask_user_json(&mut value, &literals) == 0 {
        return event;
    }
    if let Ok(masked) = serde_json::from_value(value) {
        return masked;
    }
    tracing::warn!("echo masking broke the event shape; masking tagged spans only");
    let Ok(mut spans_only) = serde_json::to_value(&event) else { return event };
    redact::mask_user_json(&mut spans_only, &UserLiterals::new());
    serde_json::from_value(spans_only).unwrap_or(event)
}

#[cfg(test)]
mod tests;
