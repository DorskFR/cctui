//! Usage ticker: a `<system-reminder>` appended to the last user message of a
//! proxied turn whenever one of the account's usage windows crossed a new
//! `step_pct` bucket since the last notice to that session. Off by default;
//! any failure forwards the body untouched.

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, Utc};
use dashmap::DashMap;

use super::{Account, session_id_for_token, usage_for_soft_limit};
use crate::soft_limit::{SoftLimits, UsageWindow};
use crate::state::AppState;

pub const DEFAULT_STEP_PCT: u32 = 10;

/// `{ "enabled": bool, "step_pct": int }` on the provider row. NULL ⇒ off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsageNotices {
    pub enabled: bool,
    pub step_pct: u32,
}

impl Default for UsageNotices {
    fn default() -> Self {
        Self { enabled: false, step_pct: DEFAULT_STEP_PCT }
    }
}

impl UsageNotices {
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let obj = value.and_then(serde_json::Value::as_object);
        let enabled = obj
            .and_then(|o| o.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let step_pct = obj
            .and_then(|o| o.get("step_pct"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=100).contains(n))
            .unwrap_or(DEFAULT_STEP_PCT);
        Self { enabled, step_pct }
    }

    /// Validate a PATCH/create payload into the stored blob; `Ok(None)` clears the
    /// column (off).
    pub fn build_json(
        value: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(v) = value.filter(|v| !v.is_null()) else { return Ok(None) };
        let Some(obj) = v.as_object() else { return Err("usage_notices must be an object".into()) };
        let enabled = match obj.get("enabled") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err("usage_notices.enabled must be a boolean".into()),
        };
        let step_pct = match obj.get("step_pct") {
            None | Some(serde_json::Value::Null) => DEFAULT_STEP_PCT,
            Some(n) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=100).contains(n))
                .ok_or_else(|| "usage_notices.step_pct must be an integer in 1..=100".to_owned())?,
        };
        if !enabled && step_pct == DEFAULT_STEP_PCT {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({ "enabled": enabled, "step_pct": step_pct })))
    }
}

/// Last bucket notified per `(session_id, window key)`.
pub type NoticeBuckets = DashMap<(String, String), u32>;

pub fn bucket(utilization: f64, step_pct: u32) -> u32 {
    let step = f64::from(step_pct.max(1));
    (utilization.max(0.0) / step).floor() as u32
}

/// Windows whose bucket moved up since the last notice, with the bucket to record
/// after delivery. A drop (window reset) lowers the recorded bucket immediately so
/// the next climb notifies again.
pub fn moved_windows<'a>(
    buckets: &NoticeBuckets,
    session_id: &str,
    windows: &'a [UsageWindow],
    step_pct: u32,
) -> Vec<(&'a UsageWindow, u32)> {
    let mut moved = Vec::new();
    for w in windows.iter().filter(|w| w.amount_usd.is_none()) {
        let now = bucket(w.utilization, step_pct);
        let key = (session_id.to_owned(), w.key.clone());
        match buckets.get(&key).map(|b| *b) {
            Some(last) if now > last => moved.push((w, now)),
            Some(last) if now < last => {
                buckets.insert(key, now);
            }
            Some(_) => {}
            None if now > 0 => moved.push((w, now)),
            None => {
                buckets.insert(key, 0);
            }
        }
    }
    moved
}

fn fmt_countdown(secs: i64) -> String {
    let mins = secs.max(0) / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h{:02}", hours, mins % 60);
    }
    format!("{}d{}h", hours / 24, hours % 24)
}

fn fmt_reset(resets_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let clock = if resets_at.date_naive() == now.date_naive() {
        resets_at.format("%H:%M UTC").to_string()
    } else {
        format!("{} {}", resets_at.weekday(), resets_at.format("%H:%M UTC"))
    };
    format!("resets {clock} in {}", fmt_countdown((resets_at - now).num_seconds()))
}

/// One line per percent window, e.g.
/// `Account usage notice: 5h window at 60 % (soft limit 98 %), resets 01:30 UTC in 1h12. Weekly (all models) at 15 %, resets Mon 09:00 UTC in 3d4h.`
pub fn notice_text(windows: &[UsageWindow], limits: &SoftLimits, now: DateTime<Utc>) -> String {
    let mut out = String::from("Account usage notice:");
    for w in windows.iter().filter(|w| w.amount_usd.is_none()) {
        let name =
            if w.kind == "session" { format!("{} window", w.label) } else { w.label.clone() };
        let _ = write!(out, " {name} at {} %", w.utilization.round() as i64);
        if let Some(cap) = limits.limits.get(&w.key).and_then(|l| l.cap_pct) {
            let _ = write!(out, " (soft limit {cap} %)");
        }
        if let Some(r) = w.resets_at {
            let _ = write!(out, ", {}", fmt_reset(r, now));
        }
        out.push('.');
    }
    out
}

