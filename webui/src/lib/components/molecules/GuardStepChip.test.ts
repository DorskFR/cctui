// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { GuardStepState } from '@bindings/GuardStepState';
import GuardStepChip from './GuardStepChip.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const step = (over: Partial<GuardStepState> = {}): GuardStepState => ({
	step: 2,
	title: 'Implement',
	allowed: 'all-read, Bash',
	disallowed: 'remote-write',
	transitions: [3, 5],
	exit: true,
	...over
});

const open = (props: Record<string, unknown>) => {
	comp = mount(GuardStepChip, { target: document.body, props: { step: step(), ...props } as never });
	flushSync();
	const root = document.querySelector<HTMLElement>('[data-journey="head-guard"]');
	if (!root) throw new Error('chip not rendered');
	return root;
};
const text = (el: Element) => el.textContent?.replace(/\s+/g, ' ').trim() ?? '';

describe('GuardStepChip', () => {
	it('shows the step with its title and the valid targets plus exit', () => {
		const root = open({ ontransition: () => {} });
		expect(text(root.querySelector('.label')!)).toBe('⛨ Step 2 · Implement');
		expect(root.getAttribute('title')).toContain('all-read, Bash');
		const options = [...root.querySelectorAll('option')].map((o) => o.value);
		expect(options).toEqual(['3', '5', 'exit']);
		expect(root.querySelector<HTMLSelectElement>('select')?.value).toBe('3');
	});

	it('asks for the picked target and nothing else', () => {
		const asked: string[] = [];
		const root = open({ ontransition: (s: string) => void asked.push(s) });
		const select = root.querySelector<HTMLSelectElement>('select')!;
		const pick = [...select.options].find((o) => o.value === 'exit')!;
		pick.selected = true;
		// The DOM shim cannot match `option:checked`, which Svelte's select binding reads.
		const nativeQuery = select.querySelector.bind(select);
		select.querySelector = ((q: string) =>
			q === ':checked' ? pick : nativeQuery(q)) as typeof select.querySelector;
		select.dispatchEvent(new Event('change', { bubbles: true }));
		flushSync();
		root.querySelector<HTMLButtonElement>('button')!.click();
		expect(asked).toEqual(['exit']);
	});

	it('is read-only without a handler, when disabled, and once exited', () => {
		expect(open({}).querySelector('select')).toBeNull();
		document.body.innerHTML = '';
		if (comp) unmount(comp);

		const busy = open({ disabled: true, ontransition: vi.fn() });
		expect(busy.querySelector<HTMLSelectElement>('select')?.disabled).toBe(true);
		expect(busy.querySelector<HTMLButtonElement>('button')?.disabled).toBe(true);
		document.body.innerHTML = '';
		if (comp) unmount(comp);

		const exited = open({ step: step({ step: -1, title: 'unknown', transitions: [] }), ontransition: vi.fn() });
		expect(text(exited.querySelector('.label')!)).toBe('⛨ Guard: exited');
		expect(exited.querySelector('select')).toBeNull();
	});

	it('names an untitled step by number alone', () => {
		const root = open({ step: step({ title: '' }) });
		expect(text(root.querySelector('.label')!)).toBe('⛨ Step 2');
	});
});
