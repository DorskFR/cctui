//! Dollar cost of recorded token usage, priced from the account's own model
//! catalog, plus the parsing of what a Fireworks response reports about usage.
//!
//! Rates live on the provider row's catalog
//! (`price_{input,cached_input,output}_per_mtok`), so a price correction is a
//! data edit — nothing here hardcodes a model or a rate. A model with no catalog
//! entry (or no prices) contributes nothing: a budget must never be inflated by
//! a guess.

/// One request's token usage, already split into billed buckets. `input`
/// excludes `cached_input` — they are priced at different rates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: i64,
    pub cached_input: i64,
    pub output: i64,
}

impl TokenUsage {
    pub const fn is_empty(&self) -> bool {
        self.input == 0 && self.cached_input == 0 && self.output == 0
    }
}

/// Usage observed for one upstream response, with the provider's own message id
/// when it exposed one (the idempotency key for `session_token_usage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedUsage {
    pub message_id: Option<String>,
    pub usage: TokenUsage,
}

/// Per-million-token USD rates for one catalog model.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
}

/// Look up a model's rates in an account's catalog (the provider row's `models`
/// JSONB). Matches the full model id first, then the last path segment on either
/// side, so `accounts/fireworks/models/kimi-k3` and `kimi-k3` resolve to the same
/// entry. `None` ⇒ unknown or unpriced model.
pub fn price_for_model(catalog: Option<&serde_json::Value>, model: &str) -> Option<ModelPrice> {
    let entries = catalog?.as_array()?;
    let want = model.trim();
    if want.is_empty() {
        return None;
    }
    let entry = entries
        .iter()
        .find(|e| e.get("model").and_then(serde_json::Value::as_str) == Some(want))
        .or_else(|| {
            entries.iter().find(|e| {
                e.get("model")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|m| tail(m).eq_ignore_ascii_case(tail(want)))
            })
        })?;
    let rate = |k: &str| entry.get(k).and_then(serde_json::Value::as_f64);
    let (input, cached_input, output) = (
        rate("price_input_per_mtok"),
        rate("price_cached_input_per_mtok"),
        rate("price_output_per_mtok"),
    );
    if input.is_none() && cached_input.is_none() && output.is_none() {
        return None;
    }
    Some(ModelPrice {
        input: input.unwrap_or_default(),
        cached_input: cached_input.unwrap_or_default(),
        output: output.unwrap_or_default(),
    })
}

