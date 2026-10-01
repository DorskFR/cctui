//! Replays `fixtures/parity/*.json` against this crate.
//!
//! The webui replays the same files against its `TypeScript` originals, so a
//! changed case fails on both sides until both implementations agree.

use cctui_clientcore::history_nav::HistoryNav;
use cctui_clientcore::{bookmarks, format, git, labels, mention, search, session_failure, uploads};
use cctui_proto::git::GitInfo;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/parity/").to_string() + name + ".json";
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn cases<'a>(fx: &'a Value, key: &str) -> &'a Vec<Value> {
    fx[key].as_array().unwrap_or_else(|| panic!("missing case list {key}"))
}

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn opt_s(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(ToString::to_string)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

#[test]
fn format_parity() {
    let fx = fixture("format");
    for c in cases(&fx, "compact") {
        assert_eq!(format::compact(c["n"].as_f64().unwrap()), s(c, "out"), "compact {c}");
    }
    for c in cases(&fx, "uptime") {
        assert_eq!(format::uptime(c["secs"].as_u64().unwrap()), s(c, "out"), "uptime {c}");
    }
    for c in cases(&fx, "statusBadgeTone") {
        assert_eq!(format::status_badge_tone(&s(c, "status")).as_str(), s(c, "out"), "tone {c}");
    }
    for c in cases(&fx, "modelShort") {
        assert_eq!(format::model_short(&s(c, "model")), s(c, "out"), "modelShort {c}");
    }
    for c in cases(&fx, "modelFamily") {
        assert_eq!(format::model_family(&s(c, "model")), s(c, "out"), "modelFamily {c}");
    }
    for c in cases(&fx, "modelAbbrev") {
        assert_eq!(format::model_abbrev(&s(c, "model")), s(c, "out"), "modelAbbrev {c}");
    }
    for c in cases(&fx, "machineInitial") {
        assert_eq!(format::machine_initial(&s(c, "label")), s(c, "out"), "machineInitial {c}");
    }
    for c in cases(&fx, "usd") {
        assert_eq!(format::usd(c["n"].as_f64().unwrap()), s(c, "out"), "usd {c}");
    }
    for c in cases(&fx, "hashHue") {
        assert_eq!(
            u64::from(format::hash_hue(&s(c, "s"))),
            c["out"].as_u64().unwrap(),
            "hashHue {c}"
        );
    }
    for c in cases(&fx, "machineTint") {
        let hue = c["hue"].as_u64().map(|h| h as u32);
        assert_eq!(format::machine_tint(&s(c, "label"), hue), s(c, "out"), "machineTint {c}");
    }
}

#[test]
fn labels_parity() {
    let fx = fixture("labels");
    let hues: Vec<u32> =
        cases(&fx, "LABEL_HUES").iter().map(|v| v.as_u64().unwrap() as u32).collect();
    assert_eq!(hues, labels::LABEL_HUES.to_vec());
    for c in cases(&fx, "storedHue") {
        let expected = c["out"].as_u64().map(|v| v as u32);
        assert_eq!(labels::stored_hue(&s(c, "color")), expected, "storedHue {c}");
    }
    for c in cases(&fx, "labelHue") {
        assert_eq!(
            u64::from(labels::label_hue(&s(c, "name"), &s(c, "color"))),
            c["out"].as_u64().unwrap(),
            "labelHue {c}"
        );
    }
    for c in cases(&fx, "hueToColor") {
        let hue = c["hue"].as_u64().map(|v| v as u32);
        assert_eq!(labels::hue_to_color(hue), s(c, "out"), "hueToColor {c}");
    }
    for c in cases(&fx, "labelTint") {
        assert_eq!(labels::label_tint(&s(c, "name"), &s(c, "color")), s(c, "out"), "labelTint {c}");
    }
}

#[test]
fn turnid_parity() {
    let fx = fixture("turnid");
    for c in cases(&fx, "turnIdFrom") {
        let mut random = [0u8; 16];
        for (slot, v) in random.iter_mut().zip(c["random"].as_array().unwrap()) {
            *slot = v.as_u64().unwrap() as u8;
        }
        let ts = c["ts"].as_u64().unwrap();
        assert_eq!(cctui_clientcore::turnid::turn_id_from(ts, random), s(c, "out"), "turnId {c}");
    }
}

#[test]
fn search_parity() {
    let fx = fixture("search");
    for c in cases(&fx, "tokenizeQuery") {
        assert_eq!(search::tokenize_query(&s(c, "q")), strings(&c["out"]), "tokenize {c}");
    }
    for c in cases(&fx, "highlightTerms") {
        let terms: Vec<String> = strings(&c["terms"]);
        assert_eq!(search::highlight_terms(&s(c, "html"), &terms), s(c, "out"), "highlight {c}");
    }
}

#[test]
fn bookmarks_parity() {
    let fx = fixture("bookmarks");
    for c in cases(&fx, "defaultTitle") {
        assert_eq!(bookmarks::default_title(&s(c, "body")), s(c, "out"), "defaultTitle {c}");
    }
    for c in cases(&fx, "queryTerms") {
        assert_eq!(bookmarks::query_terms(&s(c, "q")), strings(&c["out"]), "queryTerms {c}");
    }
    for c in cases(&fx, "sourceHref") {
        let id = opt_s(c, "session_id");
        let seq = c["seq"].as_i64();
        assert_eq!(bookmarks::source_href(id.as_deref(), seq), opt_s(c, "out"), "sourceHref {c}");
    }
    for c in cases(&fx, "isDeadLink") {
        let id = opt_s(c, "session_id");
        assert_eq!(
            bookmarks::is_dead_link(id.as_deref()),
            c["out"].as_bool().unwrap(),
            "isDeadLink {c}"
        );
    }
    for c in cases(&fx, "bookmarkMarkdown") {
        let note = opt_s(c, "note");
        assert_eq!(
            bookmarks::bookmark_markdown(&s(c, "title"), note.as_deref(), &s(c, "body")),
            s(c, "out"),
            "bookmarkMarkdown {c}"
        );
    }
}

#[test]
fn mention_parity() {
    let fx = fixture("mention");
    for c in cases(&fx, "findTrigger") {
        let caret = c["caret"].as_u64().unwrap() as usize;
        let got = mention::find_trigger(&s(c, "text"), caret);
        match c["out"].as_object() {
            None => assert!(got.is_none(), "findTrigger {c}"),
            Some(o) => {
                let t = got.unwrap_or_else(|| panic!("findTrigger {c}"));
                assert_eq!(t.start as u64, o["start"].as_u64().unwrap(), "findTrigger {c}");
                assert_eq!(t.query, o["query"].as_str().unwrap(), "findTrigger {c}");
            }
        }
    }
    for c in cases(&fx, "mentionableSessions") {
        let sessions: Vec<mention::MentionSession> =
            serde_json::from_value(c["sessions"].clone()).unwrap();
        let exclude = opt_s(c, "exclude_id");
        let ids: Vec<String> = mention::mentionable_sessions(&sessions, exclude.as_deref())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, expected_ids(c), "mentionable {c}");
    }
    for c in cases(&fx, "filterMentions") {
        let sessions: Vec<mention::MentionSession> =
            serde_json::from_value(c["sessions"].clone()).unwrap();
        let ids: Vec<String> =
            mention::filter_mentions(&sessions, &s(c, "query")).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, expected_ids(c), "filterMentions {c}");
    }
    for c in cases(&fx, "mentionToken") {
        let name = opt_s(c, "name");
        assert_eq!(
            mention::mention_token(&s(c, "id"), name.as_deref()),
            s(c, "out"),
            "mentionToken {c}"
        );
    }
    for c in cases(&fx, "applyMention") {
        let text = s(c, "text");
        let caret = c["caret"].as_u64().unwrap() as usize;
        let trigger = mention::find_trigger(&text, caret).expect("trigger");
        let name = opt_s(c, "name");
        let got = mention::apply_mention(&text, caret, &trigger, &s(c, "id"), name.as_deref());
        assert_eq!(got.text, c["out"]["text"].as_str().unwrap(), "applyMention {c}");
        assert_eq!(got.caret as u64, c["out"]["caret"].as_u64().unwrap(), "applyMention {c}");
    }
    for c in cases(&fx, "moveSelection") {
        let index = c["index"].as_u64().unwrap() as usize;
        let delta = c["delta"].as_i64().unwrap() as i32;
        let length = c["length"].as_u64().unwrap() as usize;
        assert_eq!(
            mention::move_selection(index, delta, length) as u64,
            c["out"].as_u64().unwrap(),
            "moveSelection {c}"
        );
    }
}

