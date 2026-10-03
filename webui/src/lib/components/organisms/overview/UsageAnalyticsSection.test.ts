// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const usageDays: number[] = [];
const cacheLossDays: number[] = [];
const usage = (granularity: 'hour' | 'day') => ({
	granularity,
	buckets: [{ bucket: new Date().toISOString(), input: 1, output: 1, cache_read: 1, cache_creation: 0 }],
	models: [],
	heatmap: []
});
vi.mock('$lib/queries', () => ({
	useUsageAnalytics: (days: () => number) => ({
		get isLoading() {
			return false;
		},
		get data() {
			const d = days();
			usageDays.push(d);
			return usage(d === 1 ? 'hour' : 'day');
		}
	}),
	useCacheLoss: (days: () => number) => ({
		isLoading: false,
		get data() {
			cacheLossDays.push(days());
			return [];
		}
	})
}));

import UsageAnalyticsSection from './UsageAnalyticsSection.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	usageDays.length = 0;
	cacheLossDays.length = 0;
});

function render() {
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(UsageAnalyticsSection, { target: host, props: {} });
	flushSync();
	return host;
}

function pick(host: HTMLElement, key: string) {
	const seg = [...host.querySelectorAll<HTMLElement>('[role="radio"]')].find(
		(el) => el.textContent?.trim() === key
	);
	if (!seg) throw new Error(`no ${key} segment`);
	seg.click();
	flushSync();
}

describe('UsageAnalyticsSection', () => {
	it('renders the range selector next to the charts it drives', () => {
		const host = render();
		const segs = [...host.querySelectorAll('[role="radio"]')].map((el) => el.textContent?.trim());
		expect(segs).toEqual(['24h', '7d', '30d']);
	});

	it('titles the chart per hour on 24h and per day otherwise', () => {
		const host = render();
		expect(host.textContent).toContain('Tokens per day');
		pick(host, '24h');
		expect(host.textContent).toContain('Tokens per hour');
		expect(host.textContent).not.toContain('Tokens per day');
	});

	it('scopes the cache-loss card to the selected range', () => {
		const host = render();
		expect(host.textContent).toContain('last 7 days');
		pick(host, '24h');
		expect(usageDays.at(-1)).toBe(1);
		expect(cacheLossDays.at(-1)).toBe(1);
		expect(host.textContent).toContain('last 24 hours');
	});
});
