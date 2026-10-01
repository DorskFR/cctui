import { describe, expect, it } from 'vitest';
import {
	imagePlaceholderLabel,
	isOnlyImages,
	scanImageMarkers,
	substituteImageMarkers
} from '$lib/images';
import { parityFixture } from './fixtures';

type Fixture = {
	scan: { text: string; out: { alt: string; id: string }[] }[];
	placeholderLabel: {
		name: string;
		dimensions: [number, number] | null;
		bytes: number | null;
		out: string;
	}[];
	substitute: { text: string; out: string }[];
	isOnlyImages: { text: string; out: boolean }[];
};

const fx = parityFixture<Fixture>('images');

describe('image markers parity', () => {
	it('scans the same markers', () => {
		for (const c of fx.scan) {
			expect(
				scanImageMarkers(c.text).map((m) => ({ alt: m.alt, id: m.id })),
				c.text
			).toEqual(c.out);
		}
	});

	it('words the placeholder the same', () => {
		for (const c of fx.placeholderLabel) {
			expect(imagePlaceholderLabel(c.name, c.dimensions, c.bytes), c.name).toBe(c.out);
		}
	});

	it('substitutes the same text', () => {
		for (const c of fx.substitute) {
			expect(substituteImageMarkers(c.text), c.text).toBe(c.out);
		}
	});

	it('agrees on an image-only body', () => {
		for (const c of fx.isOnlyImages) {
			expect(isOnlyImages(c.text), c.text).toBe(c.out);
		}
	});
});
