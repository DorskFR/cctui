import { describe, expect, it } from 'vitest';
import type { StorageLifecycleRules } from '@bindings/StorageLifecycleRules';
import { canSave, draftFrom, draftValid, enablesAnything, fmtBytes, rulesFrom, rulesKey } from './serverStorage.logic';

const none: StorageLifecycleRules = { trim: null, drop: null };

describe('fmtBytes', () => {
	it('scales to the largest unit under 1024', () => {
		expect(fmtBytes(0)).toBe('0 B');
		expect(fmtBytes(1536)).toBe('1.5 KB');
		expect(fmtBytes(10 * 1024 ** 3)).toBe('10 GB');
		expect(fmtBytes(2.5 * 1024 ** 4)).toBe('2.5 TB');
	});
});

describe('lifecycle draft', () => {
	it('defaults to keeping everything', () => {
		const d = draftFrom(none);
		expect(d.trimOn || d.dropOn).toBe(false);
		expect(rulesFrom(d)).toEqual(none);
		expect(enablesAnything(rulesFrom(d))).toBe(false);
	});

	it('round-trips enabled rules in KB', () => {
		const rules: StorageLifecycleRules = {
			trim: { enabled: true, after_days: 14, above_bytes: 32 * 1024 },
			drop: { enabled: true, after_days: 365 }
		};
		expect(rulesFrom(draftFrom(rules))).toEqual(rules);
	});

	it('treats a stored but disabled rule as absent', () => {
		const off: StorageLifecycleRules = { trim: { enabled: false, after_days: 7, above_bytes: 4096 }, drop: null };
		expect(rulesKey(off)).toBe(rulesKey(none));
		expect(draftFrom(off).trimOn).toBe(false);
	});

	it('rejects out-of-range values only for switched-on rules', () => {
		const d = { ...draftFrom(none), trimDays: 0, dropDays: 1.5 };
		expect(draftValid(d)).toBe(true);
		expect(draftValid({ ...d, trimOn: true })).toBe(false);
		expect(draftValid({ ...d, dropOn: true })).toBe(false);
		expect(draftValid({ ...draftFrom(none), trimOn: true, trimKb: 64 * 1024 + 1 })).toBe(false);
	});
});

describe('canSave', () => {
	const on = { ...draftFrom(none), trimOn: true };

	it('needs a preview of exactly the draft before enabling a rule', () => {
		expect(canSave(on, none, null)).toBe(false);
		expect(canSave(on, none, rulesKey(rulesFrom({ ...on, trimKb: 99 })))).toBe(false);
		expect(canSave(on, none, rulesKey(rulesFrom(on)))).toBe(true);
	});

	it('lets an admin switch everything off without a preview', () => {
		const saved = rulesFrom(on);
		expect(canSave(draftFrom(none), saved, null)).toBe(true);
	});

	it('has nothing to save when the draft equals the saved rules', () => {
		const saved = rulesFrom(on);
		expect(canSave(on, saved, rulesKey(saved))).toBe(false);
	});
});
