// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import SendButton from './SendButton.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(over: Record<string, unknown> = {}) {
	const onclick = vi.fn();
	comp = mount(SendButton, {
		target: document.body,
		props: {
			items: [],
			disabled: false,
			uploading: false,
			cacheCold: true,
			coldImminent: false,
			coldCountdownSecs: null,
			burstTokens: 93_000,
			onclick,
			...over
		}
	});
	flushSync();
	const split = document.querySelector<HTMLElement>('[data-tsu="SplitButton"]')!;
	const main = split.querySelector<HTMLButtonElement>('.split-main button')!;
	return { main, title: split.title, onclick };
}

describe('SendButton', () => {
	it('labels a cold Send that launches the follow-up, with a tooltip saying so', () => {
		const { main, title } = render({ route: 'launch' });
		expect(main.textContent).toMatch(/^\s*Follow-up ❄️ ~93/);
		expect(title).toMatch(/new session/i);
		expect(title).toMatch(/archive/i);
	});

	it('labels the follow-up under the modal route too', () => {
		const { main } = render({ route: 'followup' });
		expect(main.textContent).toMatch(/^\s*Follow-up ❄️/);
	});

	it('keeps Send while cold when Send sends in place', () => {
		const { main, title } = render({ route: 'send' });
		expect(main.textContent).toMatch(/^\s*Send ❄️ ~93/);
		expect(title).toMatch(/cold/i);
	});

	it('keeps a warm Send unchanged whatever the route', () => {
		const { main } = render({ cacheCold: false, route: 'launch' });
		expect(main.textContent?.trim()).toBe('Send');
	});

	it('shows progress and disables itself while the follow-up launches', () => {
		const { main } = render({ route: 'launch', launching: true });
		expect(main.textContent).not.toContain('❄️');
		expect(main.textContent).toMatch(/follow-up/i);
		expect(main.disabled).toBe(true);
	});
});

describe('SendButton emphasis', () => {
	const isPrimary = () =>
		document.querySelector('[data-tsu="SplitButton"]')?.classList.contains('split-primary') ?? false;

	it('is outlined while the field is empty and unfocused', () => {
		render({ cacheCold: false, active: false });
		expect(isPrimary()).toBe(false);
	});

	it('fills once the field has text or focus', () => {
		render({ cacheCold: false, active: true });
		expect(isPrimary()).toBe(true);
	});
});
