use std::collections::BTreeSet;

use serde_json::json;
use uuid::Uuid;

use super::{
    AppState, AuthContext, Extension, Json, SettingsPayload, State, StatusCode, clamp_auto_resume,
    clamp_harness_mode, clamp_locale, clamp_macros, clamp_plugins, clamp_secret_scrub,
    clamp_session_emoji_prefix, clamp_whip_stop_phrases, get_settings, harness_mode_of,
    harness_mode_to_adapter_token, put_settings, secret_scrub_of, whip_stop_phrases_of,
};

#[test]
fn clamp_plugins_keeps_installed_boolean_flags_and_drops_an_empty_block() {
    let installed = |id: &str| match id {
        "yubisashi" => Some(vec!["host".to_owned(), "cert".to_owned()]),
        "x" => Some(vec![]),
        _ => None,
    };
    let mut data = json!({ "plugins": { "enabled": { "yubisashi": true, "other": "yes", "": true, "gone": true } } });
    clamp_plugins(&mut data, installed);
    assert_eq!(data["plugins"], json!({ "enabled": { "yubisashi": true } }));

    let mut empty = json!({ "plugins": { "enabled": { "x": 1 } } });
    clamp_plugins(&mut empty, installed);
    assert_eq!(empty, json!({}));

    let mut junk = json!({ "plugins": "on" });
    clamp_plugins(&mut junk, installed);
    assert_eq!(junk, json!({}));

    let mut absent = json!({ "macros": { "enabled": true } });
    clamp_plugins(&mut absent, installed);
    assert_eq!(absent, json!({ "macros": { "enabled": true } }));

    let mut none = json!({ "plugins": { "enabled": { "yubisashi": true } } });
    clamp_plugins(&mut none, |_| None);
    assert_eq!(none, json!({}));
}

#[test]
fn clamp_plugins_keeps_declared_string_config_only() {
    let installed = |id: &str| match id {
        "yubisashi" => Some(vec!["host".to_owned(), "cert".to_owned()]),
        "x" => Some(vec![]),
        _ => None,
    };
    let long = "a".repeat(513);
    let mut data = json!({ "plugins": { "config": {
        "yubisashi": { "host": "10.0.0.5", "cert": "", "nope": "v", "n": 1 },
        "x": { "host": "dropped, undeclared" },
        "ghost": { "host": "dropped, not installed" },
        "y": { "host": long }
    } } });
    clamp_plugins(&mut data, installed);
    assert_eq!(data["plugins"], json!({ "config": { "yubisashi": { "host": "10.0.0.5" } } }));

    let mut both = json!({ "plugins": {
        "enabled": { "yubisashi": false },
        "config": { "yubisashi": { "cert": "/c.pem" } }
    } });
    clamp_plugins(&mut both, installed);
    assert_eq!(
        both["plugins"],
        json!({ "enabled": { "yubisashi": false }, "config": { "yubisashi": { "cert": "/c.pem" } } })
    );

    let mut over = json!({ "plugins": { "config": { "yubisashi": { "host": "a".repeat(513) } } } });
    clamp_plugins(&mut over, installed);
    assert_eq!(over, json!({}));
}

#[test]
fn clamp_auto_resume_coerces_to_a_boolean() {
    let mut truthy = json!({ "autoResumeOnConnectionLoss": "yes" });
    clamp_auto_resume(&mut truthy);
    assert_eq!(truthy["autoResumeOnConnectionLoss"], json!(false));
    let mut on = json!({ "autoResumeOnConnectionLoss": true });
    clamp_auto_resume(&mut on);
    assert_eq!(on["autoResumeOnConnectionLoss"], json!(true));
    let mut absent = json!({});
    clamp_auto_resume(&mut absent);
    assert_eq!(absent, json!({}));
}

#[test]
fn clamp_session_emoji_prefix_coerces_to_a_boolean() {
    // A truthy string must not survive: the SQL read compares the stored
    // JSON to `true`, so only a real boolean can enable the prefix.
    let mut truthy = json!({ "sessionEmojiPrefix": "yes" });
    clamp_session_emoji_prefix(&mut truthy);
    assert_eq!(truthy["sessionEmojiPrefix"], json!(false));

    let mut on = json!({ "sessionEmojiPrefix": true });
    clamp_session_emoji_prefix(&mut on);
    assert_eq!(on["sessionEmojiPrefix"], json!(true));

    // Absence stays implicit rather than being materialized as `false`.
    let mut absent = json!({});
    clamp_session_emoji_prefix(&mut absent);
    assert_eq!(absent, json!({}));
}

