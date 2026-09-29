//! Prometheus text exposition of the numbers the webui already shows.
//!
//! Third-party status bars, dashboards and alerts pin to metric NAMES, so the
//! names here are an API: add freely, rename never. [`docs/metrics.md`] is the
//! contract.
//!
//! ## A scrape never touches an upstream
//!
//! Window figures come from the per-credential usage cache only. A credential
//! whose cache is empty exports `cctui_account_usage_known 0` and no window
//! series at all — it does not export zeros, which would read as a fresh,
//! wide-open quota, and it does not trigger a fetch, which would let a 15-second
//! scrape interval hammer a rate-limited upstream.
//!
//! ## Auth
//!
//! Same Bearer scheme as `/api/v1` (any user or admin token), because the output
//! names accounts and their consumption. `CCTUI_METRICS_PUBLIC=1` opts into an
//! unauthenticated endpoint for the usual scrape-inside-the-cluster case; it is
//! off by default so the endpoint cannot become accidentally public.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::soft_limit::{SoftLimits, UsageWindow, is_usd_key};
use crate::state::AppState;

/// Blended per-million rates used for the cost estimate, mirroring the accounts
/// API: (input, output, cache read, cache creation).
const OPENAI_RATES: (f64, f64, f64, f64) = (1.25, 10.0, 0.125, 1.25);
const ANTHROPIC_RATES: (f64, f64, f64, f64) = (3.0, 15.0, 0.3, 3.75);

/// Machines seen within this many seconds count as live (mirrors
/// [`crate::machine_liveness`]'s online tier).
const MACHINE_ONLINE_SECS: i64 = 5 * 60;

/// `CCTUI_METRICS_PUBLIC` — off unless explicitly turned on.
#[must_use]
pub fn public_from_env() -> bool {
    matches!(
        std::env::var("CCTUI_METRICS_PUBLIC")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "on" | "yes")
    )
}

/// One credential's identity and lifetime token tally.
#[derive(Debug, sqlx::FromRow)]
pub struct ProviderRow {
    pub id: Uuid,
    pub account_name: String,
    pub provider: String,
    pub family: String,
    pub soft_limits: Option<serde_json::Value>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
}

impl ProviderRow {
    fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_creation_tokens
    }

    /// Blended-rate cost estimate — a usage-weight signal, not a bill. A
    /// pay-per-token credential's real per-model spend lives on the accounts
    /// API, which prices it from the account's own catalog.
    fn est_cost_usd(&self) -> f64 {
        let (i, o, cr, cc) =
            if self.family == "openai" { OPENAI_RATES } else { ANTHROPIC_RATES };
        (self.input_tokens as f64 * i
            + self.output_tokens as f64 * o
            + self.cache_read_tokens as f64 * cr
            + self.cache_creation_tokens as f64 * cc)
            / 1_000_000.0
    }
}

/// Everything one scrape reports, gathered before any formatting so the
/// rendering stays pure and testable.
pub struct Snapshot {
    pub version: &'static str,
    pub providers: Vec<ProviderSnapshot>,
    /// `(status, count)` over every session row.
    pub sessions_by_status: Vec<(String, i64)>,
    pub sessions_live: i64,
    pub machines_live: i64,
}

pub struct ProviderSnapshot {
    pub row: ProviderRow,
    pub caps: SoftLimits,
    /// `None` ⇒ nothing cached: usage is unknown, not zero.
    pub windows: Option<Vec<UsageWindow>>,
    pub age_secs: Option<u64>,
    pub paces: Vec<Option<crate::pace::Pace>>,
}

