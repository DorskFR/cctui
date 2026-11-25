import { describe, expect, it } from 'vitest';
import { needsReload } from './version-chip.logic.ts';

describe('needsReload', () => {
	it('stays quiet while the ui and the server agree', () => {
		expect(needsReload('0.32.0', '0.32.0', false)).toBe(false);
	});

	it('asks for a reload when the builds differ', () => {
		expect(needsReload('0.31.0', '0.32.0', false)).toBe(true);
	});

	it('asks for a reload once a newer ui has been deployed', () => {
		expect(needsReload('0.32.0', '0.32.0', true)).toBe(true);
	});

	it('ignores dev builds and an unknown server', () => {
		expect(needsReload('dev', '0.32.0', false)).toBe(false);
		expect(needsReload('0.32.0', undefined, false)).toBe(false);
	});
});