fn expected_ids(c: &Value) -> Vec<String> {
    strings(&c["out"])
}

#[test]
fn history_nav_parity() {
    let fx = fixture("historyNav");
    for case in fx.as_array().unwrap() {
        let list: Vec<String> = strings(&case["list"]);
        let mut value = s(case, "initial");
        let mut caret = value.chars().count();
        let mut nav = HistoryNav::new();
        let name = s(case, "name");
        for step in case["steps"].as_array().unwrap() {
            match step["op"].as_str().unwrap() {
                "caret" => caret = step["at"].as_u64().unwrap() as usize,
                "reset" => nav.reset(),
                "resetAll" => nav.reset_all(),
                "recall" => {
                    value = nav.recall(&list, &value, step["pick"].as_str().unwrap());
                    caret = value.chars().count();
                    assert_eq!(value, s(step, "value"), "{name}: recall");
                }
                "expectBrowsing" => {
                    assert_eq!(nav.browsing(), step["browsing"].as_bool().unwrap(), "{name}");
                }
                "key" => {
                    let out =
                        nav.handle_key(step["key"].as_str().unwrap(), &list, &value, caret, caret);
                    assert_eq!(out.handled, step["handled"].as_bool().unwrap(), "{name}: handled");
                    if let Some(next) = out.value {
                        value = next;
                        caret = value.chars().count();
                    }
                    assert_eq!(value, s(step, "value"), "{name}: value");
                }
                other => panic!("{name}: unknown op {other}"),
            }
        }
    }
}