/// Grouped rather than a correlated LATERAL per row: a scrape wants one pass
/// over the whole table, not one subquery per credential.
const PROVIDER_TOTALS_SQL: &str = "SELECT p.id, a.name AS account_name, p.provider, p.family, \
            p.soft_limits_json AS soft_limits, \
            COALESCE(SUM(stu.input_tokens), 0)::bigint          AS input_tokens, \
            COALESCE(SUM(stu.output_tokens), 0)::bigint         AS output_tokens, \
            COALESCE(SUM(stu.cache_read_tokens), 0)::bigint     AS cache_read_tokens, \
            COALESCE(SUM(stu.cache_creation_tokens), 0)::bigint AS cache_creation_tokens \
     FROM account_providers p \
     JOIN accounts a ON a.id = p.account_id \
     LEFT JOIN session_tokens st ON st.account_id = p.id \
     LEFT JOIN session_usage_totals stu ON stu.session_id = st.session_id \
     GROUP BY p.id, a.name, p.provider, p.family, p.soft_limits_json \
     ORDER BY a.name, p.provider";

/// Read every number a scrape needs. Each part degrades to empty on a DB error:
/// a partial scrape beats a 500, and the missing series is itself the signal.
pub async fn snapshot(state: &AppState) -> Snapshot {
    let rows: Vec<ProviderRow> =
        sqlx::query_as(PROVIDER_TOTALS_SQL).fetch_all(&state.pool).await.unwrap_or_default();
    let now = Utc::now();
    let providers = rows
        .into_iter()
        .map(|row| {
            let cached = state.account_usage_cache.get(&row.id);
            let age_secs = cached.as_ref().map(|c| c.fetched_at.elapsed().as_secs());
            let windows = cached.as_ref().and_then(|c| {
                c.usage.as_ref().map(crate::soft_limit::normalize_usage_windows)
            });
            drop(cached);
            let paces = windows
                .as_ref()
                .map(|ws| ws.iter().map(|w| crate::pace::for_window(now, w, None)).collect())
                .unwrap_or_default();
            let caps = SoftLimits::from_json(row.soft_limits.as_ref());
            ProviderSnapshot { row, caps, windows, age_secs, paces }
        })
        .collect();

    let sessions_by_status: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*)::bigint FROM sessions GROUP BY status")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();
    let machines_live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM machines \
         WHERE deleted_at IS NULL AND revoked_at IS NULL \
           AND last_seen_at > now() - make_interval(secs => $1)",
    )
    .bind(MACHINE_ONLINE_SECS as f64)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    let sessions_live = {
        let registry = state.registry.read().await;
        i64::try_from(registry.list().len()).unwrap_or(i64::MAX)
    };

    Snapshot {
        version: env!("CARGO_PKG_VERSION"),
        providers,
        sessions_by_status,
        sessions_live,
        machines_live,
    }
}

/// Escape a label value per the exposition format: backslash, double quote and
/// newline. An account name is operator-supplied text, so this is not optional.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

fn seconds_since_epoch(at: DateTime<Utc>) -> i64 {
    at.timestamp()
}

