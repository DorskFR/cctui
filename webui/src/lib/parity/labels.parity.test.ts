import { describe, expect, it } from 'vitest';
import { LABEL_HUES, hueToColor, labelHue, labelTint, storedHue } from '$lib/labels';
import { parityFixture } from './fixtures';

type Fixture = {
	LABEL_HUES: number[];
	storedHue: { color: string; out: number | null }[];
	labelHue: { name: string; color: string; out: number }[];
	hueToColor: { hue: number | null; out: string }[];
	labelTint: { name: string; color: string; out: string }[];
};

const fx = parityFixture<Fixture>('labels');

describe('labels parity fixtures', () => {
	it('LABEL_HUES', () => {
		expect(LABEL_HUES).toEqual(fx.LABEL_HUES);
	});
	it('storedHue', () => {
		for (const c of fx.storedHue) expect(storedHue(c.color), JSON.stringify(c)).toBe(c.out);
	});
	it('labelHue', () => {
		for (const c of fx.labelHue)
			expect(labelHue({ name: c.name, color: c.color }), JSON.stringify(c)).toBe(c.out);
	});
	it('hueToColor', () => {
		for (const c of fx.hueToColor) expect(hueToColor(c.hue), JSON.stringify(c)).toBe(c.out);
	});
	it('labelTint', () => {
		for (const c of fx.labelTint)
			expect(labelTint({ name: c.name, color: c.color }), JSON.stringify(c)).toBe(c.out);
	});
});