#[test]
fn session_failure_parity() {
    let fx = fixture("sessionFailureToast");
    for c in cases(&fx, "shouldToast") {
        assert_eq!(
            session_failure::should_toast(&s(c, "reason")),
            c["out"].as_bool().unwrap(),
            "shouldToast {c}"
        );
    }
    for c in cases(&fx, "toastDetail") {
        let detail = opt_s(c, "detail");
        assert_eq!(
            session_failure::toast_detail(detail.as_deref()),
            opt_s(c, "out"),
            "toastDetail {c}"
        );
    }
    for c in cases(&fx, "endBadgeText") {
        let detail = opt_s(c, "detail");
        assert_eq!(
            session_failure::end_badge_text(&s(c, "reason"), &s(c, "label"), detail.as_deref()),
            s(c, "out"),
            "endBadgeText {c}"
        );
    }
    for c in cases(&fx, "sessionHref") {
        assert_eq!(
            session_failure::session_href(&s(c, "session_id")),
            s(c, "out"),
            "sessionHref {c}"
        );
    }
}

#[test]
fn git_badge_parity() {
    let fx = fixture("gitBadge");
    for c in fx.as_array().unwrap() {
        let info: Option<GitInfo> = serde_json::from_value(c["info"].clone()).unwrap();
        let got = git::git_badge(info.as_ref());
        let expected: Option<git::GitBadge> = serde_json::from_value(c["out"].clone()).unwrap();
        assert_eq!(got, expected, "gitBadge {c}");
    }
}

#[test]
fn attachment_caps_parity() {
    let fx = fixture("attachments");
    assert_eq!(uploads::MAX_FILE_BYTES, fx["MAX_FILE_BYTES"].as_u64().unwrap());
    assert_eq!(uploads::MAX_TOTAL_BYTES, fx["MAX_TOTAL_BYTES"].as_u64().unwrap());
    assert_eq!(u64::from(uploads::MAX_FILES), fx["MAX_FILES"].as_u64().unwrap());
}