/// Render a snapshot as Prometheus text (version 0.0.4).
///
/// Every family emits its `# HELP`/`# TYPE` header exactly once, and always —
/// including with zero accounts configured, which is what makes an empty scrape
/// valid rather than blank.
#[must_use]
#[allow(clippy::too_many_lines)] // One linear block per metric family.
pub fn render(snap: &Snapshot) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "# HELP cctui_build_info Running server version.");
    let _ = writeln!(out, "# TYPE cctui_build_info gauge");
    let _ = writeln!(out, "cctui_build_info{{version=\"{}\"}} 1", escape(snap.version));

    let _ = writeln!(
        out,
        "# HELP cctui_account_usage_known Whether a usage reading is available for this credential."
    );
    let _ = writeln!(out, "# TYPE cctui_account_usage_known gauge");
    for p in &snap.providers {
        let _ = writeln!(
            out,
            "cctui_account_usage_known{{{}}} {}",
            account_labels(&p.row),
            i32::from(p.windows.is_some())
        );
    }

    let _ = writeln!(
        out,
        "# HELP cctui_account_usage_age_seconds Age of the cached usage reading for this credential."
    );
    let _ = writeln!(out, "# TYPE cctui_account_usage_age_seconds gauge");
    for p in &snap.providers {
        if let Some(age) = p.age_secs {
            let _ = writeln!(out, "cctui_account_usage_age_seconds{{{}}} {age}", account_labels(&p.row));
        }
    }

    let _ = writeln!(
        out,
        "# HELP cctui_account_window_utilization_percent Utilization of one usage window."
    );
    let _ = writeln!(out, "# TYPE cctui_account_window_utilization_percent gauge");
    for_each_window(snap, &mut out, |out, p, w, _| {
        if !is_usd_key(&w.key) {
            let _ = writeln!(
                out,
                "cctui_account_window_utilization_percent{{{}}} {}",
                window_labels(&p.row, w),
                w.utilization
            );
        }
    });

    let _ =
        writeln!(out, "# HELP cctui_account_window_spend_usd Dollars spent in one usage window.");
    let _ = writeln!(out, "# TYPE cctui_account_window_spend_usd gauge");
    for_each_window(snap, &mut out, |out, p, w, _| {
        if let Some(usd) = w.amount_usd {
            let _ =
                writeln!(out, "cctui_account_window_spend_usd{{{}}} {usd}", window_labels(&p.row, w));
        }
    });

    let _ = writeln!(
        out,
        "# HELP cctui_account_window_seconds_to_reset Seconds until this window resets."
    );
    let _ = writeln!(out, "# TYPE cctui_account_window_seconds_to_reset gauge");
    for_each_window(snap, &mut out, |out, p, w, _| {
        if let Some(at) = w.resets_at {
            let secs = (at - Utc::now()).num_seconds().max(0);
            let _ = writeln!(
                out,
                "cctui_account_window_seconds_to_reset{{{}}} {secs}",
                window_labels(&p.row, w)
            );
        }
    });

    let _ = writeln!(
        out,
        "# HELP cctui_account_window_pace_ratio Burn rate as a multiple of an even spend of this window."
    );
    let _ = writeln!(out, "# TYPE cctui_account_window_pace_ratio gauge");
    for_each_window(snap, &mut out, |out, p, w, pace| {
        if let Some(pace) = pace {
            let _ = writeln!(
                out,
                "cctui_account_window_pace_ratio{{{}}} {}",
                window_labels(&p.row, w),
                pace.ratio
            );
        }
    });

    let _ = writeln!(
        out,
        "# HELP cctui_account_window_projected_wall_timestamp_seconds When this window hits 100% at the current rate."
    );
    let _ = writeln!(out, "# TYPE cctui_account_window_projected_wall_timestamp_seconds gauge");
    for_each_window(snap, &mut out, |out, p, w, pace| {
        if let Some(at) = pace.and_then(|p| p.projected_wall_at) {
            let _ = writeln!(
                out,
                "cctui_account_window_projected_wall_timestamp_seconds{{{}}} {}",
                window_labels(&p.row, w),
                seconds_since_epoch(at)
            );
        }
    });

    let _ = writeln!(
        out,
        "# HELP cctui_account_window_cap_percent Configured soft-limit cap on this window."
    );
    let _ = writeln!(out, "# TYPE cctui_account_window_cap_percent gauge");
    for_each_window(snap, &mut out, |out, p, w, _| {
        if let Some(cap) = p.caps.limits.get(&w.key).and_then(|l| l.cap_pct) {
            let _ =
                writeln!(out, "cctui_account_window_cap_percent{{{}}} {cap}", window_labels(&p.row, w));
        }
    });

    let _ = writeln!(
        out,
        "# HELP cctui_account_window_cap_usd Configured soft-limit dollar cap on this window."
    );
    let _ = writeln!(out, "# TYPE cctui_account_window_cap_usd gauge");
    for_each_window(snap, &mut out, |out, p, w, _| {
        if let Some(cap) = p.caps.limits.get(&w.key).and_then(|l| l.cap_usd) {
            let _ = writeln!(out, "cctui_account_window_cap_usd{{{}}} {cap}", window_labels(&p.row, w));
        }
    });

    let _ = writeln!(
        out,
        "# HELP cctui_account_tokens_total Tokens attributed to this credential across all its sessions."
    );
    let _ = writeln!(out, "# TYPE cctui_account_tokens_total counter");
    for p in &snap.providers {
        let _ = writeln!(
            out,
            "cctui_account_tokens_total{{{}}} {}",
            account_labels(&p.row),
            p.row.total_tokens()
        );
    }

    let _ = writeln!(
        out,
        "# HELP cctui_account_cost_usd_estimate Blended-rate cost estimate of this credential's recorded usage."
    );
    let _ = writeln!(out, "# TYPE cctui_account_cost_usd_estimate gauge");
    for p in &snap.providers {
        let _ = writeln!(
            out,
            "cctui_account_cost_usd_estimate{{{}}} {}",
            account_labels(&p.row),
            p.row.est_cost_usd()
        );
    }

    let _ = writeln!(out, "# HELP cctui_sessions Sessions by persisted status.");
    let _ = writeln!(out, "# TYPE cctui_sessions gauge");
    for (status, count) in &snap.sessions_by_status {
        let _ = writeln!(out, "cctui_sessions{{status=\"{}\"}} {count}", escape(status));
    }

    let _ = writeln!(out, "# HELP cctui_sessions_live Sessions currently in the live registry.");
    let _ = writeln!(out, "# TYPE cctui_sessions_live gauge");
    let _ = writeln!(out, "cctui_sessions_live {}", snap.sessions_live);

    let _ = writeln!(out, "# HELP cctui_machines_live Machines whose daemon was seen recently.");
    let _ = writeln!(out, "# TYPE cctui_machines_live gauge");
    let _ = writeln!(out, "cctui_machines_live {}", snap.machines_live);

    out
}

