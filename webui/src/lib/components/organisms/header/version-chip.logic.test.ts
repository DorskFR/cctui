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

	it('compares pre-release builds like any release', () => {
		expect(needsReload('0.33.0-beta.1', '0.33.0-beta.2', false)).toBe(true);
		expect(needsReload('0.33.0-beta.2', '0.33.0-beta.2', false)).toBe(false);
	});

	it('never asks on preview or dirty builds, which no server can match', () => {
		expect(needsReload('pr-812', '0.33.0', false)).toBe(false);
		expect(needsReload('0.33.0-dirty', '0.33.0', false)).toBe(false);
		expect(needsReload('0.33.0-beta.1-dirty', '0.33.0-beta.1', false)).toBe(false);
	});

	it('ignores dev builds and an unknown server', () => {
		expect(needsReload('dev', '0.32.0', false)).toBe(false);
		expect(needsReload('0.32.0', undefined, false)).toBe(false);
	});
});