#[test]
fn usage_parity() {
    use cctui_clientcore::usage;

    fn f(v: &Value, key: &str) -> Option<f64> {
        v[key].as_f64()
    }
    fn ms(v: &Value, key: &str) -> Option<i64> {
        v[key].as_i64()
    }

    let fx = fixture("usage");
    for c in cases(&fx, "headroomTone") {
        assert_eq!(
            usage::headroom_tone(f(c, "utilization")).as_str(),
            s(c, "out"),
            "headroomTone {c}"
        );
    }
    for c in cases(&fx, "paceState") {
        assert_eq!(
            usage::pace_state(f(c, "ratio")).map(|p| p.as_str().to_owned()),
            opt_s(c, "out"),
            "paceState {c}"
        );
    }
    for c in cases(&fx, "countdown") {
        assert_eq!(usage::countdown(ms(c, "ms").unwrap()), s(c, "out"), "countdown {c}");
    }
    for c in cases(&fx, "resetIn") {
        assert_eq!(
            usage::reset_in(ms(c, "resetsAtMs"), ms(c, "nowMs").unwrap()),
            opt_s(c, "out"),
            "resetIn {c}"
        );
    }
    for c in cases(&fx, "resetInShort") {
        assert_eq!(
            usage::reset_in_short(ms(c, "resetsAtMs"), ms(c, "nowMs").unwrap()),
            opt_s(c, "out"),
            "resetInShort {c}"
        );
    }
    for c in cases(&fx, "usdPct") {
        assert_eq!(
            usage::usd_pct(f(c, "amountUsd"), f(c, "capUsd")),
            c["out"].as_i64(),
            "usdPct {c}"
        );
    }
    for c in cases(&fx, "usdReadout") {
        assert_eq!(
            usage::usd_readout(f(c, "amountUsd"), f(c, "capUsd")),
            opt_s(c, "out"),
            "usdReadout {c}"
        );
    }
    for c in cases(&fx, "money") {
        assert_eq!(usage::money(f(c, "n").unwrap()), s(c, "out"), "money {c}");
    }
    for c in cases(&fx, "barPct") {
        assert_eq!(usage::bar_pct(f(c, "utilization")), c["out"].as_i64(), "barPct {c}");
    }
    for c in cases(&fx, "wallInMs") {
        assert_eq!(
            usage::wall_in_ms(ms(c, "wallAtMs"), ms(c, "resetsAtMs"), ms(c, "nowMs").unwrap()),
            c["out"].as_i64(),
            "wallInMs {c}"
        );
    }
}

#[test]
fn account_switch_parity() {
    use cctui_clientcore::account_switch::{
        Binding, Credential, Window, recommended, switch_options,
    };

    let fx = fixture("account_switch");
    for c in cases(&fx, "switchOptions") {
        let name = s(c, "name");
        let b = &c["binding"];
        let binding = Binding {
            family: s(b, "family"),
            account_id: s(b, "accountId"),
            account_name: s(b, "accountName"),
        };
        let credentials: Vec<Credential> = c["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Credential {
                account_id: s(v, "accountId"),
                account_name: s(v, "accountName"),
                provider: s(v, "provider"),
                windows: v["windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| Window {
                        pct: w["pct"].as_f64().unwrap(),
                        resets_in_secs: w["resetsInSecs"].as_i64(),
                    })
                    .collect(),
            })
            .collect();

        let rows = switch_options(
            &binding,
            &credentials,
            cctui_clientcore::account_switch::LIMITED_PCT,
        );
        let expected = c["out"].as_array().unwrap();
        assert_eq!(rows.len(), expected.len(), "{name}: row count");
        for (row, want) in rows.iter().zip(expected) {
            assert_eq!(row.account_name, s(want, "accountName"), "{name}: order");
            assert_eq!(row.pct, want["pct"].as_f64(), "{name}: {} pct", row.account_name);
            assert_eq!(
                row.resets_in_secs,
                want["resetsInSecs"].as_i64(),
                "{name}: {} reset",
                row.account_name
            );
            assert_eq!(row.current, want["current"].as_bool().unwrap(), "{name}: current");
            assert_eq!(row.limited, want["limited"].as_bool().unwrap(), "{name}: limited");
        }
        assert_eq!(
            recommended(&rows).map(|i| i as i64),
            c["recommended"].as_i64(),
            "{name}: recommended"
        );
    }
}

