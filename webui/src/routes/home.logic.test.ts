// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import type { MachineLiveness } from '@bindings/MachineLiveness';
import type { SessionStats } from '@bindings/SessionStats';
import { BUCKET_HREF, asUsage, buildMetricTiles, machinesOnline, needsInputTarget } from './home.logic';

const machine = (id: string, liveness: MachineLiveness): MachineResourcesRow => ({
	machine_id: id,
	name: id,
	display_name: null,
	hue: null,
	liveness,
	connected: liveness !== 'offline',
	last_seen_at: '2026-01-01T00:00:00Z',
	resources: null,
	updated_at: null
});

const stats: SessionStats = { total: 12193, live: 8, working: 8, needs_input: 1, archived: 12076, today: 3, yesterday: 4, week: 12, month: 30 };

describe('asUsage', () => {
	it('maps a window to the session-card readout, zeroed when absent', () => {
		expect(asUsage({ input: 3, output: 2, cache_read: 1 })).toMatchObject({
			tokens_in: 3,
			tokens_out: 2,
			cache_read_tokens: 1
		});
		expect(asUsage(undefined).tokens_in).toBe(0);
	});
});

describe('machinesOnline', () => {
	it('counts the caller-visible machines and their liveness', () => {
		const rows = [
			machine('a', 'online'),
			machine('b', 'stale'),
			machine('c', 'offline')
		];
		expect(machinesOnline(rows)).toEqual({ online: 1, total: 3 });
	});

	it('is zero with no machines', () => {
		expect(machinesOnline([])).toEqual({ online: 0, total: 0 });
	});
});

describe('buildMetricTiles', () => {
	it('builds the four headline tiles', () => {
		const tiles = buildMetricTiles(stats, [machine('a', 'online'), machine('b', 'offline')]);
		expect(tiles.map((t) => t.key)).toEqual(['working', 'needs_input', 'machines', 'sessions']);
		expect(tiles[0].value).toBe(8);
		expect(tiles[1]).toMatchObject({ value: 1, warn: true });
		expect(tiles[2]).toMatchObject({ value: 1, suffix: '/2' });
		expect(tiles[3]).toMatchObject({ value: 12193, sub: 12076 });
	});

	it('counts Working from the list buckets, not the deprecated live figure', () => {
		const tiles = buildMetricTiles({ ...stats, live: 0, working: 12 }, []);
		expect(tiles[0].value).toBe(12);
	});

	it('links the session tiles to the matching list bucket', () => {
		const tiles = buildMetricTiles(stats, []);
		expect(tiles[0].href).toBe(BUCKET_HREF.working);
		expect(tiles[1].href).toBe(BUCKET_HREF.needs_input);
		expect(tiles[2].href).toBeUndefined();
		expect(tiles[3].href).toBe('/sessions');
	});

	it('drops the warn tone when nothing needs input', () => {
		const tiles = buildMetricTiles({ ...stats, needs_input: 0 }, []);
		expect(tiles[1].warn).toBe(false);
	});

	it('renders zeros before the stats query resolves', () => {
		const tiles = buildMetricTiles(undefined, []);
		expect(tiles.every((t) => t.value === 0)).toBe(true);
		expect(tiles[2].suffix).toBe('/0');
	});
});

describe('needsInputTarget', () => {
	it('is null when nothing waits', () => {
		expect(needsInputTarget(undefined)).toBeNull();
		expect(needsInputTarget([{ id: 'a', attention: null }])).toBeNull();
	});

	it('opens the one waiting session directly', () => {
		expect(needsInputTarget([{ id: 'a', attention: null }, { id: 'b', attention: 'needs_input' }])).toBe(
			'/sessions/b'
		);
	});

	it('opens the filtered list when several wait', () => {
		const both = [
			{ id: 'a', attention: 'needs_input' as const },
			{ id: 'b', attention: 'needs_input' as const }
		];
		expect(needsInputTarget(both)).toBe(BUCKET_HREF.needs_input);
	});
});
