// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({ eventRetention: vi.fn(), setEventRetention: vi.fn() }));
vi.mock('#lib/queries/index.ts', () => api);

import { m } from '#lib/paraglide/messages.js';
import EventRetentionGroup from './EventRetentionGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
beforeEach(() => {
	vi.useFakeTimers();
	api.eventRetention.mockResolvedValue({ days: 30, default_days: 90, source: 'settings' });
	api.setEventRetention.mockImplementation(async (d: number | null) => ({
		days: d ?? 90,
		default_days: 90,
		source: d === null ? 'default' : 'settings'
	}));
});
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	vi.useRealTimers();
	vi.clearAllMocks();
	document.body.innerHTML = '';
});

async function render() {
	comp = mount(EventRetentionGroup, { target: document.body });
	await vi.runAllTimersAsync();
	flushSync();
}
function setDays(v: string) {
	const input = document.querySelector<HTMLInputElement>('[data-testid="event-retention-days"]');
	if (!input) throw new Error('no days input');
	input.value = v;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
}
const buttonText = (t: string) => [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === t);

describe('EventRetentionGroup', () => {
	it('saves a longer retention on its own', async () => {
		await render();
		setDays('120');
		await vi.advanceTimersByTimeAsync(700);
		expect(api.setEventRetention).toHaveBeenCalledWith(120);
	});

	it('asks before shortening and only saves once confirmed', async () => {
		await render();
		setDays('7');
		await vi.advanceTimersByTimeAsync(700);
		expect(api.setEventRetention).not.toHaveBeenCalled();
		buttonText(m.settings_event_retention_shorten({ days: 7 }))?.click();
		flushSync();
		const confirm = [...document.querySelectorAll('[role="dialog"] button, dialog button')].find(
			(b) => b.textContent?.trim() === m.settings_event_retention_shorten({ days: 7 })
		) as HTMLButtonElement | undefined;
		expect(confirm).toBeDefined();
		confirm?.click();
		await vi.runAllTimersAsync();
		expect(api.setEventRetention).toHaveBeenCalledWith(7);
	});

	it('puts the shorten action alone beside the source badge, on one line', async () => {
		await render();
		setDays('7');
		const acts = document.querySelector('[data-testid="event-retention-actions"]');
		const buttons = [...(acts?.querySelectorAll('button') ?? [])].map((b) => b.textContent?.trim());
		expect(buttons).toEqual([m.settings_event_retention_shorten({ days: 7 })]);
		expect(acts?.textContent).toContain(m.settings_source_settings());
	});

	it('offers Reset once nothing is being shortened', async () => {
		await render();
		expect(buttonText(m.settings_reset())).toBeDefined();
		expect(buttonText(m.settings_event_retention_shorten({ days: 7 }))).toBeUndefined();
	});
});
