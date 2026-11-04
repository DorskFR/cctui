//! Per-session `<redacted>` values, so an agent echoing one is masked too. A
//! daemon restart forgets them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use cctui_crypto::redact::{self, UserLiterals};
use cctui_proto::adapter::{AdapterCommand, AdapterEvent};

/// Keyed by `local_id`, or by the pre-minted spawn key until `SessionStarted`
/// names the session.
static ECHOES: OnceLock<Mutex<HashMap<String, UserLiterals>>> = OnceLock::new();

fn echoes() -> std::sync::MutexGuard<'static, HashMap<String, UserLiterals>> {
    ECHOES.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(PoisonError::into_inner)
}

fn remember(key: &str, text: &str) {
    let mut found = UserLiterals::new();
    found.extend_from_text(text);
    if !found.is_empty() {
        echoes().entry(key.to_owned()).or_default().merge(found);
    }
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
        _ => {}
    }
}

fn observe(event: &AdapterEvent) {
    match event {
        AdapterEvent::SessionStarted { local_id, meta } => {
            let Some(key) = meta.extra.get("spawn_key").and_then(serde_json::Value::as_str) else {
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
        }
        _ => {}
    }
}

fn literals_of(local_id: &str) -> UserLiterals {
    echoes().get(local_id).cloned().unwrap_or_default()
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
        other => other,
    };
    let Ok(mut value) = serde_json::to_value(&event) else { return event };
    let literals = literals_of(
        value.get("local_id").and_then(serde_json::Value::as_str).unwrap_or_default(),
    );
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
