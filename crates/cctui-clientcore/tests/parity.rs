//! Replays `fixtures/parity/*.json` against this crate.
//!
//! The webui replays the same files against its `TypeScript` originals, so a
//! changed case fails on both sides until both implementations agree.

use cctui_clientcore::history_nav::HistoryNav;
use cctui_clientcore::{
    bookmarks, dispatch, format, git, labels, mention, search, session_failure, uploads,
};
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

/// The dispatch body both clients post for the same choices. A changed case
/// fails here and in the webui's own replay until both agree.
#[test]
fn dispatch_body_parity() {
    let fx = fixture("dispatchBody");

    for c in cases(&fx, "contextPackEnv") {
        let pack = pack_of(&c["pack"]);
        let got: serde_json::Map<String, Value> = dispatch::context_pack_env(&pack)
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        assert_eq!(Value::Object(got), c["out"], "contextPackEnv {c}");
    }

    for c in cases(&fx, "buildDispatchBody") {
        let form = form_of(&c["form"]);
        let pack = pack_of(&c["pack"]);
        let env = env_of(&c["env"]);
        let provider = c["provider"].as_str();
        let got = dispatch::build_dispatch_body(&form, &env, &pack, provider, &s(c, "sessionId"));
        assert_eq!(got, c["out"], "buildDispatchBody {}", s(c, "why"));
    }
}

fn pack_of(v: &Value) -> dispatch::ContextPack {
    dispatch::ContextPack {
        url: s(v, "url"),
        r#ref: s(v, "ref"),
        subdir: s(v, "subdir"),
        token: s(v, "token"),
    }
}

/// Object order is the order the keys were written, which is what a
/// byte-equivalence claim needs.
fn env_of(v: &Value) -> Vec<(String, String)> {
    v.as_object()
        .map(|o| {
            o.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or_default().to_owned())).collect()
        })
        .unwrap_or_default()
}

fn form_of(v: &Value) -> dispatch::DispatchForm {
    dispatch::DispatchForm {
        dispatcher: s(v, "dispatcher"),
        dispatch_adapter: s(v, "dispatch_adapter"),
        name: s(v, "name"),
        identity: s(v, "identity"),
        repo: s(v, "repo"),
        ticket: s(v, "ticket"),
        prompt: s(v, "prompt"),
        prompt_file: s(v, "prompt_file"),
        model_claude: s(v, "model_claude"),
        model_codex: s(v, "model_codex"),
        model_account: s(v, "model_account"),
        effort_claude: s(v, "effort_claude"),
        effort_codex: s(v, "effort_codex"),
        timeout: s(v, "timeout"),
        account: s(v, "account"),
    }
}

/// `fixtures/parity/spawnBody.json`, replayed by the web UI's own test against
/// `buildSpawnBody`. Only the keys a case names are asserted, so a field a
/// later lane adds cannot invalidate the file.
#[test]
fn spawn_body_parity() {
    use cctui_clientcore::spawn::{SpawnFields, build_spawn_body};

    let fx = fixture("spawnBody");
    for case in cases(&fx, "cases") {
        let name = s(case, "name");
        let f = &case["fields"];
        let text = |key: &str| f[key].as_str().unwrap_or_default().to_string();
        let list = |key: &str| {
            f[key]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(ToString::to_string)).collect())
                .unwrap_or_default()
        };
        let fields = SpawnFields {
            machine_id: text("machine_id"),
            working_dir: text("working_dir"),
            name: text("name"),
            prompt: text("prompt"),
            adapter_id: text("adapter_id"),
            permission_mode: text("permission_mode"),
            model_claude: text("model_claude"),
            model_codex: text("model_codex"),
            model_account: text("model_account"),
            effort_claude: text("effort_claude"),
            effort_codex: text("effort_codex"),
            service_tier: text("service_tier"),
            account: text("account"),
            account_provider: text("account_provider"),
            labels: list("labels"),
            context_items: list("context_items"),
            context_auto: f["context_auto"].as_bool().unwrap_or(false),
        };
        let provider = case["provider"].as_str();
        let got =
            build_spawn_body(&fields, provider, std::collections::BTreeMap::new(), None, None);
        let got = serde_json::to_value(&got).unwrap_or_else(|e| panic!("{name}: {e}"));

        let expect = case["expect"].as_object().unwrap_or_else(|| panic!("{name}: no expect"));
        for (key, want) in expect {
            // The wire omits a `false` flag and an empty list; the TypeScript
            // object spells both out. Same meaning, so an absent key is read as
            // whatever empty the expectation is shaped like.
            let actual = got.get(key).cloned().unwrap_or_else(|| match want {
                Value::Bool(_) => Value::Bool(false),
                Value::Array(_) => Value::Array(Vec::new()),
                _ => Value::Null,
            });
            assert_eq!(&actual, want, "{name}: {key}");
        }
    }
}

