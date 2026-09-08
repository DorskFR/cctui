import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ForkModal from './ForkModal.svelte';
import Host from './ForkModal.host.test.svelte';
import { OTHER_MODEL } from '$lib/harnessModels';

const flush = () => new Promise((r) => setTimeout(r, 0));

async function renderHost(model: string, models = [{ v: 'opus', label: 'Opus' }]) {
	const seen: string[] = [];
	comp = mount(Host, {
		target: document.body,
		props: { models, model, onmodel: (v: string) => seen.push(v) }
	});
	await flush();
	const select = document.querySelector('#fork-model') as HTMLSelectElement;
	return { seen, select };
}

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

	it('offers the free-text "Other model" entry on the model field', async () => {
		const { select } = await renderHost('opus');
		const other = [...select.options].find((o) => o.value === OTHER_MODEL);
		expect(other?.textContent).toMatch(/other/i);
	});

	it('keeps a free-text / remembered model id selected and round-tripping', async () => {
		const { seen, select } = await renderHost('claude-opus-5[1m]');
		expect(select.value).toBe('claude-opus-5[1m]');
		expect([...select.options].map((o) => o.value)).toContain('claude-opus-5[1m]');
		expect(seen.at(-1)).toBe('claude-opus-5[1m]');
	});

	it('titles the reopen and extract variants', () => {
		render({ archived: true });
		expect(document.querySelector('.sheet-title')?.textContent).toMatch(/reopen/i);
		cleanup();
		render({ extractLabel: '3 messages' });
		expect(document.body.textContent).toContain('3 messages');
	});
});