fn reminder_block(text: &str) -> String {
    format!("<system-reminder>\n{text}\n</system-reminder>")
}

fn append_to_content(content: &mut serde_json::Value, text: &str, block_type: &str) -> bool {
    match content {
        serde_json::Value::String(s) => {
            s.push_str("\n\n");
            s.push_str(text);
            true
        }
        serde_json::Value::Array(items) => {
            items.push(serde_json::json!({ "type": block_type, "text": text }));
            true
        }
        _ => false,
    }
}

fn last_user_message(items: &mut [serde_json::Value]) -> Option<&mut serde_json::Value> {
    items
        .iter_mut()
        .rev()
        .find(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("user"))
}

/// Append the reminder to the last user message of an anthropic `messages` or
/// openai `input` body. `false` when the shape is unknown (body untouched).
pub fn inject(body: &mut serde_json::Value, text: &str) -> bool {
    let block = reminder_block(text);
    if let Some(messages) = body.get_mut("messages").and_then(serde_json::Value::as_array_mut) {
        return last_user_message(messages)
            .and_then(|m| m.get_mut("content"))
            .is_some_and(|c| append_to_content(c, &block, "text"));
    }
    match body.get_mut("input") {
        Some(input @ serde_json::Value::String(_)) => {
            append_to_content(input, &block, "input_text")
        }
        Some(serde_json::Value::Array(items)) => last_user_message(items)
            .and_then(|m| m.get_mut("content"))
            .is_some_and(|c| append_to_content(c, &block, "input_text")),
        _ => false,
    }
}

/// A notice due for this turn; `commit` after a successful injection.
#[derive(Debug)]
pub struct PendingNotice {
    pub text: String,
    buckets: Vec<((String, String), u32)>,
}

impl PendingNotice {
    pub fn inject(&self, body: &mut serde_json::Value) -> bool {
        inject(body, &self.text)
    }

    pub fn commit(self, state: &AppState) {
        for (key, b) in self.buckets {
            state.usage_notice_buckets.insert(key, b);
        }
    }
}