fn tail(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// USD cost of one usage tally at the given rates.
pub fn usage_cost_usd(u: &TokenUsage, p: &ModelPrice) -> f64 {
    (u.input as f64).mul_add(
        p.input,
        (u.cached_input as f64).mul_add(p.cached_input, u.output as f64 * p.output),
    ) / 1_000_000.0
}

/// Total USD for a set of per-model tallies. Rows whose model is unknown to the
/// catalog are skipped (see the module docs).
pub fn tallies_cost_usd(
    catalog: Option<&serde_json::Value>,
    rows: &[(Option<String>, TokenUsage)],
) -> f64 {
    rows.iter()
        .filter_map(|(model, usage)| {
            let price = price_for_model(catalog, model.as_deref()?)?;
            Some(usage_cost_usd(usage, &price))
        })
        .sum()
}

/// Parse the usage a Fireworks response reports.
///
/// Handles both a plain JSON completion body and an SSE stream (the last `data:`
/// frame carrying a non-null `usage` wins — Fireworks puts the final tally on the
/// terminal chunk). The two response headers, when present, override the body's
/// prompt/cached split: they are what the provider actually meters. Returns
/// `None` when neither source reports anything.
pub fn parse_fireworks_usage(
    body: &[u8],
    prompt_header: Option<&str>,
    cached_header: Option<&str>,
) -> Option<CapturedUsage> {
    let parsed = parse_body(body);
    let (mut prompt, mut cached, output) = parsed.as_ref().map_or((None, None, 0), |b| {
        (b.prompt_tokens, b.cached_tokens, b.completion_tokens.unwrap_or(0))
    });
    if let Some(n) = prompt_header.and_then(parse_count) {
        prompt = Some(n);
    }
    if let Some(n) = cached_header.and_then(parse_count) {
        cached = Some(n);
    }
    let prompt_total = prompt.unwrap_or(0);
    let cached_input = cached.unwrap_or(0).clamp(0, prompt_total.max(0));
    let usage = TokenUsage {
        input: (prompt_total - cached_input).max(0),
        cached_input,
        output: output.max(0),
    };
    if usage.is_empty() {
        return None;
    }
    Some(CapturedUsage { message_id: parsed.and_then(|b| b.id), usage })
}

fn parse_count(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok().filter(|n| *n >= 0)
}

struct BodyUsage {
    id: Option<String>,
    prompt_tokens: Option<i64>,
    cached_tokens: Option<i64>,
    completion_tokens: Option<i64>,
}

fn parse_body(body: &[u8]) -> Option<BodyUsage> {
    if let Some(v) = serde_json::from_slice::<serde_json::Value>(body).ok().as_ref()
        && let Some(u) = body_usage(v)
    {
        return Some(u);
    }
    let text = std::str::from_utf8(body).ok()?;
    let mut last = None;
    for line in text.lines() {
        let Some(payload) = line.strip_prefix("data:").map(str::trim) else { continue };
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else { continue };
        if let Some(u) = body_usage(&v) {
            last = Some(u);
        }
    }
    last
}

/// One JSON object → its usage, if it carries a non-null `usage` block.
fn body_usage(v: &serde_json::Value) -> Option<BodyUsage> {
    let usage = v.get("usage").filter(|u| !u.is_null())?;
    let num = |k: &str| usage.get(k).and_then(serde_json::Value::as_i64);
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(serde_json::Value::as_i64);
    let prompt = num("prompt_tokens");
    let completion = num("completion_tokens");
    if prompt.is_none() && completion.is_none() {
        return None;
    }
    Some(BodyUsage {
        id: v.get("id").and_then(serde_json::Value::as_str).map(str::to_owned),
        prompt_tokens: prompt,
        cached_tokens: cached,
        completion_tokens: completion,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> serde_json::Value {
        json!([{
            "model": "accounts/fireworks/models/kimi-k3",
            "label": "Kimi K3",
            "price_input_per_mtok": 3.0,
            "price_cached_input_per_mtok": 0.3,
            "price_output_per_mtok": 15.0,
        }, {
            "model": "accounts/fireworks/models/unpriced",
            "label": "Unpriced",
        }])
    }

    #[test]
    fn price_matches_full_id_and_bare_name() {
        let c = catalog();
        let full = price_for_model(Some(&c), "accounts/fireworks/models/kimi-k3").unwrap();
        let bare = price_for_model(Some(&c), "kimi-k3").unwrap();
        assert_eq!(full, bare);
        assert!((full.cached_input - 0.3).abs() < 1e-9);
    }

    #[test]
    fn unknown_or_unpriced_model_has_no_price() {
        let c = catalog();
        assert!(price_for_model(Some(&c), "nope").is_none());
        assert!(price_for_model(Some(&c), "accounts/fireworks/models/unpriced").is_none());
        assert!(price_for_model(None, "kimi-k3").is_none());
    }

    #[test]
    fn cost_prices_cached_input_at_the_cached_rate() {
        let p = price_for_model(Some(&catalog()), "kimi-k3").unwrap();
        let u = TokenUsage { input: 1_000_000, cached_input: 1_000_000, output: 1_000_000 };
        // 3.00 + 0.30 + 15.00
        assert!((usage_cost_usd(&u, &p) - 18.3).abs() < 1e-9);
    }

    #[test]
    fn tallies_skip_models_absent_from_the_catalog() {
        let c = catalog();
        let rows = vec![
            (Some("kimi-k3".to_owned()), TokenUsage { input: 2_000_000, ..TokenUsage::default() }),
            (Some("ghost".to_owned()), TokenUsage { output: 9_000_000, ..TokenUsage::default() }),
            (None, TokenUsage { output: 9_000_000, ..TokenUsage::default() }),
        ];
        assert!((tallies_cost_usd(Some(&c), &rows) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn json_body_usage_splits_cached_input() {
        let body = json!({
            "id": "cmpl-1",
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 200,
                "prompt_tokens_details": {"cached_tokens": 400},
            }
        })
        .to_string();
        let got = parse_fireworks_usage(body.as_bytes(), None, None).unwrap();
        assert_eq!(got.message_id.as_deref(), Some("cmpl-1"));
        assert_eq!(got.usage, TokenUsage { input: 600, cached_input: 400, output: 200 });
    }

    #[test]
    fn sse_final_chunk_wins() {
        let sse = "data: {\"id\":\"c1\",\"usage\":null,\"choices\":[]}\n\n\
                   data: {\"id\":\"c1\",\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n\n\
                   data: {\"id\":\"c1\",\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":50,\
                   \"prompt_tokens_details\":{\"cached_tokens\":30}}}\n\n\
                   data: [DONE]\n\n";
        let got = parse_fireworks_usage(sse.as_bytes(), None, None).unwrap();
        assert_eq!(got.usage, TokenUsage { input: 70, cached_input: 30, output: 50 });
    }

    #[test]
    fn headers_override_the_body_split() {
        let body = json!({"usage": {"prompt_tokens": 100, "completion_tokens": 50}}).to_string();
        let got = parse_fireworks_usage(body.as_bytes(), Some("120"), Some("90")).unwrap();
        assert_eq!(got.usage, TokenUsage { input: 30, cached_input: 90, output: 50 });
    }

    #[test]
    fn headers_alone_still_record_usage() {
        let got = parse_fireworks_usage(b"", Some("500"), Some("100")).unwrap();
        assert_eq!(got.usage, TokenUsage { input: 400, cached_input: 100, output: 0 });
        assert!(got.message_id.is_none());
    }

    #[test]
    fn cached_never_exceeds_prompt_and_nothing_reported_is_none() {
        let got = parse_fireworks_usage(b"", Some("100"), Some("400")).unwrap();
        assert_eq!(got.usage, TokenUsage { input: 0, cached_input: 100, output: 0 });
        assert!(parse_fireworks_usage(b"not json", None, None).is_none());
        assert!(parse_fireworks_usage(b"{\"usage\":null}", None, None).is_none());
    }
}
