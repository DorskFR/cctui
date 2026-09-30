import { describe, expect, it } from 'vitest';
import {
	compact,
	hashHue,
	machineInitial,
	machineTint,
	modelAbbrev,
	modelFamily,
	modelShort,
	statusBadgeTone,
	uptime,
	usd
} from '$lib/format';
import { parityFixture } from './fixtures';

type Fixture = {
	compact: { n: number; out: string }[];
	uptime: { secs: number; out: string }[];
	statusBadgeTone: { status: string; out: string }[];
	modelShort: { model: string; out: string }[];
	modelFamily: { model: string; out: string }[];
	modelAbbrev: { model: string; out: string }[];
	machineInitial: { label: string; out: string }[];
	usd: { n: number; out: string }[];
	hashHue: { s: string; out: number }[];
	machineTint: { label: string; hue: number | null; out: string }[];
};

const fx = parityFixture<Fixture>('format');

describe('format parity fixtures', () => {
	it('compact', () => {
		for (const c of fx.compact) expect(compact(c.n), JSON.stringify(c)).toBe(c.out);
	});
	it('uptime', () => {
		for (const c of fx.uptime) expect(uptime(c.secs), JSON.stringify(c)).toBe(c.out);
	});
	it('statusBadgeTone', () => {
		for (const c of fx.statusBadgeTone) expect(statusBadgeTone(c.status), JSON.stringify(c)).toBe(c.out);
	});
	it('modelShort', () => {
		for (const c of fx.modelShort) expect(modelShort(c.model), JSON.stringify(c)).toBe(c.out);
	});
	it('modelFamily', () => {
		for (const c of fx.modelFamily) expect(modelFamily(c.model), JSON.stringify(c)).toBe(c.out);
	});
	it('modelAbbrev', () => {
		for (const c of fx.modelAbbrev) expect(modelAbbrev(c.model), JSON.stringify(c)).toBe(c.out);
	});
	it('machineInitial', () => {
		for (const c of fx.machineInitial) expect(machineInitial(c.label), JSON.stringify(c)).toBe(c.out);
	});
	it('usd', () => {
		for (const c of fx.usd) expect(usd(c.n), JSON.stringify(c)).toBe(c.out);
	});
	it('hashHue', () => {
		for (const c of fx.hashHue) expect(hashHue(c.s), JSON.stringify(c)).toBe(c.out);
	});
	it('machineTint', () => {
		for (const c of fx.machineTint) expect(machineTint(c.label, c.hue), JSON.stringify(c)).toBe(c.out);
	});
});
