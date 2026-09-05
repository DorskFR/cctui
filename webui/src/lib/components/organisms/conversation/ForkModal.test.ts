import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ForkModal from './ForkModal.svelte';

let comp: ReturnType<typeof mount> | null = null;
function cleanup() {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
}
afterEach(cleanup);

function render(over: Partial<Record<string, unknown>> = {}) {
	const oncancel = vi.fn();
	const onsubmit = vi.fn();
	comp = mount(ForkModal, {
		target: document.body,
		props: {
			archived: false,
			isCodexSession: false,
			parentTokens: 12_000,
			models: [{ v: 'opus', label: 'Opus' }],
			efforts: ['', 'high'],
			forking: false,
			model: 'opus',
			effort: '',
			oncancel,
			onsubmit,
			...over
		}
	});
	return { oncancel, onsubmit };
}

describe('ForkModal', () => {
	it('is a kit Modal dialog, not a hand-rolled scrim + div', () => {
		render();
		expect(document.querySelector('dialog[data-tsu="Modal"]')).not.toBeNull();
		expect(document.querySelector('.fork-scrim, .fork-modal, [role="dialog"]:not(dialog)')).toBeNull();
		expect(document.querySelectorAll('.sheet-body select')).toHaveLength(2);
	});

	it('routes cancel and submit through the footer buttons', () => {
		const { oncancel, onsubmit } = render();
		const buttons = [...document.querySelectorAll('.sheet-foot button')] as HTMLButtonElement[];
		expect(buttons.map((b) => b.textContent?.trim())).toEqual(['Cancel', 'Fork']);
		buttons[1].click();
		expect(onsubmit).toHaveBeenCalledTimes(1);
		buttons[0].click();
		expect(oncancel).toHaveBeenCalledTimes(1);
	});

	it('titles the reopen and extract variants', () => {
		render({ archived: true });
		expect(document.querySelector('.sheet-title')?.textContent).toMatch(/reopen/i);
		cleanup();
		render({ extractLabel: '3 messages' });
		expect(document.body.textContent).toContain('3 messages');
	});
});
