// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import MaintenanceScreen from './MaintenanceScreen.svelte';

let comp: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(10_000);
});
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.useRealTimers();
});

function render(props: { nextRetryAt: number | null; probing: boolean; onretry?: () => void }) {
	comp = mount(MaintenanceScreen, {
		target: document.body,
		props: { onretry: () => {}, ...props }
	});
	flushSync();
}

const text = () => document.body.textContent ?? '';
const retryButton = () =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.includes('Retry now'));

describe('MaintenanceScreen', () => {
	it('explains the outage and counts down to the next retry', () => {
		render({ nextRetryAt: 18_000, probing: false });
		expect(text()).toContain('cctui is starting up or under maintenance');
		expect(text()).toContain('Retrying in 8 s');
		vi.advanceTimersByTime(3_000);
		flushSync();
		expect(text()).toContain('Retrying in 5 s');
	});

	it('shows the probe in flight and disables the retry', () => {
		render({ nextRetryAt: null, probing: true });
		expect(text()).toContain('Retrying…');
		expect(retryButton()?.disabled).toBe(true);
	});

	it('retry now calls back', () => {
		const onretry = vi.fn();
		render({ nextRetryAt: 20_000, probing: false, onretry });
		retryButton()?.click();
		expect(onretry).toHaveBeenCalledTimes(1);
	});
});