#[test]
fn clamp_secret_scrub_rejects_bad_regex_and_keeps_valid() {
    let mut good = json!({
        "secretScrubEnabled": true,
        "secretScrubPatterns": [{ "name": "acme", "regex": "ACME-[0-9]+", "enabled": true }],
    });
    assert!(clamp_secret_scrub(&mut good).is_ok());
    assert_eq!(good["secretScrubPatterns"][0]["regex"], "ACME-[0-9]+");

    let mut bad = json!({ "secretScrubPatterns": [{ "name": "x", "regex": "([a-z" }] });
    assert!(clamp_secret_scrub(&mut bad).is_err());
}

#[test]
fn secret_scrub_of_returns_only_enabled_patterns() {
    let data = json!({
        "secretScrubEnabled": true,
        "secretScrubPatterns": [
            { "name": "on", "regex": "AAA[0-9]+", "enabled": true },
            { "name": "off", "regex": "BBB[0-9]+", "enabled": false },
        ],
    });
    let cfg = secret_scrub_of(&data);
    assert!(cfg.enabled);
    assert_eq!(cfg.patterns.len(), 1);
    assert_eq!(cfg.patterns[0].name, "on");
}

#[test]
fn secret_scrub_is_on_when_the_user_never_chose() {
    assert!(secret_scrub_of(&json!({})).enabled);
    assert!(secret_scrub_of(&json!(null)).enabled);
    assert!(!secret_scrub_of(&json!({ "secretScrubEnabled": false })).enabled);
}

#[test]
fn clamp_secret_scrub_opt_ins_default_false() {
    let mut data = json!({
        "secretScrubOutbound": "yes",
        "secretScrubLocalTranscripts": null,
    });
    assert!(clamp_secret_scrub(&mut data).is_ok());
    assert_eq!(data["secretScrubOutbound"], json!(false));
    assert_eq!(data["secretScrubLocalTranscripts"], json!(false));

    let mut on = json!({ "secretScrubOutbound": true, "secretScrubLocalTranscripts": true });
    assert!(clamp_secret_scrub(&mut on).is_ok());
    assert_eq!(on["secretScrubOutbound"], json!(true));
    assert_eq!(on["secretScrubLocalTranscripts"], json!(true));

    let mut absent = json!({ "secretScrubEnabled": true });
    assert!(clamp_secret_scrub(&mut absent).is_ok());
    assert!(absent.get("secretScrubOutbound").is_none());
    assert!(absent.get("secretScrubLocalTranscripts").is_none());
}

#[test]
fn secret_scrub_opt_ins_are_off_unless_chosen() {
    use cctui_server_kernel::execution_settings::secret_scrub_outbound_of;
    assert!(!secret_scrub_of(&json!({})).local_transcripts);
    assert!(!secret_scrub_outbound_of(&json!({})));
    assert!(!secret_scrub_outbound_of(&json!(null)));
    let data = json!({ "secretScrubOutbound": true, "secretScrubLocalTranscripts": true });
    assert!(secret_scrub_of(&data).local_transcripts);
    assert!(secret_scrub_outbound_of(&data));
}

#[test]
fn secret_scrub_opt_ins_need_the_base_switch() {
    use cctui_server_kernel::execution_settings::secret_scrub_outbound_of;
    let off = json!({
        "secretScrubEnabled": false,
        "secretScrubOutbound": true,
        "secretScrubLocalTranscripts": true,
    });
    let cfg = secret_scrub_of(&off);
    assert!(!cfg.enabled);
    assert!(!cfg.local_transcripts);
    assert!(!secret_scrub_outbound_of(&off));
    let on = json!({
        "secretScrubEnabled": true,
        "secretScrubOutbound": true,
        "secretScrubLocalTranscripts": true,
    });
    assert!(secret_scrub_of(&on).local_transcripts);
    assert!(secret_scrub_outbound_of(&on));
}

#[test]
fn clamp_secret_scrub_drops_empty_patterns_key() {
    let mut data = json!({ "secretScrubPatterns": [{ "regex": "  " }] });
    assert!(clamp_secret_scrub(&mut data).is_ok());
    assert!(data.get("secretScrubPatterns").is_none());
}

