// Instance status and server self-update decisions, shared with the TUI
// through `fixtures/parity/instance.json`.
//
// These return paraglide message *ids* rather than sentences, so the TUI can
// agree on which thing to say without the fixture pinning English.
import type { UpdateHookPhase } from '@bindings/UpdateHookPhase';

export type PhaseTone = 'success' | 'faint' | 'danger';

/** The probe withholds anything older, so a present-and-different tag is enough. */
export function updateAvailable(version: string, latest: string | null | undefined): boolean {
	return !!latest && latest !== version;
}

export function phaseTone(phase: UpdateHookPhase | null | undefined): PhaseTone {
	if (phase === 'succeeded') return 'success';
	if (phase === 'running' || phase === 'verifying') return 'faint';
	return 'danger';
}

export function phaseMessage(phase: UpdateHookPhase): string {
	return `update_run_phase_${phase}`;
}

export function hintMessage(isAdmin: boolean, ready: boolean, hook: boolean): string {
	if (!isAdmin) return 'update_ask_admin';
	if (!ready) return 'update_no_target_hint';
	return hook ? 'update_hook_hint' : 'update_agent_hint';
}

export function confirmMessage(hook: boolean): string {
	return hook ? 'update_confirm_body_hook' : 'update_confirm_body';
}

export function badgeMessage(hook: boolean): string {
	return hook ? 'update_hook_badge' : 'update_agent_badge';
}

/** The trigger is admin-scoped server-side and needs a configured machine. */
export function canLaunch(isAdmin: boolean, ready: boolean, available: boolean): boolean {
	return isAdmin && ready && available;
}
