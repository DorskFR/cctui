// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { mergeDefaults, settings, type SettingsState } from '#lib/settings.svelte.ts';
import { DEFAULT_AUTO_FOLLOWUP } from '#lib/autoFollowup.ts';
import AutoFollowupGroup from './AutoFollowupGroup.svelte';

let component: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (component) unmount(component);
	component = null;
	settings.setAutoFollowup(DEFAULT_AUTO_FOLLOWUP);
	document.body.innerHTML = '';
});

function render() {
	component = mount(AutoFollowupGroup, { target: document.body });
	flushSync();
}

const toggle = () => document.querySelector<HTMLButtonElement>('[role="switch"]');
const amount = () =>
	document.querySelector<HTMLInputElement>('[data-testid="autofollowup-threshold"] input');
const unit = () =>
	document.querySelector<HTMLSelectElement>('[data-testid="autofollowup-threshold"] select');
const hint = () => document.querySelector('[data-testid="autofollowup-percent-hint"]');

describe('auto follow-up settings', () => {
	it('defaults to off at 150k tokens', () => {
		expect(mergeDefaults(null).autoFollowup).toEqual({
			enabled: false,
			threshold: { unit: 'tokens', value: 150_000 }
		});
		const malformed = { autoFollowup: { enabled: 'yes', threshold: { unit: 'percent', value: 400 } } };
		expect(mergeDefaults(malformed as unknown as Partial<SettingsState>).autoFollowup).toEqual(
			DEFAULT_AUTO_FOLLOWUP
		);
		render();
		expect(toggle()?.getAttribute('aria-checked')).toBe('false');
		expect(amount()?.value).toBe('150000');
		expect(unit()?.value).toBe('tokens');
		expect(hint()).toBeNull();
		toggle()?.click();
		flushSync();
		expect(settings.autoFollowup.enabled).toBe(true);
	});

	it('a percent threshold explains the unknown-window case', () => {
		render();
		const select = unit()!;
		select.value = 'percent';
		select.dispatchEvent(new Event('change', { bubbles: true }));
		flushSync();
		expect(settings.autoFollowup.threshold).toEqual({ unit: 'percent', value: 80 });
		expect(hint()?.textContent).toContain('context window is known');
		const input = amount()!;
		input.value = '150';
		input.dispatchEvent(new Event('change', { bubbles: true }));
		flushSync();
		expect(settings.autoFollowup.threshold).toEqual({ unit: 'percent', value: 80 });
	});
});