#[test]
fn harness_mode_of_clamps_unknown_and_missing_to_bg() {
    assert_eq!(harness_mode_of(&json!({})), "bg");
    assert_eq!(harness_mode_of(&json!({ "harnessMode": "wat" })), "bg");
    assert_eq!(harness_mode_of(&json!({ "harnessMode": 3 })), "bg");
    assert_eq!(harness_mode_of(&json!({ "harnessMode": "sdk" })), "sdk");
    assert_eq!(harness_mode_of(&json!({ "harnessMode": "oneshot" })), "oneshot");
}

#[test]
fn clamp_rewrites_a_typo_but_leaves_absence_implicit() {
    let mut bad = json!({ "harnessMode": "wat", "theme": "dark" });
    clamp_harness_mode(&mut bad);
    assert_eq!(bad["harnessMode"], "bg");
    assert_eq!(bad["theme"], "dark");

    let mut none = json!({ "theme": "dark" });
    clamp_harness_mode(&mut none);
    assert!(none.get("harnessMode").is_none());
}

#[test]
fn whip_phrases_absent_stays_none() {
    assert_eq!(whip_stop_phrases_of(&json!({})), None);
    let mut data = json!({ "theme": "dark" });
    clamp_whip_stop_phrases(&mut data);
    assert!(data.get("whipStopPhrases").is_none());
}

#[test]
fn whip_phrases_default_block_is_dropped() {
    let mut data = json!({ "whipStopPhrases": { "mode": "extend", "phrases": [] } });
    clamp_whip_stop_phrases(&mut data);
    assert!(data.get("whipStopPhrases").is_none());
}

#[test]
fn whip_phrases_trims_lowercases_dedupes_and_drops_empties() {
    let mut data = json!({
        "whipStopPhrases": {
            "mode": "extend",
            "phrases": ["  Pour Une Autre Session ", "", "pour une autre session", "  "]
        }
    });
    clamp_whip_stop_phrases(&mut data);
    assert_eq!(
        data["whipStopPhrases"],
        json!({ "mode": "extend", "phrases": ["pour une autre session"] })
    );
}

#[test]
fn whip_phrases_replace_mode_and_guidance_survive() {
    let mut data = json!({
        "whipStopPhrases": {
            "mode": "replace",
            "phrases": ["prêt pour ta relecture"],
            "guidance": "  Continue en français.  "
        }
    });
    clamp_whip_stop_phrases(&mut data);
    assert_eq!(
        data["whipStopPhrases"],
        json!({
            "mode": "replace",
            "phrases": ["prêt pour ta relecture"],
            "guidance": "Continue en français."
        })
    );
}

#[test]
fn whip_phrases_unknown_mode_clamps_to_extend() {
    let block = whip_stop_phrases_of(&json!({
        "whipStopPhrases": { "mode": "wat", "phrases": ["x"] }
    }))
    .unwrap();
    assert_eq!(block["mode"], "extend");
}

#[test]
fn whip_phrases_caps_count() {
    let many: Vec<String> = (0..500).map(|i| format!("phrase {i}")).collect();
    let block = whip_stop_phrases_of(&json!({
        "whipStopPhrases": { "mode": "replace", "phrases": many }
    }))
    .unwrap();
    assert_eq!(block["phrases"].as_array().unwrap().len(), super::MAX_WHIP_PHRASES);
}

#[test]
fn clamp_locale_keeps_known_drops_unknown_and_leaves_absence() {
    let mut en = json!({ "locale": "en", "theme": "dark" });
    clamp_locale(&mut en);
    assert_eq!(en["locale"], "en");
    assert_eq!(en["theme"], "dark");

    let mut fr = json!({ "locale": "fr" });
    clamp_locale(&mut fr);
    assert_eq!(fr["locale"], "fr");

    let mut bad = json!({ "locale": "de" });
    clamp_locale(&mut bad);
    assert!(bad.get("locale").is_none());

    let mut typed = json!({ "locale": 3 });
    clamp_locale(&mut typed);
    assert!(typed.get("locale").is_none());

    let mut absent = json!({ "theme": "dark" });
    clamp_locale(&mut absent);
    assert!(absent.get("locale").is_none());
}

