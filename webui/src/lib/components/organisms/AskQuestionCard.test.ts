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

describe('Cmd/Ctrl+Enter submit chord', () => {
	const chord = (target: Element, mod: 'metaKey' | 'ctrlKey' = 'metaKey') => {
		const e = new KeyboardEvent('keydown', { key: 'Enter', [mod]: true, bubbles: true, cancelable: true });
		target.dispatchEvent(e);
		return e;
	};
	const otherInput = (i = 0) =>
		document.querySelectorAll<HTMLInputElement>('input[aria-label="Other…"]')[i];
	const typeOther = async (i: number, text: string) => {
		const input = otherInput(i);
		input.value = text;
		input.dispatchEvent(new Event('input', { bubbles: true }));
		await flush();
	};

	it('submits from a focused option', async () => {
		const onsubmit = await render([single]);
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		radios[0].click();
		await flush();
		const e = chord(radios[0]);
		await flush();
		expect(e.defaultPrevented).toBe(true);
		expect(onsubmit).toHaveBeenCalledTimes(1);
		expect(onsubmit).toHaveBeenCalledWith(expect.stringContaining('Red'), [[0]]);
	});

	it('submits with Ctrl+Enter from a multi-select option', async () => {
		const onsubmit = await render([multi]);
		const opt = [...document.querySelectorAll('button')].find((b) => b.hasAttribute('aria-pressed'))!;
		opt.click();
		await flush();
		chord(opt, 'ctrlKey');
		await flush();
		expect(onsubmit).toHaveBeenCalledWith(expect.any(String), [[0]]);
	});

	it('submits from the Other… input', async () => {
		const onsubmit = await render([single]);
		await typeOther(0, 'Green');
		chord(otherInput(0));
		await flush();
		expect(onsubmit).toHaveBeenCalledWith(expect.stringContaining('Green'), null);
	});

	it('does nothing until every question is answered', async () => {
		const onsubmit = await render([single, multi]);
		const radios = document.querySelectorAll<HTMLInputElement>('input[type="radio"]');
		radios[1].click();
		await flush();
		const e = chord(radios[1]);
		await flush();
		chord(otherInput(1), 'ctrlKey');
		await flush();
		expect(e.defaultPrevented).toBe(false);
		expect(onsubmit).not.toHaveBeenCalled();
	});

	it('ignores plain Enter', async () => {
		const onsubmit = await render([single]);
		await typeOther(0, 'Green');
		otherInput(0).dispatchEvent(
			new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
		);
		await flush();
		expect(onsubmit).not.toHaveBeenCalled();
	});

	it('ignores the chord from outside the card', async () => {
		const onsubmit = await render([single]);
		document.querySelector<HTMLInputElement>('input[type="radio"]')!.click();
		await flush();
		const outside = document.createElement('textarea');
		document.body.appendChild(outside);
		chord(outside);
		await flush();
		expect(onsubmit).not.toHaveBeenCalled();
	});

	it('submits only once', async () => {
		const onsubmit = await render([single]);
		const radio = document.querySelector<HTMLInputElement>('input[type="radio"]')!;
		radio.click();
		await flush();
		chord(radio);
		await flush();
		chord(document.querySelector('.ask')!);
		await flush();
		expect(onsubmit).toHaveBeenCalledTimes(1);
	});
});
