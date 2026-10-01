import { describe, expect, it } from 'vitest';
import type { UpdateHookPhase } from '@bindings/UpdateHookPhase';
import {
	badgeMessage,
	canLaunch,
	confirmMessage,
	hintMessage,
	phaseMessage,
	phaseTone,
	updateAvailable
} from '$lib/instance';
import { parityFixture } from './fixtures';

type Fixture = {
	updateAvailable: { version: string; latest: string | null; out: boolean }[];
	phaseTone: { phase: UpdateHookPhase | null; out: string }[];
	phaseMessage: { phase: UpdateHookPhase; out: string }[];
	hintMessage: { isAdmin: boolean; ready: boolean; hook: boolean; out: string }[];
	confirmMessage: { hook: boolean; out: string }[];
	badgeMessage: { hook: boolean; out: string }[];
	canLaunch: { isAdmin: boolean; ready: boolean; available: boolean; out: boolean }[];
};

const fx = parityFixture<Fixture>('instance');

describe('instance parity', () => {
	it('updateAvailable', () => {
		for (const c of fx.updateAvailable) {
			expect(updateAvailable(c.version, c.latest), JSON.stringify(c)).toBe(c.out);
		}
	});

	it('phaseTone', () => {
		for (const c of fx.phaseTone) {
			expect(phaseTone(c.phase), JSON.stringify(c)).toBe(c.out);
		}
	});

	it('phaseMessage', () => {
		for (const c of fx.phaseMessage) {
			expect(phaseMessage(c.phase), JSON.stringify(c)).toBe(c.out);
		}
	});

	it('hintMessage', () => {
		for (const c of fx.hintMessage) {
			expect(hintMessage(c.isAdmin, c.ready, c.hook), JSON.stringify(c)).toBe(c.out);
		}
	});

	it('confirmMessage', () => {
		for (const c of fx.confirmMessage) {
			expect(confirmMessage(c.hook), JSON.stringify(c)).toBe(c.out);
		}
	});

	it('badgeMessage', () => {
		for (const c of fx.badgeMessage) {
			expect(badgeMessage(c.hook), JSON.stringify(c)).toBe(c.out);
		}
	});

	it('canLaunch', () => {
		for (const c of fx.canLaunch) {
			expect(canLaunch(c.isAdmin, c.ready, c.available), JSON.stringify(c)).toBe(c.out);
		}
	});
});
