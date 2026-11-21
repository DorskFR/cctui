//! The one owner of storage size queries and of the tool-output lifecycle.

mod job;
mod lifecycle;
mod sizes;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

pub use job::{NAME, task};
pub use lifecycle::{info, preview, save_rules, validate};
pub use sizes::overview;

/// Must select exactly the rows [`lifecycle::slots`] rewrites.
const TOOL_RESULT: &str = "((e.event_type = 'tool_use' \
       AND (e.payload->>'kind' IN ('tool_result', 'server_tool_result') \
            OR e.payload->>'type' = 'tool_result')) \
    OR (e.event_type = 'message' AND e.payload->>'type' = 'response_item' \
       AND e.payload->'payload'->>'type' IN ('function_call_output', 'custom_tool_call_output')))";

const SLOTS: [&str; 3] =
    ["e.payload->'content'", "e.payload->'output_summary'", "e.payload->'payload'->'output'"];

/// Must count like [`lifecycle::text_len`].
fn slot_len(slot: &str) -> String {
    format!(
        "COALESCE(CASE jsonb_typeof(({slot})) \
           WHEN 'string' THEN octet_length(({slot}) #>> '{{}}') \
           WHEN 'array' THEN \
             (SELECT sum(octet_length(b->>'text')) FROM jsonb_array_elements(({slot})) b) \
           WHEN 'null' THEN 0 \
           ELSE octet_length(({slot})::text) END, 0)"
    )
}

static OUTPUT_LEN: LazyLock<String> =
    LazyLock::new(|| SLOTS.iter().map(|s| slot_len(s)).collect::<Vec<_>>().join(" + "));

fn excess_over(limit: &str) -> String {
    SLOTS
        .iter()
        .map(|s| format!("GREATEST({} - {limit}, 0)", slot_len(s)))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Caps the heap pages a sampled query reads.
const SAMPLE_PAGES: f64 = 2000.0;

/// Every query of one page load samples the same blocks.
const SAMPLE_SEED: &str = "REPEATABLE (7)";

fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}
