import { describe, expect, it } from 'vitest';
import type { PoolUsageMember } from '@bindings/PoolUsageMember';
import type { PoolUsageWindow } from '@bindings/PoolUsageWindow';
import {
	familyLabel,
	fmtDemand,
	poolWindowLabel,
	poolWindowPace,
	projectionState,
	weightsNote
} from './pools-usage.logic';

const member = (weight: number): PoolUsageMember => ({
	account_id: `a${weight}`,
	name: 'x',
	emoji: null,
	weight,
	usage_known: true
});

const window = (over: Partial<PoolUsageWindow> = {}): PoolUsageWindow => ({
	key: 'weekly_all',
	kind: 'weekly_all',
	label: 'Weekly (all models)',
	model_display_name: null,
	level_pct: 43,
	expected_pct: 31,
	ratio: 1.38,
	next_reset_at: '2026-09-15T04:00:00Z',
	members: [],
	projection: null,
	projection_unavailable: 'insufficient_history',
	...over
});

describe('familyLabel', () => {
	it('names the harness behind a family', () => {
		expect(familyLabel('anthropic')).toBe('Claude');
		expect(familyLabel('openai')).toBe('Codex');
		expect(familyLabel('fireworks')).toBe('Fireworks');
	});
	it('passes an unknown family through', () => {
		expect(familyLabel('mistral')).toBe('mistral');
	});
});

describe('poolWindowLabel', () => {
	it('uses the short canonical label', () => {
		expect(poolWindowLabel(window({ key: 'session', kind: 'session' }))).toBe('5h');
		expect(poolWindowLabel(window())).toBe('7d');
	});
	it('names the model of a scoped weekly window', () => {
		expect(
			poolWindowLabel(
				window({ key: 'weekly_model:fable', kind: 'weekly_scoped', model_display_name: 'Fable' })
			)
		).toBe('Fable');
	});
});

describe('poolWindowPace', () => {
	it('mirrors the aggregate ratio and expected share', () => {
		const pace = poolWindowPace(window());
		expect(pace).not.toBeNull();
		expect(pace?.ratio).toBe(1.38);
		expect(pace?.expected_pct).toBe(31);
		expect(pace?.elapsed_fraction).toBeCloseTo(0.31);
		expect(pace?.projected_wall_at).toBeUndefined();
	});
	it('carries the projected wall and slope base when the server has them', () => {
		const pace = poolWindowPace(
			window({
				projection: {
					wall_at: '2026-09-13T12:00:00Z',
					first_member_wall_at: null,
					demand_pct_per_hour: 3.2,
					slope_hours: 2.5,
					min_margin_pct: 0
				},
				projection_unavailable: null
			})
		);
		expect(pace?.projected_wall_at).toBe('2026-09-13T12:00:00Z');
		expect(pace?.slope_hours).toBe(2.5);
	});
	it('is null without a ratio', () => {
		expect(poolWindowPace(window({ ratio: null }))).toBeNull();
	});
});

describe('projectionState', () => {
	const now = Date.parse('2026-09-11T12:00:00Z');
	it('reports insufficient history when the server sent no projection', () => {
		expect(projectionState(window(), now)).toEqual({ kind: 'insufficient' });
	});
	it('holds when the projection has no wall', () => {
		const w = window({
			projection: {
				wall_at: null,
				first_member_wall_at: null,
				demand_pct_per_hour: 0,
				slope_hours: 3,
				min_margin_pct: 57
			},
			projection_unavailable: null
		});
		expect(projectionState(w, now)).toEqual({ kind: 'holds' });
	});
	it('counts down to the wall', () => {
		const w = window({
			projection: {
				wall_at: '2026-09-11T14:00:00Z',
				first_member_wall_at: null,
				demand_pct_per_hour: 3,
				slope_hours: 3,
				min_margin_pct: 0
			},
			projection_unavailable: null
		});
		expect(projectionState(w, now)).toEqual({ kind: 'wall', ms: 2 * 3_600_000 });
	});
});

describe('weightsNote', () => {
	it('is silent when every member weighs one', () => {
		expect(weightsNote([member(1), member(1)])).toBeNull();
		expect(weightsNote([])).toBeNull();
	});
	it('lists the weights in member order otherwise', () => {
		expect(weightsNote([member(4), member(4), member(1)])).toBe('4 · 4 · 1');
		expect(weightsNote([member(2.5), member(1)])).toBe('2.5 · 1');
	});
});

describe('fmtDemand', () => {
	it('keeps one decimal under ten points an hour', () => {
		expect(fmtDemand(3.21)).toBe('3.2');
		expect(fmtDemand(11.7)).toBe('12');
	});
});
