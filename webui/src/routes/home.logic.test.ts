import { describe, it, expect } from 'vitest';
import type { MachineRow } from '@bindings/MachineRow';
import type { MachineLiveness } from '@bindings/MachineLiveness';
import type { SessionStats } from '@bindings/SessionStats';
import { asUsage, buildMetricTiles, machinesOnline } from './home.logic';

const machine = (
	id: string,
	liveness: MachineLiveness,
	revoked_at: string | null = null
): MachineRow => ({
	id,
	user_id: 'u1',
	name: id,
	display_name: null,
	first_seen_at: '2026-01-01T00:00:00Z',
	last_seen_at: '2026-01-01T00:00:00Z',
	revoked_at,
	kind: 'persistent',
	hue: null,
	key_preview: null,
	liveness
});

const stats: SessionStats = { total: 12193, live: 8, needs_input: 1, archived: 12076 };

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
	it('counts online against enrolled, ignoring revoked machines', () => {
		const rows = [
			machine('a', 'online'),
			machine('b', 'stale'),
			machine('c', 'offline'),
			machine('d', 'online', '2026-02-01T00:00:00Z')
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
		expect(tiles.map((t) => t.key)).toEqual(['live', 'needs_input', 'machines', 'sessions']);
		expect(tiles[0].value).toBe(8);
		expect(tiles[1]).toMatchObject({ value: 1, warn: true });
		expect(tiles[2]).toMatchObject({ value: 1, suffix: '/2' });
		expect(tiles[3]).toMatchObject({ value: 12193, sub: 12076 });
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