#[test]
fn adapter_token_maps_bg_to_claude_daemon_and_passes_others_through() {
    assert_eq!(harness_mode_to_adapter_token(Some("bg")), "claude-daemon");
    assert_eq!(harness_mode_to_adapter_token(None), "claude-daemon");
    assert_eq!(harness_mode_to_adapter_token(Some("typo")), "claude-daemon");
    assert_eq!(harness_mode_to_adapter_token(Some("sdk")), "sdk");
    assert_eq!(harness_mode_to_adapter_token(Some("oneshot")), "oneshot");
}

#[test]
fn clamp_macros_keeps_well_formed_items_and_drops_an_empty_off_block() {
    let mut data = serde_json::json!({
        "macros": {
            "enabled": "yes",
            "items": [
                { "title": "  Nettoyage ", "prompt": "range le dépôt", "model": "opus",
                  "effort": "high", "pool_id": "p1", "confirm": false, "adapter": "" },
                { "title": "sans prompt", "prompt": "   " },
                "garbage"
            ]
        }
    });
    clamp_macros(&mut data);
    let block = &data["macros"];
    assert_eq!(block["enabled"], serde_json::json!(false));
    let items = block["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Nettoyage");
    assert_eq!(items[0]["adapter"], "claude-code");
    assert_eq!(items[0]["confirm"], serde_json::json!(false));
    assert_eq!(items[0]["pool_id"], "p1");
    assert!(items[0]["machine_id"].is_null());
    assert!(items[0]["id"].as_str().is_some_and(|id| !id.is_empty()));

    let mut off = serde_json::json!({ "macros": { "enabled": false, "items": [] } });
    clamp_macros(&mut off);
    assert!(off.get("macros").is_none());

    let mut on = serde_json::json!({ "macros": { "enabled": true } });
    clamp_macros(&mut on);
    assert_eq!(on["macros"]["enabled"], serde_json::json!(true));
    assert_eq!(on["macros"]["items"], serde_json::json!([]));
}

fn ctx(user_id: Uuid) -> AuthContext {
    AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes: BTreeSet::new() }
}

#[tokio::test]
async fn put_settings_rejects_a_bad_regex_with_the_parse_error() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap();
    let body = SettingsPayload {
        revision: None,
        version: 1,
        data: json!({
            "secretScrubPatterns": [{ "name": "custom", "regex": "*_token", "enabled": true }]
        }),
    };
    let state = State(AppState::for_test(pool));
    let sent = put_settings(state, Extension(ctx(Uuid::new_v4())), Json(body)).await;
    let Err(err) = sent else {
        panic!("an uncompilable pattern must not reach the database");
    };
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(err.message().contains("*_token"), "{}", err.message());
    assert!(err.message().starts_with("invalid scrub regex"), "{}", err.message());
}

#[tokio::test]
async fn put_settings_refuses_a_copy_read_before_another_write() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "put_settings_refuses_a_copy_read_before_another_write",
    ) else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'settings revision test', $2)")
        .bind(user_id)
        .bind(user_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let state = || State(AppState::for_test(pool.clone()));
    let put = |data: serde_json::Value, revision: Option<String>| {
        put_settings(
            state(),
            Extension(ctx(user_id)),
            Json(SettingsPayload { version: 1, data, revision }),
        )
    };

    let first = put(json!({ "nav": "top" }), None).await.unwrap().0;
    let read_by_stale_tab = get_settings(state(), Extension(ctx(user_id))).await.unwrap().0;
    assert_eq!(read_by_stale_tab.revision, first.revision);

    let second = put(json!({ "nav": "side" }), first.revision.clone()).await.unwrap().0;
    assert_ne!(second.revision, first.revision);

    let Err(refused) = put(json!({ "nav": "top" }), read_by_stale_tab.revision).await else {
        panic!("a copy read before the second write must not overwrite it");
    };
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let kept = get_settings(state(), Extension(ctx(user_id))).await.unwrap().0;
    assert_eq!(kept.data["nav"], json!("side"));
    assert_eq!(kept.revision, second.revision);

    let unconditional = put(json!({ "nav": "top" }), None).await.unwrap().0;
    assert_eq!(unconditional.data["nav"], json!("top"));

    sqlx::query("DELETE FROM users WHERE id = $1").bind(user_id).execute(&pool).await.unwrap();
}
