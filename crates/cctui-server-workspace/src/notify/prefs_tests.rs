use serde_json::json;

use super::{
    CHANNELS, NotificationSettings, NotificationStage, ROUTE_KINDS, USER_ACTION_BLOCKING, cell,
    channel, default_stage, delay_secs, migrate_sound, normalize, parse, resolved, route_key,
    stage,
};
use crate::notify::{Push, kind};

const AFTER_5: NotificationStage = NotificationStage::After { minutes: 5 };
const AFTER_10: NotificationStage = NotificationStage::After { minutes: 10 };

#[test]
fn defaults_reproduce_the_static_push_table() {
    let none = NotificationSettings::default();
    let cases = [
        (kind::APPROVAL, false),
        (kind::USER_ACTION, false),
        (kind::USER_ACTION, true),
        (kind::NEEDS_INPUT, false),
        (kind::SESSION_STUCK, false),
        (kind::SYSTEM_ALERT, false),
        (kind::INBOX_STUCK, false),
        (kind::DISPATCH_INFO, false),
    ];
    for (k, blocking) in cases {
        let key = route_key(k, blocking);
        let want = Push::for_kind(k, blocking).stage();
        assert_eq!(stage(&none, key, channel::NTFY), want, "{key} ntfy");
        assert_eq!(stage(&none, key, channel::WEB_PUSH), want, "{key} web_push");
        assert_eq!(stage(&none, key, channel::EMAIL), NotificationStage::Off, "email starts off");
        assert_eq!(cell(&none, key, channel::EMAIL), want, "the email cell is ready once on");
    }
    assert_eq!(stage(&none, kind::APPROVAL, channel::NTFY), AFTER_5);
    assert_eq!(stage(&none, USER_ACTION_BLOCKING, channel::NTFY), AFTER_10);
    assert_eq!(stage(&none, kind::USER_ACTION, channel::NTFY), NotificationStage::Off);
}

#[test]
fn in_app_is_on_for_every_recorded_kind_by_default() {
    for key in ROUTE_KINDS {
        assert_eq!(default_stage(key, channel::IN_APP), NotificationStage::Now, "{key}");
    }
    assert_eq!(default_stage(kind::DISPATCH_INFO, channel::IN_APP), NotificationStage::Off);
}

#[test]
fn only_a_blocking_user_action_gets_the_blocking_key() {
    assert_eq!(route_key(kind::USER_ACTION, true), USER_ACTION_BLOCKING);
    assert_eq!(route_key(kind::USER_ACTION, false), kind::USER_ACTION);
    assert_eq!(route_key(kind::APPROVAL, true), kind::APPROVAL);
}

#[test]
fn delays_are_seconds_and_off_never_fires() {
    assert_eq!(delay_secs(NotificationStage::Off), None);
    assert_eq!(delay_secs(NotificationStage::Now), Some(0));
    assert_eq!(delay_secs(AFTER_10), Some(600));
    assert_eq!(delay_secs(NotificationStage::After { minutes: u32::MAX }), Some(24 * 3600));
}

#[test]
fn a_sparse_block_overrides_only_what_it_names() {
    let data = json!({ "notifications": {
        "channels": { "ntfy": { "enabled": false }, "email": { "enabled": true } },
        "routes": { "approval": { "web_push": { "mode": "now" } } },
    }});
    let s = parse(&data);
    assert!(s.enabled && s.sound);
    assert_eq!(s.max_push_per_hour, 20);
    assert_eq!(stage(&s, kind::APPROVAL, channel::NTFY), NotificationStage::Off);
    assert_eq!(stage(&s, kind::APPROVAL, channel::WEB_PUSH), NotificationStage::Now);
    assert_eq!(stage(&s, kind::APPROVAL, channel::EMAIL), AFTER_5);
    assert_eq!(stage(&s, kind::NEEDS_INPUT, channel::WEB_PUSH), AFTER_10);
}

#[test]
fn the_master_switch_turns_every_channel_off() {
    let s = parse(&json!({ "notifications": { "enabled": false } }));
    for c in CHANNELS {
        assert_eq!(stage(&s, kind::SYSTEM_ALERT, c), NotificationStage::Off, "{c}");
    }
}

#[test]
fn a_malformed_block_reads_as_the_defaults() {
    let s = parse(&json!({ "notifications": { "enabled": "yes", "routes": 3 } }));
    assert_eq!(s, NotificationSettings::default());
    assert_eq!(parse(&json!({})), NotificationSettings::default());
}

#[test]
fn the_legacy_sound_switch_is_read_until_the_new_one_is_set() {
    let old = json!({ "display": { "notifySound": false } });
    assert!(!parse(&old).sound);
    let both = json!({ "display": { "notifySound": false }, "notifications": { "sound": true } });
    assert!(parse(&both).sound);

    let mut data = old;
    migrate_sound(&mut data);
    assert_eq!(data["notifications"]["sound"], json!(false));
    let mut set = both;
    migrate_sound(&mut set);
    assert_eq!(set["notifications"]["sound"], json!(true), "the new key wins");
}

#[test]
fn resolved_fills_every_channel_and_cell() {
    let s = resolved(&NotificationSettings::default());
    assert_eq!(s.channels.len(), CHANNELS.len());
    assert!(!s.channels["email"].enabled);
    assert!(s.channels["ntfy"].enabled);
    assert_eq!(s.routes.len(), ROUTE_KINDS.len());
    for row in s.routes.values() {
        assert_eq!(row.len(), CHANNELS.len());
    }
    assert_eq!(s.routes["approval"]["ntfy"], AFTER_5);
    assert_eq!(s.routes["approval"]["in_app"], NotificationStage::Now);
    assert_eq!(s.routes["user_action"]["ntfy"], NotificationStage::Off);
}

#[test]
fn normalize_drops_unknown_keys_and_checks_ranges() {
    let raw = json!({
        "channels": { "signal": { "enabled": true }, "ntfy": { "enabled": false } },
        "routes": {
            "made_up": { "ntfy": { "mode": "now" } },
            "approval": { "fax": { "mode": "now" }, "ntfy": { "mode": "after", "minutes": 15 } },
        },
        "max_push_per_hour": 0,
        "quiet_hours": { "start": "22:00", "end": "07:00", "tz": "Europe/Paris" },
    });
    let s = normalize(&raw).unwrap();
    assert_eq!(s.channels.keys().collect::<Vec<_>>(), ["ntfy"]);
    assert_eq!(s.routes.keys().collect::<Vec<_>>(), ["approval"]);
    assert_eq!(s.routes["approval"].keys().collect::<Vec<_>>(), ["ntfy"]);
    assert_eq!(s.max_push_per_hour, 1);

    let zero = json!({ "routes": { "approval": { "ntfy": { "mode": "after", "minutes": 0 } } } });
    assert!(normalize(&zero).unwrap_err().contains("after must be"));
    let bad_time = json!({ "quiet_hours": { "start": "25:00", "end": "07:00", "tz": "UTC" } });
    assert!(normalize(&bad_time).unwrap_err().contains("start"));
    let bad_tz = json!({ "quiet_hours": { "start": "22:00", "end": "07:00", "tz": "x; DROP" } });
    assert!(normalize(&bad_tz).unwrap_err().contains("tz"));
    assert!(normalize(&json!({ "enabled": "on" })).is_err());
}