#[test]
fn accounts_parity() {
    use cctui_clientcore::accounts::{
        AccountRef, PoolRef, RedirectRef, accepts_member, membership_after_move, ordered_members,
        pool_of, redirect_chips,
    };

    let fx = fixture("accounts");
    let accounts: Vec<AccountRef> =
        serde_json::from_value(fx["accounts"].clone()).expect("accounts");
    let pools: Vec<PoolRef> = serde_json::from_value(fx["pools"].clone()).expect("pools");
    let redirects: Vec<RedirectRef> =
        serde_json::from_value(fx["redirects"].clone()).expect("redirects");
    let pool = |id: &str| pools.iter().find(|p| p.id == id).expect("a fixture pool");

    for c in cases(&fx, "poolOf") {
        assert_eq!(
            pool_of(&pools, &s(c, "accountId")).map(|p| p.id.clone()),
            opt_s(c, "out"),
            "poolOf {c}"
        );
    }
    for c in cases(&fx, "orderedMembers") {
        assert_eq!(ordered_members(pool(&s(c, "poolId"))), strings(&c["out"]), "orderedMembers {c}");
    }
    for c in cases(&fx, "acceptsMember") {
        assert_eq!(
            accepts_member(pool(&s(c, "poolId")), &s(c, "accountId"), &accounts),
            c["out"].as_bool().expect("a bool"),
            "acceptsMember {c}"
        );
    }
    for c in cases(&fx, "membershipAfterMove") {
        let to = opt_s(c, "to");
        let got = membership_after_move(&pools, &s(c, "accountId"), to.as_deref());
        let want = c["out"].as_array().expect("a list");
        assert_eq!(got.len(), want.len(), "membershipAfterMove {c}");
        for (change, expected) in got.iter().zip(want) {
            assert_eq!(change.pool_id, s(expected, "poolId"), "membershipAfterMove {c}");
            assert_eq!(change.accounts, strings(&expected["accounts"]), "membershipAfterMove {c}");
        }
    }
    for c in cases(&fx, "redirectChips") {
        let got = redirect_chips(&redirects, &accounts, &s(c, "accountId"));
        let want = c["out"].as_array().expect("a list");
        assert_eq!(got.len(), want.len(), "redirectChips {c}");
        for (chip, expected) in got.iter().zip(want) {
            assert_eq!(chip.id, s(expected, "id"), "redirectChips {c}");
            assert_eq!(chip.family, s(expected, "family"), "redirectChips {c}");
            assert_eq!(chip.target_name, s(expected, "targetName"), "redirectChips {c}");
            assert_eq!(chip.until, opt_s(expected, "until"), "redirectChips {c}");
        }
    }
}

