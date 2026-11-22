use super::followup_name;

#[test]
fn a_followup_name_bumps_the_counter_of_a_user_set_name() {
    assert_eq!(followup_name(Some("triage"), true).as_deref(), Some("triage 2"));
    assert_eq!(followup_name(Some("triage 2"), true).as_deref(), Some("triage 3"));
    assert_eq!(followup_name(Some("triage 999"), true).as_deref(), Some("triage 1000"));
    assert_eq!(followup_name(Some("triage 1000"), true).as_deref(), Some("triage 1000 2"));
    assert_eq!(followup_name(Some("v 02"), true).as_deref(), Some("v 02 2"));
    assert_eq!(followup_name(Some("triage"), false), None);
    assert_eq!(followup_name(Some("  "), true), None);
    assert_eq!(followup_name(None, true), None);
}
