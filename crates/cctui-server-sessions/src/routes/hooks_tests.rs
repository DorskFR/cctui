use crate::inbox::preset::{PayloadFile, Plan, with_files, within_budget};

#[test]
fn a_batch_with_files_starts_a_session_instead_of_replying() {
    use cctui_proto::api::presets::OnStale;
    let reply = || Plan::Reply("s-1".into());
    assert_eq!(with_files(reply(), OnStale::Followup, false), reply());
    assert_eq!(
        with_files(reply(), OnStale::Followup, true),
        Plan::Followup { parent: "s-1".into() }
    );
    assert_eq!(with_files(reply(), OnStale::New, true), Plan::Spawn);
    assert_eq!(with_files(Plan::Spawn, OnStale::Followup, true), Plan::Spawn);
}

#[test]
fn attached_payloads_stop_at_the_batch_budget() {
    let file = |name: &str, len: usize| PayloadFile { name: name.into(), bytes: vec![b'x'; len] };
    let kept = within_budget(
        vec![file("a", 40), file("b", 70), file("a", 1), file("c", 60), file("d", 1)],
        100,
    );
    let names: Vec<&str> = kept.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a", "c"], "b and d overflow, the second a repeats a name");
    assert!(within_budget(vec![file("a", 101)], 100).is_empty());
}
