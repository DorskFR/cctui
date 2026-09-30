import { describe, expect, it } from 'vitest';
import type { SessionEndReason } from '@bindings/SessionEndReason';
import { endBadgeText } from '$lib/sessionEnd';
import { sessionHref, shouldToast, toastDetail } from '$lib/sessionFailureToast';
import { parityFixture } from './fixtures';

type Fixture = {
	shouldToast: { reason: string; out: boolean }[];
	toastDetail: { detail: string | null; out: string | null }[];
	endBadgeText: { reason: string; label: string; detail: string | null; out: string }[];
	sessionHref: { session_id: string; out: string }[];
};

const fx = parityFixture<Fixture>('sessionFailureToast');

describe('sessionFailureToast parity fixtures', () => {
	it('shouldToast', () => {
		for (const c of fx.shouldToast)
			expect(shouldToast(c.reason as SessionEndReason), JSON.stringify(c)).toBe(c.out);
	});
	it('toastDetail', () => {
		for (const c of fx.toastDetail) expect(toastDetail(c.detail), JSON.stringify(c)).toBe(c.out);
	});
	it('endBadgeText', () => {
		for (const c of fx.endBadgeText)
			expect(
				endBadgeText(c.reason as SessionEndReason, c.label, c.detail),
				JSON.stringify(c)
			).toBe(c.out);
	});
	it('sessionHref', () => {
		for (const c of fx.sessionHref) expect(sessionHref(c.session_id), JSON.stringify(c)).toBe(c.out);
	});
});
