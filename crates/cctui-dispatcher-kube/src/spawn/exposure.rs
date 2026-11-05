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

const fn wire_gate(gate: HumanGate) -> api::HumanGate {
    match gate {
        HumanGate::NoPush => api::HumanGate::NoPush,
        HumanGate::DraftPrOnly => api::HumanGate::DraftPrOnly,
        HumanGate::OutwardHeldForApproval => api::HumanGate::OutwardHeldForApproval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Exposure = Exposure { untrusted_input: true, sensitive_data: true, outward: true };

    #[test]
    fn a_declared_exposure_carries_its_human_gate() {
        assert_eq!(
            wire_exposure(FULL, Some(HumanGate::DraftPrOnly)),
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
            (HumanGate::NoPush, "no_push"),
            (HumanGate::DraftPrOnly, "draft_pr_only"),
            (HumanGate::OutwardHeldForApproval, "outward_held_for_approval"),
        ] {
            assert_eq!(serde_json::to_value(wire_gate(gate)).unwrap(), wire);
        }
    }
}
