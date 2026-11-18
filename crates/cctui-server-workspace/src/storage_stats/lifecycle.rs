use cctui_proto::api::{
    StorageDropRule, StorageLifecycleInfo, StorageLifecyclePreview, StorageLifecycleRules,
    StorageLifecycleRun, StorageTrimRule,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{SAMPLE_SEED, TOOL_RESULT, to_u64};

pub const MARK: &str = "cctui_lifecycle";
pub const TRIM_NOTE: &str = "\n… [trimmed by the storage lifecycle]";
pub const MAX_DAYS: u32 = 3650;
pub const MIN_ABOVE_BYTES: u64 = 1024;
pub const MAX_ABOVE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Trim { after_days: u32, above_bytes: u64 },
    Drop { after_days: u32 },
}

impl Rule {
    pub const fn action(self) -> &'static str {
        match self {
            Self::Trim { .. } => "trim",
            Self::Drop { .. } => "drop",
        }
    }

    pub const fn after_days(self) -> u32 {
        match self {
            Self::Trim { after_days, .. } | Self::Drop { after_days } => after_days,
        }
    }

    pub fn fingerprint(self) -> String {
        match self {
            Self::Trim { after_days, above_bytes } => format!("trim:{after_days}:{above_bytes}"),
            Self::Drop { after_days } => format!("drop:{after_days}"),
        }
    }
}

/// Drop runs first, so trim does not spend a pass on rows about to be dropped.
pub fn active(rules: &StorageLifecycleRules) -> Vec<Rule> {
    let drop = rules.drop.filter(|d| d.enabled).map(|d| Rule::Drop { after_days: d.after_days });
    let trim = rules.trim.filter(|t| t.enabled).map(|t| Rule::Trim {
        after_days: t.after_days,
        above_bytes: t.above_bytes,
    });
    drop.into_iter().chain(trim).collect()
}

pub fn validate(rules: StorageLifecycleRules) -> Result<StorageLifecycleRules, String> {
    let days = |what: &str, d: u32| {
        if (1..=MAX_DAYS).contains(&d) {
            Ok(())
        } else {
            Err(format!("{what}: after_days must be between 1 and {MAX_DAYS}"))
        }
    };
    if let Some(t) = rules.trim {
        days("trim", t.after_days)?;
        if !(MIN_ABOVE_BYTES..=MAX_ABOVE_BYTES).contains(&t.above_bytes) {
            return Err(format!(
                "trim: above_bytes must be between {MIN_ABOVE_BYTES} and {MAX_ABOVE_BYTES}"
            ));
        }
    }
    if let Some(d) = rules.drop {
        days("drop", d.after_days)?;
    }
    Ok(rules)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Top(&'static str),
    Codex,
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Conversation text never has a slot.
fn slots(event_type: &str, payload: &Value) -> &'static [Slot] {
    let tool_result = matches!(str_at(payload, "kind"), Some("tool_result" | "server_tool_result"))
        || str_at(payload, "type") == Some("tool_result");
    let codex_output = str_at(payload, "type") == Some("response_item")
        && matches!(
            payload.get("payload").and_then(|p| str_at(p, "type")),
            Some("function_call_output" | "custom_tool_call_output")
        );
    match event_type {
        "tool_use" if tool_result => &[Slot::Top("content"), Slot::Top("output_summary")],
        "message" if codex_output => &[Slot::Codex],
        _ => &[],
    }
}

fn slot_mut(payload: &mut Value, slot: Slot) -> Option<&mut Value> {
    match slot {
        Slot::Top(key) => payload.get_mut(key),
        Slot::Codex => payload.get_mut("payload")?.get_mut("output"),
    }
}

fn slot_ref(payload: &Value, slot: Slot) -> Option<&Value> {
    match slot {
        Slot::Top(key) => payload.get(key),
        Slot::Codex => payload.get("payload")?.get("output"),
    }
}

pub fn text_len(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| str_at(b, "text")).map(str::len).sum(),
        Value::Null => 0,
        other => other.to_string().len(),
    }
}

