use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

pub use cctui_proto::api::notifications::{
    NotificationChannelPref, NotificationSettings, NotificationStage, QuietHours,
};

use super::{Push, kind};

pub mod channel {
    pub const IN_APP: &str = "in_app";
    pub const WEB_PUSH: &str = "web_push";
    pub const NTFY: &str = "ntfy";
    pub const EMAIL: &str = "email";
}

pub const CHANNELS: &[&str] = &[channel::IN_APP, channel::WEB_PUSH, channel::NTFY, channel::EMAIL];
pub const PUSH_CHANNELS: &[&str] = &[channel::WEB_PUSH, channel::NTFY, channel::EMAIL];

pub const USER_ACTION_BLOCKING: &str = "user_action_blocking";

pub const ROUTE_KINDS: &[&str] = &[
    kind::APPROVAL,
    kind::USER_ACTION,
    USER_ACTION_BLOCKING,
    kind::NEEDS_INPUT,
    kind::SESSION_STUCK,
    kind::SYSTEM_ALERT,
    kind::INBOX_STUCK,
];

pub const MAX_AFTER_MINUTES: u32 = 24 * 60;
pub const MAX_PUSH_PER_HOUR_CEILING: u32 = 1000;
const MAX_TZ_CHARS: usize = 64;

#[must_use]
pub fn route_key(notice_kind: &str, blocking: bool) -> &str {
    if notice_kind == kind::USER_ACTION && blocking { USER_ACTION_BLOCKING } else { notice_kind }
}

impl Push {
    #[must_use]
    pub const fn stage(self) -> NotificationStage {
        match self {
            Self::Off => NotificationStage::Off,
            Self::Now => NotificationStage::Now,
            Self::AfterMinutes(m) => {
                NotificationStage::After { minutes: m.unsigned_abs() }
            }
        }
    }
}

#[must_use]
pub fn default_stage(key: &str, channel: &str) -> NotificationStage {
    if !super::is_recorded(key) {
        return NotificationStage::Off;
    }
    if channel == channel::IN_APP {
        return NotificationStage::Now;
    }
    let push = if key == USER_ACTION_BLOCKING {
        Push::for_kind(kind::USER_ACTION, true)
    } else {
        Push::for_kind(key, false)
    };
    push.stage()
}

#[must_use]
pub fn default_channel_enabled(channel: &str) -> bool {
    channel != channel::EMAIL
}

#[must_use]
pub fn delay_secs(stage: NotificationStage) -> Option<i32> {
    match stage {
        NotificationStage::Off => None,
        NotificationStage::Now => Some(0),
        NotificationStage::After { minutes } => {
            let minutes = i32::try_from(minutes.min(MAX_AFTER_MINUTES)).unwrap_or(0);
            Some(minutes * 60)
        }
    }
}

#[must_use]
pub fn channel_enabled(s: &NotificationSettings, channel: &str) -> bool {
    s.channels.get(channel).map_or_else(|| default_channel_enabled(channel), |c| c.enabled)
}

#[must_use]
pub fn cell(s: &NotificationSettings, key: &str, channel: &str) -> NotificationStage {
    s.routes
        .get(key)
        .and_then(|row| row.get(channel))
        .copied()
        .unwrap_or_else(|| default_stage(key, channel))
}

#[must_use]
pub fn stage(s: &NotificationSettings, key: &str, channel: &str) -> NotificationStage {
    if !s.enabled || !channel_enabled(s, channel) {
        return NotificationStage::Off;
    }
    cell(s, key, channel)
}

#[must_use]
pub fn resolved(s: &NotificationSettings) -> NotificationSettings {
    let channels = CHANNELS
        .iter()
        .map(|&c| (c.to_owned(), NotificationChannelPref { enabled: channel_enabled(s, c) }))
        .collect();
    let routes = ROUTE_KINDS
        .iter()
        .map(|&key| {
            let row: BTreeMap<String, NotificationStage> =
                CHANNELS.iter().map(|&c| (c.to_owned(), cell(s, key, c))).collect();
            (key.to_owned(), row)
        })
        .collect();
    NotificationSettings {
        enabled: s.enabled,
        sound: s.sound,
        channels,
        routes,
        quiet_hours: s.quiet_hours.clone(),
        max_push_per_hour: s.max_push_per_hour,
    }
}