/// Walk every cached window of every credential, letting `emit` decide what to
/// write. Keeps the label set and the skip rules in one place per family.
fn for_each_window(
    snap: &Snapshot,
    out: &mut String,
    emit: impl Fn(&mut String, &ProviderSnapshot, &UsageWindow, Option<crate::pace::Pace>),
) {
    for p in &snap.providers {
        let Some(windows) = p.windows.as_ref() else { continue };
        for (i, w) in windows.iter().enumerate() {
            emit(out, p, w, p.paces.get(i).copied().flatten());
        }
    }
}

fn account_labels(row: &ProviderRow) -> String {
    format!(
        "account=\"{}\",provider=\"{}\",credential=\"{}\"",
        escape(&row.account_name),
        escape(&row.provider),
        row.id
    )
}

fn window_labels(row: &ProviderRow, w: &UsageWindow) -> String {
    format!("{},window=\"{}\"", account_labels(row), escape(&w.key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soft_limit::{KEY_SESSION, KEY_USD_7D, usd_window};

    fn row(name: &str) -> ProviderRow {
        ProviderRow {
            id: Uuid::nil(),
            account_name: name.to_owned(),
            provider: "anthropic".to_owned(),
            family: "anthropic".to_owned(),
            soft_limits: None,
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }
    }

    fn empty() -> Snapshot {
        Snapshot {
            version: "9.9.9",
            providers: Vec::new(),
            sessions_by_status: Vec::new(),
            sessions_live: 0,
            machines_live: 0,
        }
    }

    /// Every `# TYPE` line must be followed by a name that actually appears, and
    /// no metric family may be declared twice — the two things `promtool check
    /// metrics` is strictest about.
    fn declared_families(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|l| l.strip_prefix("# TYPE "))
            .map(|l| l.split_whitespace().next().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn an_empty_scrape_is_well_formed_not_blank() {
        let text = render(&empty());
        let families = declared_families(&text);
        assert!(families.len() >= 10, "{families:?}");
        let mut sorted = families.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), families.len(), "a family is declared twice");
        for f in &families {
            assert!(text.contains(&format!("# HELP {f} ")), "{f} has no HELP");
        }
        assert!(text.contains("cctui_machines_live 0"));
        assert!(text.contains("cctui_build_info{version=\"9.9.9\"} 1"));
    }

    #[test]
    fn every_sample_line_names_a_declared_family() {
        let snap = Snapshot {
            providers: vec![ProviderSnapshot {
                row: row("prod"),
                caps: SoftLimits::default(),
                windows: Some(vec![usd_window(KEY_USD_7D, 3.5, None)]),
                age_secs: Some(12),
                paces: vec![None],
            }],
            sessions_by_status: vec![("active".to_owned(), 3)],
            ..empty()
        };
        let text = render(&snap);
        let families = declared_families(&text);
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let name = line.split(['{', ' ']).next().unwrap_or_default();
            assert!(families.iter().any(|f| f == name), "undeclared sample: {line}");
        }
        assert!(text.contains("cctui_account_window_spend_usd{account=\"prod\""));
        assert!(text.contains("window=\"usd_7d\"} 3.5"));
        assert!(text.contains("cctui_account_usage_age_seconds{account=\"prod\""));
        assert!(text.contains("cctui_sessions{status=\"active\"} 3"));
    }

    #[test]
    fn an_uncached_credential_reports_unknown_and_no_zeroed_windows() {
        let snap = Snapshot {
            providers: vec![ProviderSnapshot {
                row: row("cold"),
                caps: SoftLimits::default(),
                windows: None,
                age_secs: None,
                paces: Vec::new(),
            }],
            ..empty()
        };
        let text = render(&snap);
        assert!(text.contains("cctui_account_usage_known{account=\"cold\",provider=\"anthropic\",credential=\"00000000-0000-0000-0000-000000000000\"} 0"));
        assert!(!text.contains("cctui_account_window_utilization_percent{"));
        assert!(!text.contains("cctui_account_usage_age_seconds{"));
    }

    #[test]
    fn a_percent_window_exports_utilization_and_its_configured_cap() {
        let mut r = row("prod");
        r.soft_limits = Some(serde_json::json!({ "session": { "cap_pct": 70 } }));
        let snap = Snapshot {
            providers: vec![ProviderSnapshot {
                row: r,
                caps: SoftLimits::from_json(Some(&serde_json::json!({ "session": { "cap_pct": 70 } }))),
                windows: Some(vec![UsageWindow {
                    key: KEY_SESSION.to_owned(),
                    kind: "session".to_owned(),
                    label: "5h".to_owned(),
                    utilization: 42.5,
                    amount_usd: None,
                    resets_at: None,
                    model_id: None,
                    model_display_name: None,
                }]),
                age_secs: Some(0),
                paces: vec![None],
            }],
            ..empty()
        };
        let text = render(&snap);
        assert!(text.contains("window=\"session\"} 42.5"));
        assert!(text.contains("cctui_account_window_cap_percent{account=\"prod\""));
        assert!(!text.contains("cctui_account_window_spend_usd{"));
    }

    #[test]
    fn a_hostile_account_name_cannot_break_the_exposition_format() {
        let mut r = row("prod\"; evil\nmore\\x");
        r.provider = "anthropic".to_owned();
        let snap =
            Snapshot { providers: vec![ProviderSnapshot { row: r, caps: SoftLimits::default(), windows: None, age_secs: None, paces: Vec::new() }], ..empty() };
        let text = render(&snap);
        // The name's quote, newline and backslash must not split or terminate a
        // line: one sample line per emitted series, no more.
        let samples = text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).count();
        assert_eq!(samples, 6, "{text}");
        assert!(text.contains("account=\"prod\\\"; evil\\nmore\\\\x\""));
    }

    #[test]
    fn escaping_covers_the_three_characters_the_format_reserves() {
        assert_eq!(escape("a\\b\"c\nd"), "a\\\\b\\\"c\\nd");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn the_cost_estimate_uses_the_family_rate() {
        let anthropic = row("a");
        assert!((anthropic.est_cost_usd() - 3.0).abs() < 1e-9);
        let mut openai = row("o");
        openai.family = "openai".to_owned();
        assert!((openai.est_cost_usd() - 1.25).abs() < 1e-9);
        assert_eq!(anthropic.total_tokens(), 1_000_000);
    }
}
