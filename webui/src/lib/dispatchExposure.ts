import type { DispatchExposure } from '@bindings/DispatchExposure';
import type { HumanGate } from '@bindings/HumanGate';
import { m } from '$lib/paraglide/messages';

export type ExposureFlag = {
	key: 'untrusted_input' | 'sensitive_data' | 'outward';
	label: string;
	on: boolean;
};

export type ExposureView = {
	flags: ExposureFlag[];
	gate: string | null;
	ungated: boolean;
};

const GATE_LABEL: Record<HumanGate, () => string> = {
	no_push: m.exposure_gate_no_push,
	draft_pr_only: m.exposure_gate_draft_pr_only,
	outward_held_for_approval: m.exposure_gate_outward_held_for_approval
};

export function exposureView(exposure: DispatchExposure | null | undefined): ExposureView | null {
	if (!exposure) return null;
	const flags: ExposureFlag[] = [
		{ key: 'untrusted_input', label: m.exposure_untrusted_input(), on: exposure.untrusted_input },
		{ key: 'sensitive_data', label: m.exposure_sensitive_data(), on: exposure.sensitive_data },
		{ key: 'outward', label: m.exposure_outward(), on: exposure.outward }
	];
	const gate = exposure.human_gate ? GATE_LABEL[exposure.human_gate]() : null;
	return { flags, gate, ungated: gate === null && flags.every((f) => f.on) };
}
