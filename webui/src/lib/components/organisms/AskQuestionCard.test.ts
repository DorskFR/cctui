// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import AskQuestionCard from './AskQuestionCard.svelte';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const single = {
	question: 'Pick a colour',
	options: [
		{ label: 'Red', description: 'warm', preview: 'red preview' },
		{ label: 'Blue', description: 'cool', preview: 'blue preview' }
	]
};
const multi = { question: 'Pick features', multiSelect: true, options: [{ label: 'A' }, { label: 'B' }] };

async function render(questions: unknown[], onsubmit = vi.fn()) {
	comp = mount(AskQuestionCard, {
		target: document.body,
		props: { questions, interactive: true, onsubmit } as never
	});
	await flush();
	return onsubmit;
}

describe('single-select options', () => {
	it('renders a radiogroup of native radios', async () => {
		await render([single]);
		expect(document.querySelector('[role="radiogroup"]')).not.toBeNull();
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		expect(radios.length).toBe(2);
		expect([...radios].every((r) => r.checked)).toBe(false);
	});

	it('announces the checked option and submits its index', async () => {
		const onsubmit = await render([single]);
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		radios[1].click();
		await flush();
		expect(radios[1].checked).toBe(true);
		expect(radios[0].checked).toBe(false);

		const send = [...document.querySelectorAll('button')].find((b) =>
			b.textContent?.includes('Send')
		);
		send?.click();
		await flush();
		expect(onsubmit).toHaveBeenCalledWith(expect.stringContaining('Blue'), [[1]]);
	});

	it('keeps one option selected at a time', async () => {
		await render([single]);
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		radios[0].click();
		await flush();
		radios[1].click();
		await flush();
		expect(radios[0].checked).toBe(false);
		expect(radios[1].checked).toBe(true);
	});

	it('exposes each option description to assistive tech', async () => {
		await render([single]);
		const first = document.querySelector<HTMLInputElement>('input[type="radio"]')!;
		const id = first.getAttribute('aria-describedby');
		expect(id).toBeTruthy();
		expect(document.getElementById(id!)?.textContent).toContain('warm');
	});

	it('follows keyboard focus with the preview pane', async () => {
		await render([single]);
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		radios[1].dispatchEvent(new FocusEvent('focus'));
		await flush();
		expect(document.querySelector('.preview')?.textContent).toContain('blue preview');
	});

	it('disables the options once the card is not interactive', async () => {
		comp = mount(AskQuestionCard, {
			target: document.body,
			props: { questions: [single], interactive: false, onsubmit: vi.fn() } as never
		});
		await flush();
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		expect([...radios].every((r) => r.disabled)).toBe(true);
	});
});

describe('multi-select options', () => {
	it('renders toggle options carrying their pressed state', async () => {
		await render([multi]);
		const opts = [...document.querySelectorAll('button')].filter((b) =>
			b.hasAttribute('aria-pressed')
		);
		expect(opts.length).toBe(2);
		expect(opts[0].getAttribute('aria-pressed')).toBe('false');
	});

	it('accumulates several picks', async () => {
		const onsubmit = await render([multi]);
		const opts = [...document.querySelectorAll('button')].filter((b) =>
			b.hasAttribute('aria-pressed')
		);
		opts[0].click();
		await flush();
		opts[1].click();
		await flush();
		expect(opts[0].getAttribute('aria-pressed')).toBe('true');
		expect(opts[1].getAttribute('aria-pressed')).toBe('true');

		const send = [...document.querySelectorAll('button')].find((b) =>
			b.textContent?.includes('Send')
		);
		send?.click();
		await flush();
		expect(onsubmit).toHaveBeenCalledWith(expect.any(String), [[0, 1]]);
	});
});

describe('two single-select questions with descriptions', () => {
	const form = [
		{
			header: 'Restore',
			question: 'Restore the backup?',
			options: [
				{ label: 'Yes', description: 'restore now' },
				{ label: 'No', description: 'keep current' }
			]
		},
		{
			header: 'Fix',
			question: 'How to fix?',
			options: [
				{ label: 'Patch', description: 'small change' },
				{ label: 'Rewrite', description: 'large change' },
				{ label: 'Skip', description: 'do nothing' }
			]
		}
	];

	it('stays rendered after the first click on any option', async () => {
		for (const target of [0, 1, 2, 3, 4]) {
			await render(form);
			const labels = document.querySelectorAll<HTMLLabelElement>('[role="radiogroup"] label');
			expect(labels.length).toBe(5);
			labels[target].click();
			await flush();
			expect(document.querySelector('.ask')).not.toBeNull();
			expect(document.querySelectorAll('[role="radiogroup"]').length).toBe(2);
			const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
			expect(radios[target].checked).toBe(true);
			unmount(comp!);
			comp = null;
			document.body.innerHTML = '';
		}
	});
});
