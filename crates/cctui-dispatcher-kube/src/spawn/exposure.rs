//! The Rule of Two exposure a `WorkerProfile` declares, as the server records it.

use cctui_orchestrator::{Exposure, HumanGate};
use cctui_proto::api::{self, DispatchExposure};

#[must_use]
pub fn wire_exposure(exposure: Exposure, gate: Option<HumanGate>) -> DispatchExposure {
    DispatchExposure {
        untrusted_input: exposure.untrusted_input,
        sensitive_data: exposure.sensitive_data,
        outward: exposure.outward,
        human_gate: gate.map(wire_gate),
    }
}

fn wire_gate(gate: HumanGate) -> api::HumanGate {
    match gate.as_str() {
        "noPush" => api::HumanGate::NoPush,
        "draftPrOnly" => api::HumanGate::DraftPrOnly,
        "outwardHeldForApproval" => api::HumanGate::OutwardHeldForApproval,
        _ => api::HumanGate::Custom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Exposure = Exposure { untrusted_input: true, sensitive_data: true, outward: true };

    #[test]
    fn a_declared_exposure_carries_its_human_gate() {
        assert_eq!(
            wire_exposure(FULL, Some(HumanGate::DRAFT_PR_ONLY)),
            DispatchExposure {
                untrusted_input: true,
                sensitive_data: true,
                outward: true,
                human_gate: Some(api::HumanGate::DraftPrOnly),
            }
        );
        let lean = Exposure { untrusted_input: true, sensitive_data: false, outward: false };
        let mapped = wire_exposure(lean, None);
        assert!(mapped.untrusted_input && !mapped.sensitive_data && !mapped.outward);
        assert_eq!(mapped.human_gate, None);
    }

    #[test]
    fn every_gate_maps_to_its_wire_name() {
        for (gate, wire) in [
            (HumanGate::NO_PUSH, "no_push"),
            (HumanGate::DRAFT_PR_ONLY, "draft_pr_only"),
            (HumanGate::OUTWARD_HELD_FOR_APPROVAL, "outward_held_for_approval"),
            (HumanGate::new("reviewWithScreenshots").unwrap(), "custom"),
        ] {
            assert_eq!(serde_json::to_value(wire_gate(gate)).unwrap(), wire);
        }
    }

    #[test]
    fn a_recorded_gate_this_build_does_not_know_reads_back_as_custom() {
        for wire in ["custom", "some_future_gate"] {
            let gate: api::HumanGate = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(gate, api::HumanGate::Custom, "{wire}");
        }
    }
}