#[test]
fn spend_parity() {
    use cctui_clientcore::spend::{
        DailyCacheLoss, DailyPoint, SessionSpend, Windows, cache_loss_totals, fill_daily,
        has_langfuse_cost, langfuse_cost_label, model_spend, spend_since, spend_totals,
    };

    fn sessions(v: &Value) -> Vec<SessionSpend> {
        v.as_array()
            .expect("a list")
            .iter()
            .map(|s| SessionSpend {
                model: s["model"].as_str().map(ToString::to_string),
                registered_at_ms: s["registeredAtMs"].as_i64(),
                cost_usd: s["costUsd"].as_f64().unwrap_or_default(),
                tokens: s["tokens"].as_u64().unwrap_or_default(),
            })
            .collect()
    }
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    let fx = fixture("spend");
    for c in cases(&fx, "spendSince") {
        let got = spend_since(&sessions(&c["sessions"]), c["cutoffMs"].as_i64().expect("an i64"));
        assert!(close(got, c["out"].as_f64().expect("a number")), "spendSince {c} -> {got}");
    }
    for c in cases(&fx, "modelSpend") {
        let w = &c["windows"];
        let windows = Windows {
            today_ms: w["todayMs"].as_i64().expect("an i64"),
            week_ms: w["weekMs"].as_i64().expect("an i64"),
            month_ms: w["monthMs"].as_i64().expect("an i64"),
        };
        let got = model_spend(&sessions(&c["sessions"]), windows);
        let want = c["out"].as_array().expect("a list");
        assert_eq!(got.len(), want.len(), "modelSpend {c}");
        for (row, expected) in got.iter().zip(want) {
            assert_eq!(row.model, s(expected, "model"), "modelSpend {c}");
            for (label, got, want) in [
                ("today", row.today, expected["today"].as_f64()),
                ("week", row.week, expected["week"].as_f64()),
                ("month", row.month, expected["month"].as_f64()),
            ] {
                assert!(close(got, want.expect("a number")), "modelSpend {label} {c} -> {got}");
            }
        }
        let totals = spend_totals(&got);
        let want = &c["totals"];
        assert_eq!(totals.model, s(want, "model"), "spendTotals {c}");
        assert!(close(totals.today, want["today"].as_f64().expect("a number")), "spendTotals {c}");
        assert!(close(totals.week, want["week"].as_f64().expect("a number")), "spendTotals {c}");
        assert!(close(totals.month, want["month"].as_f64().expect("a number")), "spendTotals {c}");
    }
    for c in cases(&fx, "fillDaily") {
        let points: Vec<DailyPoint> = c["points"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|p| DailyPoint {
                day_ms: p["dayMs"].as_i64().expect("an i64"),
                tokens: p["tokens"].as_u64().expect("a u64"),
            })
            .collect();
        let days = usize::try_from(c["days"].as_u64().expect("a u64")).expect("a usize");
        let got = fill_daily(&points, days, c["endDayMs"].as_i64().expect("an i64"));
        let want: Vec<u64> =
            c["out"].as_array().expect("a list").iter().map(|n| n.as_u64().unwrap()).collect();
        assert_eq!(got, want, "fillDaily {c}");
    }
    for c in cases(&fx, "cacheLossTotals") {
        let days: Vec<DailyCacheLoss> = c["days"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|d| DailyCacheLoss {
                usd: d["total"].as_f64().unwrap_or_default(),
                lost_tokens: d["lost_tokens"].as_u64().unwrap_or_default(),
                busts: d["busts"].as_u64().unwrap_or_default(),
            })
            .collect();
        let got = cache_loss_totals(&days);
        let want = &c["out"];
        assert!(close(got.usd, want["usd"].as_f64().expect("a number")), "cacheLossTotals {c}");
        assert_eq!(got.tokens, want["tokens"].as_u64().expect("a u64"), "cacheLossTotals {c}");
        assert_eq!(got.busts, want["busts"].as_u64().expect("a u64"), "cacheLossTotals {c}");
    }
    for c in cases(&fx, "langfuseCostLabel") {
        let got = langfuse_cost_label(c["costUsd"].as_f64().expect("a number"));
        assert_eq!(got, s(c, "out"), "langfuseCostLabel {c}");
    }
    for c in cases(&fx, "hasLangfuseCost") {
        let usage = c["usage"].as_object().map(|u| cctui_clientcore::spend::LangfuseSpend {
            cost_usd: u["cost_usd"].as_f64().unwrap_or_default(),
            trace_count: u["trace_count"].as_u64().unwrap_or_default(),
        });
        assert_eq!(
            has_langfuse_cost(usage),
            c["out"].as_bool().expect("a bool"),
            "hasLangfuseCost {c}"
        );
    }
}

