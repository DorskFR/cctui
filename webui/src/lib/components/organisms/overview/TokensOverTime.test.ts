import { describe, it, expect, afterEach } from 'vitest';
import { mount, unmount } from 'svelte';
import type { UsageBucket } from '@bindings/UsageBucket';
import TokensOverTime from './TokensOverTime.svelte';

// Component render test: mount the stacked-bar chart with fixture
// data and assert the DOM reflects the zero-filled series (30 daily bars, with
// stacked segments on the days that carry usage). Uses Svelte 5's `mount` in
// happy-dom — no extra test-library dependency.
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
	...over
});

describe('TokensOverTime', () => {
	it('renders one bar per day and stacked segments for the current bucket', () => {
		const el = render([bucket({ input: 100, output: 40, cache_read: 20 })], 30, 'day');
		const cols = el.querySelectorAll('.col');
		expect(cols.length).toBe(30); // zero-filled to a full 30-day series

		// Every column has the three segment nodes; the last (today) has non-zero
		// heights while an untouched earlier column is flat (0%).
		const last = cols[cols.length - 1];
		const inSeg = last.querySelector('.seg.in') as HTMLElement;
		const outSeg = last.querySelector('.seg.out') as HTMLElement;
		const cacheSeg = last.querySelector('.seg.cache') as HTMLElement;
		// Segments scale against the peak STACKED total (100+40+20=160), so the
		// single populated column's segments fill it to 62.5/25/12.5%.
		expect(inSeg.style.height).toBe('62.5%');
		expect(outSeg.style.height).toBe('25%');
		expect(cacheSeg.style.height).toBe('12.5%');

		const first = cols[0];
		expect((first.querySelector('.seg.in') as HTMLElement).style.height).toBe('0%');

		// Legend labels the three series.
		expect(el.textContent).toContain('Input');
		expect(el.textContent).toContain('Output');
	});

	it('renders 24 bars for the hourly range', () => {
		const el = render([], 1, 'hour');
		expect(el.querySelectorAll('.col').length).toBe(24);
	});
});