fn legacy_sound(data: &Value) -> Option<bool> {
    data.get("display")?.get("notifySound")?.as_bool()
}

#[must_use]
pub fn parse(data: &Value) -> NotificationSettings {
    let block = data.get("notifications");
    let mut s = block
        .and_then(|v| NotificationSettings::deserialize(v).ok())
        .unwrap_or_default();
    if block.and_then(|b| b.get("sound")).is_none()
        && let Some(sound) = legacy_sound(data)
    {
        s.sound = sound;
    }
    s
}

pub async fn load(conn: &mut PgConnection, user_id: Uuid) -> sqlx::Result<NotificationSettings> {
    let data: Option<Value> =
        sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(conn)
            .await?;
    Ok(data.as_ref().map_or_else(NotificationSettings::default, parse))
}

pub fn migrate_sound(data: &mut Value) {
    let Some(sound) = legacy_sound(data) else { return };
    let Some(obj) = data.as_object_mut() else { return };
    let block = obj.entry("notifications").or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(block) = block.as_object_mut() {
        block.entry("sound").or_insert(Value::Bool(sound));
    }
}

#[must_use]
pub fn parse_hhmm(raw: &str) -> Option<u32> {
    let (h, m) = raw.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn check_quiet(q: &QuietHours) -> Result<(), String> {
    for (name, value) in [("start", &q.start), ("end", &q.end)] {
        if parse_hhmm(value).is_none() {
            return Err(format!("notifications.quiet_hours.{name} must be HH:MM"));
        }
    }
    let tz_ok = !q.tz.is_empty()
        && q.tz.chars().count() <= MAX_TZ_CHARS
        && q.tz.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c));
    if tz_ok { Ok(()) } else { Err("notifications.quiet_hours.tz must be an IANA zone".into()) }
}

fn check_stage(key: &str, channel: &str, stage: NotificationStage) -> Result<(), String> {
    match stage {
        NotificationStage::After { minutes } if !(1..=MAX_AFTER_MINUTES).contains(&minutes) => {
            Err(format!(
                "notifications.routes.{key}.{channel}: after must be 1 to {MAX_AFTER_MINUTES} \
                 minutes"
            ))
        }
        _ => Ok(()),
    }
}

pub fn normalize(raw: &Value) -> Result<NotificationSettings, String> {
    let mut s = NotificationSettings::deserialize(raw)
        .map_err(|e| format!("notifications settings are malformed: {e}"))?;
    s.channels.retain(|c, _| CHANNELS.contains(&c.as_str()));
    s.routes.retain(|k, _| ROUTE_KINDS.contains(&k.as_str()));
    for (key, row) in &mut s.routes {
        row.retain(|c, _| CHANNELS.contains(&c.as_str()));
        for (channel, stage) in row.iter() {
            check_stage(key, channel, *stage)?;
        }
    }
    s.routes.retain(|_, row| !row.is_empty());
    if let Some(q) = &s.quiet_hours {
        check_quiet(q)?;
    }
    s.max_push_per_hour = s.max_push_per_hour.clamp(1, MAX_PUSH_PER_HOUR_CEILING);
    Ok(s)
}

pub async fn tz_known(conn: &mut PgConnection, tz: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)")
        .bind(tz)
        .fetch_one(conn)
        .await
}

pub async fn clamp_settings(conn: &mut PgConnection, data: &mut Value) -> Result<(), String> {
    migrate_sound(data);
    let Some(raw) = data.get("notifications") else { return Ok(()) };
    let s = normalize(raw)?;
    if let Some(q) = &s.quiet_hours {
        let known = tz_known(conn, &q.tz).await.map_err(|e| format!("time zone check: {e}"))?;
        if !known {
            return Err(format!("unknown time zone '{}'", q.tz));
        }
    }
    let value = serde_json::to_value(&s).map_err(|e| e.to_string())?;
    if let Some(obj) = data.as_object_mut() {
        obj.insert("notifications".to_owned(), value);
    }
    Ok(())
}

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;