fn floor_char(s: &str, max: usize) -> usize {
    (0..=max.min(s.len())).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

fn head(s: &str, budget: usize) -> String {
    format!("{}{TRIM_NOTE}", &s[..floor_char(s, budget)])
}

/// At most `above` bytes, marker included, so a second pass finds nothing to trim.
fn trimmed(v: &Value, above: usize) -> Value {
    let mut budget = above.saturating_sub(TRIM_NOTE.len());
    let Value::Array(blocks) = v else {
        let text = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
        return Value::String(head(&text, budget));
    };
    let mut kept = Vec::with_capacity(blocks.len() + 1);
    for block in blocks {
        let Some(text) = str_at(block, "text") else {
            kept.push(block.clone());
            continue;
        };
        if budget == 0 {
            continue;
        }
        let cut = floor_char(text, budget);
        let mut block = block.clone();
        block["text"] = Value::String(text[..cut].to_owned());
        budget -= cut;
        if cut < text.len() {
            budget = 0;
        }
        kept.push(block);
    }
    kept.push(json!({ "type": "text", "text": TRIM_NOTE.trim_start() }));
    Value::Array(kept)
}

pub fn dropped_text(original: u64) -> String {
    format!("[tool output removed by the storage lifecycle: {original} bytes]")
}

pub fn apply(rule: Rule, event_type: &str, payload: &Value) -> Option<Value> {
    let slots = slots(event_type, payload);
    let total: usize = slots.iter().filter_map(|s| slot_ref(payload, *s)).map(text_len).sum();
    if total == 0 {
        return None;
    }
    let mark = payload.get(MARK);
    let done = mark.and_then(|m| str_at(m, "action"));
    let original = mark
        .and_then(|m| m.get("original_bytes"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| u64::try_from(total).unwrap_or(u64::MAX));
    let mut out = payload.clone();
    let changed = match rule {
        Rule::Drop { .. } if done == Some("drop") => false,
        Rule::Drop { .. } => {
            for slot in slots {
                if let Some(v) = slot_mut(&mut out, *slot) {
                    *v = Value::String(dropped_text(original));
                }
            }
            true
        }
        Rule::Trim { above_bytes, .. } => {
            let above = usize::try_from(above_bytes).unwrap_or(usize::MAX);
            let mut changed = false;
            for slot in slots {
                if let Some(v) = slot_mut(&mut out, *slot)
                    && text_len(v) > above
                {
                    *v = trimmed(v, above);
                    changed = true;
                }
            }
            changed
        }
    };
    if !changed {
        return None;
    }
    let obj = out.as_object_mut()?;
    obj.insert(MARK.to_owned(), json!({ "action": rule.action(), "original_bytes": original }));
    Some(out)
}

pub async fn load_rules(pool: &PgPool) -> Result<StorageLifecycleRules, sqlx::Error> {
    let rows: Vec<(String, bool, i32, Option<i64>)> = sqlx::query_as(
        "SELECT action, enabled, after_days, above_bytes FROM storage_lifecycle_rules",
    )
    .fetch_all(pool)
    .await?;
    let mut rules = StorageLifecycleRules::default();
    for (action, enabled, after_days, above) in rows {
        let after_days = u32::try_from(after_days).unwrap_or(1);
        match (action.as_str(), above) {
            ("trim", Some(above)) => {
                rules.trim =
                    Some(StorageTrimRule { enabled, after_days, above_bytes: to_u64(above) });
            }
            ("drop", _) => rules.drop = Some(StorageDropRule { enabled, after_days }),
            _ => {}
        }
    }
    Ok(rules)
}

pub async fn save_rules(
    pool: &PgPool,
    rules: StorageLifecycleRules,
    by: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM storage_lifecycle_rules").execute(&mut *tx).await?;
    let rows = [
        rules.trim.map(|t| ("trim", t.enabled, t.after_days, Some(t.above_bytes))),
        rules.drop.map(|d| ("drop", d.enabled, d.after_days, None)),
    ];
    for (action, enabled, after_days, above) in rows.into_iter().flatten() {
        sqlx::query(
            "INSERT INTO storage_lifecycle_rules (action, enabled, after_days, above_bytes, \
             updated_by) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(action)
        .bind(enabled)
        .bind(i32::try_from(after_days).unwrap_or(i32::MAX))
        .bind(above.map(|b| i64::try_from(b).unwrap_or(i64::MAX)))
        .bind(by)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

type RunRow = (
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
);

fn run_of(row: RunRow) -> StorageLifecycleRun {
    let (started_at, finished_at, scanned, trimmed, dropped, skipped, freed, error) = row;
    StorageLifecycleRun {
        started_at,
        finished_at,
        rows_scanned: to_u64(scanned),
        rows_trimmed: to_u64(trimmed),
        rows_dropped: to_u64(dropped),
        rows_skipped: to_u64(skipped),
        bytes_freed: to_u64(freed),
        error,
    }
}

pub async fn info(pool: &PgPool, name: &str) -> Result<StorageLifecycleInfo, sqlx::Error> {
    let rules = load_rules(pool).await?;
    let last: Option<RunRow> = sqlx::query_as(
        "SELECT started_at, finished_at, rows_scanned, rows_trimmed, rows_dropped, \
         rows_skipped, bytes_freed, error FROM storage_lifecycle_runs \
         WHERE name = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(bytes_freed), 0)::int8 FROM storage_lifecycle_runs WHERE name = $1",
    )
    .bind(name)
    .fetch_one(pool)
    .await?;
    let last_run = last.map(run_of);
    Ok(StorageLifecycleInfo { rules, last_run, bytes_freed_total: to_u64(total) })
}

fn preview_sql() -> String {
    let len = super::OUTPUT_LEN.as_str();
    let excess = super::excess_over("$3::int8");
    format!(
        "WITH s AS ( \
           SELECT e.created_at, pg_column_size(e.payload)::float8 AS stored, \
                  GREATEST(octet_length(e.payload::text), 1)::float8 AS logical, \
                  ({len})::float8 AS len, ({excess})::float8 AS excess, \
                  (e.payload->'{MARK}'->>'action') IS DISTINCT FROM 'drop' AS droppable \
           FROM stream_events e TABLESAMPLE SYSTEM ($1::real) {SAMPLE_SEED} \
           WHERE {TOOL_RESULT} AND ($5::text[] IS NULL OR e.session_id = ANY($5))), \
         c AS ( \
           SELECT stored, logical, \
             CASE WHEN $4::int4 IS NOT NULL AND droppable AND len > 0 \
                       AND created_at < now() - make_interval(days => $4::int4) THEN 'drop' \
                  WHEN $2::int4 IS NOT NULL AND excess > 0 \
                       AND created_at < now() - make_interval(days => $2::int4) THEN 'trim' \
             END AS action, len, excess \
           FROM s) \
         SELECT round(count(*) FILTER (WHERE action = 'trim') * $6::float8)::int8, \
                round(count(*) FILTER (WHERE action = 'drop') * $6::float8)::int8, \
                round(COALESCE(sum(stored * LEAST(1, \
                    CASE action WHEN 'drop' THEN len ELSE excess END / logical)) \
                  FILTER (WHERE action IS NOT NULL), 0) * $6::float8)::int8 \
         FROM c"
    )
}

/// Previews every rule present, enabled or not: the admin checks a rule
/// before switching it on.
pub async fn preview(
    pool: &PgPool,
    rules: StorageLifecycleRules,
    scope: Option<&[String]>,
) -> Result<StorageLifecyclePreview, sqlx::Error> {
    let mut tx = pool.begin().await?;
    super::sizes::bound(&mut tx).await?;
    let percent =
        if scope.is_some() { 100.0 } else { super::sizes::sample_percent(&mut tx).await? };
    let (trimmed, dropped, freed): (i64, i64, i64) =
        sqlx::query_as(sqlx::AssertSqlSafe(preview_sql()))
            .bind(percent)
            .bind(rules.trim.map(|t| i32::try_from(t.after_days).unwrap_or(i32::MAX)))
            .bind(rules.trim.map(|t| i64::try_from(t.above_bytes).unwrap_or(i64::MAX)))
            .bind(rules.drop.map(|d| i32::try_from(d.after_days).unwrap_or(i32::MAX)))
            .bind(scope)
            .bind(100.0 / percent)
            .fetch_one(&mut *tx)
            .await?;
    tx.rollback().await?;
    Ok(StorageLifecyclePreview {
        percent,
        rows_trimmed: to_u64(trimmed),
        rows_dropped: to_u64(dropped),
        bytes_freed: to_u64(freed),
    })
}
