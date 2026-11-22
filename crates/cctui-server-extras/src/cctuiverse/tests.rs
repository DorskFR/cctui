use super::*;

#[test]
fn remote_refs_round_trip() {
    let id = Uuid::new_v4();
    assert_eq!(parse_remote_ref(&remote_ref(id)), Some(id));
    assert_eq!(parse_remote_ref(&format!(" remote:{id} ")), Some(id));
    assert_eq!(parse_remote_ref(&id.to_string()), None);
    assert_eq!(parse_remote_ref("remote:nope"), None);
}

#[test]
fn labels_are_bounded_and_markup_free() {
    assert_eq!(clean_label("  alice  ").as_deref(), Some("alice"));
    assert_eq!(clean_label(&"é".repeat(80)).map(|l| l.chars().count()), Some(80));
    for bad in ["", "   ", "\u{202e}", "<b>", "say \"hi\"", &"x".repeat(81)] {
        assert!(clean_label(bad).is_none(), "{bad:?}");
    }
    assert_eq!(clean_label("a\nb\u{202e}c\u{200b}").as_deref(), Some("abc"));
}

#[test]
fn peer_names_are_cleaned_not_refused() {
    assert_eq!(sanitize_label(" Review \"<v2>\" ").as_deref(), Some("Review v2"));
    assert_eq!(sanitize_label(&"y".repeat(300)).map(|l| l.chars().count()), Some(80));
    assert_eq!(sanitize_label("you\u{202e} (human)").as_deref(), Some("you (human)"));
    assert!(sanitize_label("<>\"\u{200b}").is_none());
}

#[test]
fn preambles_name_the_peer_and_its_id() {
    let id = Uuid::from_u128(9);
    let p = session_preamble(id, "bob \"<x>\"");
    assert!(p.starts_with("<cctuiverse-linked peer=\"bob x\" id=\"remote:"));
    assert!(p.contains(&format!("CctuiPeers as remote:{id}")));
    assert!(p.trim_end().ends_with("</cctuiverse-linked>"));
    let r = room_joiner_preamble(id, "alice", "ops");
    assert!(r.contains("room=\"ops\"") && r.contains("CctuiRoom"));
    assert!(room_host_preamble(id, "bob", "ops").contains("\"bob (remote)\""));
}

#[test]
fn auto_forward_ids_are_stable_per_message() {
    assert_eq!(derived_message_id("s", 1), derived_message_id("s", 1));
    assert_ne!(derived_message_id("s", 1), derived_message_id("s", 2));
    assert_ne!(derived_message_id("s", 1), derived_message_id("t", 1));
}
