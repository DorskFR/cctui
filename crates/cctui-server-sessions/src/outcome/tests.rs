use cctui_proto::api::outcome::{OutcomeSource, OutcomeStatus};

use super::turn_end::{ENDED_WITHOUT_OUTCOME_HEADLINE, TurnFacts, decide};

fn facts(final_text: Option<&str>, open: i64, children: i64) -> TurnFacts {
    TurnFacts {
        declared: false,
        final_text: final_text.map(str::to_owned),
        open_user_actions: open,
        live_children: children,
    }
}

#[test]
fn a_turn_that_already_declared_an_outcome_infers_nothing() {
    let declared = TurnFacts { declared: true, ..facts(Some("result: done"), 0, 0) };
    assert!(decide(&declared).is_none());
}

#[test]
fn a_result_line_in_the_final_text_becomes_a_text_outcome() {
    let out = decide(&facts(Some("All good.\nresult: shipped the fix"), 0, 0)).unwrap();
    assert_eq!(out.status, OutcomeStatus::Result);
    assert_eq!(out.headline, "shipped the fix");
    assert_eq!(out.source, OutcomeSource::Text);
    let pending = decide(&facts(Some("needs input: which env?"), 2, 1)).unwrap();
    assert_eq!(pending.status, OutcomeStatus::NeedsInput, "a declaration beats pending work");
}

#[test]
fn a_quiet_turn_with_nothing_pending_ended_without_outcome() {
    let out = decide(&facts(Some("I looked around."), 0, 0)).unwrap();
    assert_eq!(out.status, OutcomeStatus::EndedWithoutOutcome);
    assert_eq!(out.source, OutcomeSource::Inferred);
    assert_eq!(out.headline, ENDED_WITHOUT_OUTCOME_HEADLINE);
    assert!(out.normalized().is_ok());
    assert!(decide(&facts(None, 0, 0)).is_some());
}

#[test]
fn an_open_user_action_or_a_live_child_defers_the_inference() {
    assert!(decide(&facts(Some("waiting"), 1, 0)).is_none());
    assert!(decide(&facts(Some("waiting"), 0, 1)).is_none());
}
