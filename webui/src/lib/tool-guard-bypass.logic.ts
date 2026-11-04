import type { ToolGuardState } from '@bindings/ToolGuardState';

/** An active bypass stays switchable even after the policy is gone. */
export const canOfferBypass = (
	state: ToolGuardState | null | undefined,
	archived = false
): boolean =>
	!!state && !archived && state.can_toggle && (state.policy_active || state.bypass);

export const guardOff = (session: { tool_guard_bypass?: boolean } | null | undefined) =>
	session?.tool_guard_bypass === true;

export type BypassStep = 'idle' | 'confirm' | 'busy';
export type BypassEvent = 'ask' | 'cancel' | 'commit' | 'settled';

export function nextBypassStep(step: BypassStep, ev: BypassEvent): BypassStep {
	switch (ev) {
		case 'ask':
			return step === 'idle' ? 'confirm' : step;
		case 'cancel':
			return step === 'confirm' ? 'idle' : step;
		case 'commit':
			return step === 'confirm' ? 'busy' : step;
		case 'settled':
			return 'idle';
	}
}