#[test]
fn spawn_accounts_parity() {
    use cctui_clientcore::spawn_accounts::{
        ALL_ADAPTERS, PoolMembers, UsageWindow, account_adapters, account_backs_adapter,
        adapter_for_provider, compatible_pools, effective_adapter_for, env_key_valid, headline_pct,
        provider_for_adapter, stale_account_pick,
    };

    let fx = fixture("spawnAccounts");
    assert_eq!(strings(&fx["allAdapters"]), ALL_ADAPTERS);

    for c in cases(&fx, "adapterForProvider") {
        assert_eq!(adapter_for_provider(&s(c, "provider")), s(c, "out"), "adapter {c}");
    }
    for c in cases(&fx, "accountAdapters") {
        let providers = strings(&c["providers"]);
        assert_eq!(account_adapters(&providers), strings(&c["out"]), "accountAdapters {c}");
    }
    for c in cases(&fx, "accountBacksAdapter") {
        let providers = c["providers"].as_array().map(|_| strings(&c["providers"]));
        assert_eq!(
            account_backs_adapter(providers.as_deref(), &s(c, "adapter")),
            c["out"].as_bool().expect("a bool"),
            "accountBacksAdapter {c}"
        );
    }
    for c in cases(&fx, "effectiveAdapterFor") {
        let providers = c["providers"].as_array().map(|_| strings(&c["providers"]));
        assert_eq!(
            effective_adapter_for(providers.as_deref(), &s(c, "adapter")),
            s(c, "out"),
            "effectiveAdapterFor {c}"
        );
    }
    for c in cases(&fx, "providerForAdapter") {
        let providers = strings(&c["providers"]);
        assert_eq!(
            provider_for_adapter(&providers, &s(c, "adapter")),
            opt_s(c, "out").as_deref(),
            "providerForAdapter {c}"
        );
    }
    for c in cases(&fx, "staleAccountPick") {
        assert_eq!(
            stale_account_pick(&s(c, "value"), &strings(&c["names"])),
            c["out"].as_bool().expect("a bool"),
            "staleAccountPick {c}"
        );
    }
    for c in cases(&fx, "envKeyValid") {
        assert_eq!(
            env_key_valid(&s(c, "key")),
            c["out"].as_bool().expect("a bool"),
            "envKeyValid {c}"
        );
    }
    for c in cases(&fx, "headlinePct") {
        let windows: Vec<UsageWindow> = c["windows"]
            .as_array()
            .expect("windows")
            .iter()
            .map(|w| UsageWindow {
                key: s(w, "key"),
                utilization: w["utilization"].as_f64().expect("a number"),
            })
            .collect();
        let want = c["out"].as_u64().map(|n| u32::try_from(n).expect("fits"));
        assert_eq!(headline_pct(&windows), want, "headlinePct {c}");
    }

    let group = &fx["compatiblePools"];
    let lookup = |key: &str| -> Option<Vec<String>> {
        group["accounts"]
            .as_array()
            .expect("accounts")
            .iter()
            .find(|a| a["key"] == key)
            .map(|a| strings(&a["providers"]))
    };
    for c in cases(group, "cases") {
        let owned: Vec<Vec<String>> =
            c["pools"].as_array().expect("pools").iter().map(strings).collect();
        let pools: Vec<PoolMembers<'_>> = owned
            .iter()
            .map(|members| PoolMembers { members: members.iter().map(String::as_str).collect() })
            .collect();
        let want: Vec<usize> = c["out"]
            .as_array()
            .expect("out")
            .iter()
            .map(|v| usize::try_from(v.as_u64().expect("an index")).expect("fits"))
            .collect();
        assert_eq!(
            compatible_pools(&pools, &lookup, &s(c, "harness")),
            want,
            "compatiblePools {c}"
        );
    }
}
