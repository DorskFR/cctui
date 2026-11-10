import { describe, expect, it } from 'vitest';
import type { Feature } from '@bindings/Feature';
import {
	COLOR_DIMENSIONS,
	effectiveColorBy,
	effectiveGroupBy,
	effectiveSections,
	GROUP_DIMENSIONS,
	offeredDimensions,
	offeredSections,
	SECTIONS,
	type Section
} from './sessions.logic';

const all = () => true;
const without =
	(...off: Feature[]) =>
	(f: Feature) =>
		!off.includes(f);

describe('section toggles by feature', () => {
	it('offers every section with every feature on', () => {
		expect(offeredSections(all)).toEqual(SECTIONS);
	});

	it('offers no starred section without organising, and no drafts section without drafts', () => {
		const values = offeredSections(without('organising', 'drafts')).map((s) => s.value);
		expect(values).toEqual(['live', 'dispatched', 'archived', 'unread']);
	});

	it('leaves the stored set untouched with every feature on', () => {
		const stored = new Set<Section>(['live']);
		expect(effectiveSections(stored, all)).toBe(stored);
	});

	it('keeps pinned rows listed when the starred toggle is gone, even if it was turned off', () => {
		const shown = effectiveSections(new Set<Section>(['live', 'dispatched']), without('organising'));
		expect(shown.has('starred')).toBe(true);
		expect(shown.has('live')).toBe(true);
	});

	it('asks for no draft section when drafts is off, even if it was turned on', () => {
		const shown = effectiveSections(new Set<Section>(['live', 'drafts']), without('drafts'));
		expect(shown.has('drafts')).toBe(false);
	});
});

describe('color and group dimensions by feature', () => {
	it('offers every dimension with every feature on', () => {
		expect(offeredDimensions(COLOR_DIMENSIONS, all)).toEqual(COLOR_DIMENSIONS);
		expect(offeredDimensions(GROUP_DIMENSIONS, all)).toEqual(GROUP_DIMENSIONS);
	});

	it('offers no label dimension without organising', () => {
		const off = without('organising');
		expect(offeredDimensions(COLOR_DIMENSIONS, off).map((d) => d.value)).not.toContain('label');
		expect(offeredDimensions(GROUP_DIMENSIONS, off).map((d) => d.value)).not.toContain('label');
		expect(offeredDimensions(GROUP_DIMENSIONS, off).map((d) => d.value)).toContain('room');
	});

	it('offers no room dimension without rooms', () => {
		expect(offeredDimensions(GROUP_DIMENSIONS, without('rooms')).map((d) => d.value)).not.toContain('room');
	});

	it('falls a stored label preference back to the default', () => {
		expect(effectiveColorBy('label', without('organising'))).toBe('none');
		expect(effectiveGroupBy('label', without('organising'))).toBe('status');
		expect(effectiveGroupBy('room', without('rooms'))).toBe('status');
	});

	it('keeps a stored preference the server can fill', () => {
		expect(effectiveColorBy('label', all)).toBe('label');
		expect(effectiveGroupBy('working_dir', without('organising'))).toBe('working_dir');
	});
});