/// Resolve whether this turn owes the session a notice. Off ⇒ `None` at the
/// cost of one field read.
pub async fn pending(
    state: &AppState,
    acct: &Account,
    session_token: &str,
) -> Option<PendingNotice> {
    if !acct.usage_notices.enabled {
        return None;
    }
    let session_id = session_id_for_token(state, session_token).await?;
    let usage = usage_for_soft_limit(state, acct.id).await?;
    let windows = crate::soft_limit::normalize_usage_windows(&usage);
    let moved = moved_windows(
        &state.usage_notice_buckets,
        &session_id,
        &windows,
        acct.usage_notices.step_pct,
    );
    if moved.is_empty() {
        return None;
    }
    let buckets = moved.iter().map(|(w, b)| ((session_id.clone(), w.key.clone()), *b)).collect();
    tracing::debug!(account = %acct.id, session = %session_id, "usage notice due");
    Some(PendingNotice { text: notice_text(&windows, &acct.soft_limits, Utc::now()), buckets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn window(
        key: &str,
        kind: &str,
        label: &str,
        utilization: f64,
        resets_at: Option<DateTime<Utc>>,
    ) -> UsageWindow {
        UsageWindow {
            key: key.into(),
            kind: kind.into(),
            label: label.into(),
            utilization,
            amount_usd: None,
            resets_at,
            model_id: None,
            model_display_name: None,
        }
    }

    fn session_at(pct: f64) -> Vec<UsageWindow> {
        vec![window("session", "session", "5h", pct, None)]
    }

    #[test]
    fn notifies_after_52_and_81_only() {
        let buckets = NoticeBuckets::new();
        let notified: Vec<bool> = [4.0, 52.0, 58.0, 81.0]
            .into_iter()
            .map(|pct| {
                let windows = session_at(pct);
                let moved = moved_windows(&buckets, "s1", &windows, 10);
                for (w, b) in &moved {
                    buckets.insert(("s1".into(), w.key.clone()), *b);
                }
                !moved.is_empty()
            })
            .collect();
        assert_eq!(notified, [false, true, false, true]);
        assert_eq!(bucket(52.0, 10), 5);
        assert_eq!(bucket(81.0, 10), 8);
    }

    #[test]
    fn sessions_are_independent_and_resets_rearm() {
        let buckets = NoticeBuckets::new();
        let first = session_at(52.0);
        let m = moved_windows(&buckets, "s1", &first, 10);
        buckets.insert(("s1".into(), "session".into()), m[0].1);
        assert!(moved_windows(&buckets, "s1", &session_at(55.0), 10).is_empty());
        assert_eq!(moved_windows(&buckets, "s2", &session_at(55.0), 10).len(), 1);
        assert!(moved_windows(&buckets, "s1", &session_at(3.0), 10).is_empty());
        assert_eq!(moved_windows(&buckets, "s1", &session_at(12.0), 10).len(), 1);
    }

    #[test]
    fn usd_windows_never_tick() {
        let buckets = NoticeBuckets::new();
        let w = crate::soft_limit::usd_window(crate::soft_limit::KEY_SESSION_USD, 5.0, None);
        assert!(moved_windows(&buckets, "s1", &[w], 10).is_empty());
    }

    #[test]
    fn message_text_format() {
        let now = Utc.with_ymd_and_hms(2026, 9, 5, 0, 18, 0).unwrap();
        let windows = [
            window(
                "session",
                "session",
                "5h",
                60.2,
                Some(Utc.with_ymd_and_hms(2026, 9, 5, 1, 30, 0).unwrap()),
            ),
            window(
                "weekly_all",
                "weekly_all",
                "Weekly (all models)",
                15.0,
                Some(Utc.with_ymd_and_hms(2026, 9, 7, 9, 0, 0).unwrap()),
            ),
            window("weekly_model:fable", "weekly_scoped", "Weekly Fable", 30.0, None),
        ];
        let limits =
            SoftLimits::from_json(Some(&serde_json::json!({ "session": { "cap_pct": 98 } })));
        assert_eq!(
            notice_text(&windows, &limits, now),
            "Account usage notice: 5h window at 60 % (soft limit 98 %), resets 01:30 UTC in 1h12. \
             Weekly (all models) at 15 %, resets Mon 09:00 UTC in 2d8h. Weekly Fable at 30 %."
        );
    }

    #[test]
    fn injects_into_anthropic_messages() {
        let mut body = serde_json::json!({
            "model": "claude",
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": [{ "type": "text", "text": "yo" }] },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] }
            ]
        });
        assert!(inject(&mut body, "N"));
        let last = &body["messages"][2]["content"];
        assert_eq!(last.as_array().unwrap().len(), 2);
        assert_eq!(
            last[1],
            serde_json::json!({ "type": "text", "text": "<system-reminder>\nN\n</system-reminder>" })
        );
        assert_eq!(body["messages"][0]["content"], "hi");

        let mut plain = serde_json::json!({ "messages": [{ "role": "user", "content": "hi" }] });
        assert!(inject(&mut plain, "N"));
        assert_eq!(
            plain["messages"][0]["content"],
            "hi\n\n<system-reminder>\nN\n</system-reminder>"
        );
    }

    #[test]
    fn injects_into_openai_responses() {
        let mut body = serde_json::json!({
            "model": "gpt",
            "input": [
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
                { "type": "function_call_output", "call_id": "c", "output": "ok" }
            ]
        });
        assert!(inject(&mut body, "N"));
        assert_eq!(
            body["input"][0]["content"][1],
            serde_json::json!({ "type": "input_text", "text": "<system-reminder>\nN\n</system-reminder>" })
        );
        let mut plain = serde_json::json!({ "input": "hi" });
        assert!(inject(&mut plain, "N"));
        assert_eq!(plain["input"], "hi\n\n<system-reminder>\nN\n</system-reminder>");
    }

    #[test]
    fn unknown_shapes_are_left_alone() {
        let original = serde_json::json!({ "prompt": "hi", "input": 3 });
        let mut body = original.clone();
        assert!(!inject(&mut body, "N"));
        assert_eq!(body, original);
        let mut no_user =
            serde_json::json!({ "messages": [{ "role": "assistant", "content": "x" }] });
        assert!(!inject(&mut no_user, "N"));
    }

    #[test]
    fn setting_parses_and_validates() {
        assert_eq!(UsageNotices::from_json(None), UsageNotices::default());
        assert!(!UsageNotices::default().enabled);
        let on =
            UsageNotices::from_json(Some(&serde_json::json!({ "enabled": true, "step_pct": 25 })));
        assert_eq!(on, UsageNotices { enabled: true, step_pct: 25 });
        assert_eq!(
            UsageNotices::from_json(Some(&serde_json::json!({ "enabled": true, "step_pct": 0 })))
                .step_pct,
            DEFAULT_STEP_PCT
        );
        assert_eq!(UsageNotices::build_json(None).unwrap(), None);
        assert_eq!(
            UsageNotices::build_json(Some(&serde_json::json!({ "enabled": false }))).unwrap(),
            None
        );
        assert_eq!(
            UsageNotices::build_json(Some(&serde_json::json!({ "enabled": true }))).unwrap(),
            Some(serde_json::json!({ "enabled": true, "step_pct": 10 }))
        );
        assert!(
            UsageNotices::build_json(Some(
                &serde_json::json!({ "enabled": true, "step_pct": 101 })
            ))
            .is_err()
        );
        assert!(UsageNotices::build_json(Some(&serde_json::json!([]))).is_err());
    }
}
