import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { PoolUsageView } from '@bindings/PoolUsageView';
import type { PoolUsageWindow } from '@bindings/PoolUsageWindow';
import PoolUsageGauges from './PoolUsageGauges.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const soon = new Date(Date.now() + 4 * 24 * 3_600_000).toISOString();

const window = (over: Partial<PoolUsageWindow> = {}): PoolUsageWindow => ({
	key: 'weekly_all',
	kind: 'weekly_all',
	label: 'Weekly (all models)',
	model_display_name: null,
	level_pct: 43,
	expected_pct: 31,
	ratio: 1.38,
	next_reset_at: soon,
	members: [],
	projection: null,
	projection_unavailable: 'insufficient_history',
	...over
});

const view = (over: Partial<PoolUsageView> = {}, windows: PoolUsageWindow[] = [window()]): PoolUsageView => ({
	pool_id: 'p1',
	name: 'production',
	strategy: 'headroom',
	failover: false,
	families: [
		{
			family: 'anthropic',
			members: [
				{ account_id: 'a1', name: 'one', emoji: null, weight: 1, usage_known: true },
				{ account_id: 'a2', name: 'two', emoji: null, weight: 1, usage_known: true }
			],
			windows
		}
	],
	...over
});

const render = (usage: PoolUsageView) => {
	comp = mount(PoolUsageGauges, { target: document.body, props: { usage } });
	return document.body.textContent?.replace(/\s+/g, ' ') ?? '';
};

describe('PoolUsageGauges', () => {
	it('shows the family, its member count and the weighted level', () => {
		const text = render(view());
		expect(text).toContain('Claude · 2 accounts');
		expect(text).toContain('43%');
	});

	it('flames a pool burning past its linear budget', () => {
		expect(render(view())).toContain('🔥');
		document.body.innerHTML = '';
		if (comp) unmount(comp);
		comp = null;
		expect(render(view({}, [window({ ratio: 0.9 })]))).not.toContain('🔥');
	});

	it('warns when the pool has no failover, and stays quiet when it has', () => {
		expect(render(view())).toContain('Failover off');
		if (comp) unmount(comp);
		comp = null;
		document.body.innerHTML = '';
		expect(render(view({ failover: true }))).not.toContain('Failover off');
	});

	it('reads insufficient history in the row tooltip when no projection came', () => {
		render(view());
		const title = document.querySelector('.soft-limit')?.getAttribute('title') ?? '';
		expect(title).toContain('insufficient history');
	});

	it('reads the pool wall countdown and the demand behind it', () => {
		render(
			view({}, [
				window({
					projection: {
						wall_at: new Date(Date.now() + 2 * 3_600_000).toISOString(),
						first_member_wall_at: null,
						demand_pct_per_hour: 3.21,
						slope_hours: 2.6,
						min_margin_pct: 0
					},
					projection_unavailable: null
				})
			])
		);
		const title = document.querySelector('.soft-limit')?.getAttribute('title') ?? '';
		expect(title).toContain('pool wall in ~2h00');
		expect(title).toContain('demand 3.2 pt/h over a 3 h base');
	});

	it('notes the weights only when they differ from one', () => {
		const v = view();
		v.families[0].members[0].weight = 4;
		expect(render(v)).toContain('weights 4 · 1');
	});

	it('says usage unknown for a family without windows', () => {
		expect(render(view({}, []))).toContain('usage unknown');
	});
});
