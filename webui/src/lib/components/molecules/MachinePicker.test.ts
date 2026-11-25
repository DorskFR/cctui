// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { MachineRow } from '@bindings/MachineRow';
import MachinePicker from './MachinePicker.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const machine = { id: 'm1', name: 'dev1', display_name: null, hue: 120, liveness: 'online' } as unknown as MachineRow;

function render(variant?: 'pill' | 'field') {
	comp = mount(MachinePicker, {
		target: document.body,
		props: { value: 'm1', machines: [machine], label: 'Machine', ...(variant ? { variant } : {}) }
	});
	flushSync();
	const picker = document.body.querySelector<HTMLElement>('.picker');
	if (!picker) throw new Error('picker not rendered');
	return picker;
}

describe('MachinePicker', () => {
	it('tints the whole trigger by default', () => {
		const picker = render();
		expect(picker.classList.contains('field')).toBe(false);
		expect(picker.getAttribute('style')).toMatch(/--mh:\s*120/);
		expect(picker.querySelector('.pill')).toBeNull();
	});

	it('field variant keeps the outline neutral and tints only the inner pill', () => {
		const picker = render('field');
		expect(picker.classList.contains('field')).toBe(true);
		expect(picker.getAttribute('style') ?? '').not.toContain('--mh');
		const pill = picker.querySelector<HTMLElement>('.pill');
		expect(pill?.getAttribute('style')).toMatch(/--mh:\s*120/);
		expect(pill?.textContent).toContain('dev1');
		expect(picker.querySelector('select')?.getAttribute('aria-label')).toBe('Machine');
	});
});
