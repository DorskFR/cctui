import { describe, expect, it } from 'vitest';
import type { ToolGuardState } from '@bindings/ToolGuardState';
import { canOfferBypass, guardOff, nextBypassStep } from './tool-guard-bypass.logic';

const state = (over: Partial<ToolGuardState> = {}): ToolGuardState => ({
	bypass: false,
	by: null,
	at: null,
	policy_active: true,
	can_toggle: true,
	...over
});

describe('canOfferBypass', () => {
	it('offers the bypass to a viewer who may lift an active policy', () => {
		expect(canOfferBypass(state())).toBe(true);
	});

	it('hides it from a viewer who does not own the account', () => {
		expect(canOfferBypass(state({ can_toggle: false }))).toBe(false);
		expect(canOfferBypass(state({ can_toggle: false, bypass: true }))).toBe(false);
	});

	it('hides it when no policy scans the session', () => {
		expect(canOfferBypass(state({ policy_active: false }))).toBe(false);
	});

	it('keeps an active bypass switchable after the policy is gone', () => {
		expect(canOfferBypass(state({ policy_active: false, bypass: true }))).toBe(true);
	});

	it('hides it while unknown or on an archived session', () => {
		expect(canOfferBypass(undefined)).toBe(false);
		expect(canOfferBypass(null)).toBe(false);
		expect(canOfferBypass(state(), true)).toBe(false);
	});
});

describe('guardOff', () => {
	it('is set only by an explicit bypass flag', () => {
		expect(guardOff({ tool_guard_bypass: true })).toBe(true);
		expect(guardOff({ tool_guard_bypass: false })).toBe(false);
		expect(guardOff({})).toBe(false);
		expect(guardOff(null)).toBe(false);
	});
});

describe('nextBypassStep', () => {
	it('asks before committing', () => {
		expect(nextBypassStep('idle', 'commit')).toBe('idle');
		expect(nextBypassStep('idle', 'ask')).toBe('confirm');
		expect(nextBypassStep('confirm', 'commit')).toBe('busy');
		expect(nextBypassStep('busy', 'settled')).toBe('idle');
	});

	it('cancels a pending confirm but not an in-flight request', () => {
		expect(nextBypassStep('confirm', 'cancel')).toBe('idle');
		expect(nextBypassStep('busy', 'cancel')).toBe('busy');
		expect(nextBypassStep('busy', 'ask')).toBe('busy');
	});
});