#[test]
fn admin_parity() {
    use cctui_clientcore::admin;

    let fx = fixture("admin");
    assert_eq!(strings(&fx["allScopes"]), admin::ALL_SCOPES.to_vec());
    for c in cases(&fx, "scopeCells") {
        let got: Vec<bool> =
            admin::scope_cells(&strings(&c["granted"])).iter().map(|cell| cell.granted).collect();
        let want: Vec<bool> =
            c["out"].as_array().expect("a list").iter().map(|b| b.as_bool().unwrap()).collect();
        assert_eq!(got, want, "scopeCells {c}");
    }
    for c in cases(&fx, "visibleOrder") {
        let revoked: Vec<bool> =
            c["revoked"].as_array().expect("a list").iter().map(|b| b.as_bool().unwrap()).collect();
        assert_eq!(
            admin::visible_order(&revoked, c["show_revoked"].as_bool().expect("a bool")),
            indices(&c["out"]),
            "visibleOrder {c}"
        );
    }
    for c in cases(&fx, "filterByName") {
        assert_eq!(
            admin::filter_by_name(&strings(&c["names"]), &s(c, "query")),
            indices(&c["out"]),
            "filterByName {c}"
        );
    }
    for c in cases(&fx, "keyIcon") {
        assert_eq!(admin::key_icon(&s(c, "kind")), s(c, "out"), "keyIcon {c}");
    }
}

fn indices(v: &Value) -> Vec<usize> {
    v.as_array()
        .expect("a list")
        .iter()
        .map(|i| usize::try_from(i.as_u64().expect("an index")).expect("fits"))
        .collect()
}

#[test]
fn images_parity() {
    use cctui_clientcore::images;
    let fx = fixture("images");
    for c in cases(&fx, "scan") {
        let found: Vec<(String, String)> =
            images::scan(&s(c, "text")).into_iter().map(|m| (m.alt, m.id)).collect();
        let want: Vec<(String, String)> = c["out"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|m| (s(m, "alt"), s(m, "id")))
            .collect();
        assert_eq!(found, want, "scan {c}");
    }
    for c in cases(&fx, "placeholderLabel") {
        let dims = c["dimensions"].as_array().map(|d| {
            (d[0].as_u64().expect("w") as u32, d[1].as_u64().expect("h") as u32)
        });
        assert_eq!(
            images::placeholder_label(&s(c, "name"), dims, c["bytes"].as_u64()),
            s(c, "out"),
            "placeholderLabel {c}"
        );
    }
    for c in cases(&fx, "substitute") {
        assert_eq!(images::substitute(&s(c, "text")), s(c, "out"), "substitute {c}");
    }
    for c in cases(&fx, "isOnlyImages") {
        assert_eq!(
            images::is_only_images(&s(c, "text")),
            c["out"].as_bool().expect("a bool"),
            "isOnlyImages {c}"
        );
    }
}

#[test]
fn instance_parity() {
    use cctui_clientcore::instance;
    use cctui_proto::updatehook::UpdateHookPhase;

    let phase = |v: &Value| -> Option<UpdateHookPhase> {
        v.as_str().map(|s| serde_json::from_value(Value::String(s.to_owned())).expect("a phase"))
    };
    let b = |v: &Value, key: &str| v[key].as_bool().expect("a bool");

    let fx = fixture("instance");
    for c in cases(&fx, "updateAvailable") {
        assert_eq!(
            instance::update_available(&s(c, "version"), c["latest"].as_str()),
            b(c, "out"),
            "updateAvailable {c}"
        );
    }
    for c in cases(&fx, "phaseTone") {
        assert_eq!(
            instance::phase_tone(phase(&c["phase"])).as_str(),
            s(c, "out"),
            "phaseTone {c}"
        );
    }
    for c in cases(&fx, "phaseMessage") {
        assert_eq!(
            instance::phase_message(phase(&c["phase"]).expect("a phase")),
            s(c, "out"),
            "phaseMessage {c}"
        );
    }
    for c in cases(&fx, "hintMessage") {
        assert_eq!(
            instance::hint_message(b(c, "isAdmin"), b(c, "ready"), b(c, "hook")),
            s(c, "out"),
            "hintMessage {c}"
        );
    }
    for c in cases(&fx, "confirmMessage") {
        assert_eq!(instance::confirm_message(b(c, "hook")), s(c, "out"), "confirmMessage {c}");
    }
    for c in cases(&fx, "badgeMessage") {
        assert_eq!(instance::badge_message(b(c, "hook")), s(c, "out"), "badgeMessage {c}");
    }
    for c in cases(&fx, "canLaunch") {
        assert_eq!(
            instance::can_launch(b(c, "isAdmin"), b(c, "ready"), b(c, "available")),
            b(c, "out"),
            "canLaunch {c}"
        );
    }
}
