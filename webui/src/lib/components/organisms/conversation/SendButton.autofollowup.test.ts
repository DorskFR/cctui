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

function render(over: { cacheCold: boolean; route: 'send' | 'launch'; autoFollowupTokens: number | null }) {
	comp = mount(SendButton, {
		target: document.body,
		props: {
			items: [],
			disabled: false,
			uploading: false,
			coldImminent: false,
			coldCountdownSecs: null,
			burstTokens: 93_000,
			onclick: vi.fn(),
			...over
		}
	});
	flushSync();
	const split = document.querySelector<HTMLElement>('[data-tsu="SplitButton"]')!;
	return { label: split.querySelector('.split-main button')?.textContent?.trim(), title: split.title };
}

describe('SendButton past the auto follow-up threshold', () => {
	it('reads Follow-up with the context in use', () => {
		const { label, title } = render({ cacheCold: false, route: 'send', autoFollowupTokens: 182_000 });
		expect(label).toBe('Follow-up · 182k');
		expect(title).toMatch(/context/i);
	});

	it('a cold launch keeps its own label: one follow-up, one label', () => {
		const { label } = render({ cacheCold: true, route: 'launch', autoFollowupTokens: 182_000 });
		expect(label).toMatch(/^Follow-up ❄️ ~93/);
	});

	it('a cold send in place that the server follows up reads the context label', () => {
		const { label } = render({ cacheCold: true, route: 'send', autoFollowupTokens: 182_000 });
		expect(label).toBe('Follow-up · 182k');
	});
});
