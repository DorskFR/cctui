// @vitest-environment happy-dom
import { describe, it, expect, afterEach } from 'vitest';
import { mount, unmount } from 'svelte';
import type { UsageBucket } from '@bindings/UsageBucket';
import TokensOverTime from './TokensOverTime.svelte';

let host: HTMLElement | null = null;
let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	host?.remove();
	comp = null;
	host = null;
});

function render(buckets: UsageBucket[], days: number, granularity: 'hour' | 'day') {
	host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(TokensOverTime, { target: host, props: { buckets, days, granularity } });
	return host;
}

const bucket = (over: Partial<UsageBucket>): UsageBucket => ({
	bucket: new Date().toISOString(),
	input: 0,
	output: 0,
	cache_read: 0,
	cache_creation: 0,
	cost_usd: 0,
	...over
});

describe('TokensOverTime', () => {
	it('renders one bar per day, scaled against the peak total', () => {
		const el = render([bucket({ input: 100, output: 40, cache_read: 20 })], 30, 'day');
		const cols = el.querySelectorAll('.col');
		expect(cols.length).toBe(30);

		const last = cols[cols.length - 1];
		expect((last.querySelector('.bar') as HTMLElement).style.height).toBe('100%');
		expect((cols[0].querySelector('.bar') as HTMLElement).style.height).toBe('0%');
	});

	it('accents only the five most recent bars', () => {
		const el = render([], 30, 'day');
		const cols = [...el.querySelectorAll('.col')];
		expect(cols.filter((c) => c.classList.contains('recent')).length).toBe(5);
		expect(cols.slice(-5).every((c) => c.classList.contains('recent'))).toBe(true);
	});

	it('labels the axis every 7th day, ending on the newest bucket', () => {
		const el = render([], 30, 'day');
		const ticks = [...el.querySelectorAll('.tick')];
		const labelled = ticks.flatMap((t, i) => (t.textContent?.trim() ? [i] : []));
		expect(labelled).toEqual([1, 8, 15, 22, 29]);
	});

	it('captions the dollars of the range and prices each bar in its tooltip', () => {
		const el = render(
			[bucket({ input: 100, output: 40, cache_read: 20, cost_usd: 1.5 })],
			30,
			'day'
		);
		expect(el.textContent).toContain('$1.50');
		const cols = el.querySelectorAll('.col');
		expect(cols[cols.length - 1].getAttribute('title')).toContain('$1.50');
		expect(cols[0].getAttribute('title')).toContain('$0.00');
	});

	it('keeps the caption to tokens when nothing was priced', () => {
		const el = render([bucket({ input: 100 })], 30, 'day');
		expect(el.textContent).not.toContain('$');
	});

	it('renders 24 bars for the hourly range', () => {
		const el = render([], 1, 'hour');
		expect(el.querySelectorAll('.col').length).toBe(24);
	});
});
