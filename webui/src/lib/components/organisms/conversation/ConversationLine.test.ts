// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ConversationLine from './ConversationLine.svelte';
import lineSource from './ConversationLine.svelte?raw';
import type { Line } from './types';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

async function render(ln: Line) {
	comp = mount(ConversationLine, {
		target: document.body,
		props: {
			ln,
			archived: false,
			onretry: vi.fn(),
			onedit: vi.fn(),
			onsaveimage: vi.fn(),
			oncopymarkdown: vi.fn()
		}
	});
	await flush();
	return document.querySelector('.line') as HTMLElement;
}

const line = (over: Partial<Line> = {}): Line => ({
	role: 'user',
	ts: 1_700_000_000_000,
	html: '<p>hello</p>',
	text: 'hello',
	...over
});

describe('role badge and tint', () => {
	it('badges a user line USER', async () => {
		const el = await render(line());
		expect(el.classList.contains('user')).toBe(true);
		expect(el.textContent).toContain('user');
	});

	it('badges a peer line PEER, not USER', async () => {
		const el = await render(line({ role: 'peer', peerFrom: 'cctui orchestrator skill' }));
		expect(el.classList.contains('peer')).toBe(true);
		expect(el.classList.contains('user')).toBe(false);
		expect(el.getAttribute('data-journey-key')).toBe('peer');
		expect(el.textContent).toContain('peer');
	});

	// jsdom does not apply Svelte's scoped <style>, so the tint is asserted on
	// the rules themselves: a new role with no --bc of its own would silently
	// inherit the muted default and be indistinguishable from its neighbours.
	it('gives peer its own --bc and bubble tint, distinct from user and system', () => {
		const css = lineSource;
		expect(css).toMatch(/\.line\.peer\s*\{\s*--bc:\s*var\(--role-peer\);/);
		expect(css).toMatch(/\.line\.peer\s+\.bubble\s*\{/);
		expect(css).not.toMatch(/\.line\.peer\s*\{\s*--bc:\s*var\(--role-(user|system)\)/);
	});

	it('shows the peer sender name in the meta row', async () => {
		const el = await render(line({ role: 'peer', peerFrom: 'cctui orchestrator skill' }));
		const from = el.querySelector('.peer-from') as HTMLElement;
		expect(from.textContent).toContain('cctui orchestrator skill');
		expect(from.title).toBe('cctui orchestrator skill');
	});

	it('omits the sender row when the peer gave no name', async () => {
		const el = await render(line({ role: 'peer' }));
		expect(el.querySelector('.peer-from')).toBeNull();
	});
});

describe('queue state on the message itself', () => {
	it('tints a waiting queued bubble and labels it', async () => {
		const el = await render(line({ queued: true }));
		expect(el.classList.contains('queued')).toBe(true);
		expect(el.classList.contains('cancelled')).toBe(false);
		expect(el.textContent).toContain('queued');
	});

	it('shows neither tint nor chip once delivered', async () => {
		const el = await render(line({ queued: true, queuedAt: 1_699_999_000_000 }));
		expect(el.classList.contains('queued')).toBe(false);
		expect(el.querySelector('.meta-end')).toBeNull();
		expect(el.textContent).not.toContain('queued');
	});

	it('strikes through a prompt removed from the queue', async () => {
		const el = await render(line({ queued: true, cancelled: true }));
		expect(el.classList.contains('cancelled')).toBe(true);
		expect(el.textContent).toContain('removed from queue');
	});

	it('gives queued and cancelled bubbles their own rules, distinct from pending', () => {
		expect(lineSource).toMatch(/\.line\.user\.queued\s+\.bubble\s*\{/);
		expect(lineSource).toMatch(/\.line\.user\.cancelled\s+\.bubble\s*\{/);
		expect(lineSource).toMatch(/--role-queued/);
		expect(lineSource).not.toMatch(/\.line\.user\.queued\s+\.bubble\s*\{[^}]*--warn/);
	});

	it('leaves an ordinary user line with no queue class or label', async () => {
		const el = await render(line());
		expect(el.classList.contains('queued')).toBe(false);
		expect(el.querySelector('.meta-end')).toBeNull();
	});
});

describe('quote reply action', () => {
	const onquote = vi.fn();

	async function renderQuotable(ln: Line, over: Record<string, unknown> = {}) {
		onquote.mockClear();
		comp = mount(ConversationLine, {
			target: document.body,
			props: {
				ln,
				archived: false,
				onretry: vi.fn(),
				onedit: vi.fn(),
				onsaveimage: vi.fn(),
				oncopymarkdown: vi.fn(),
				onquote,
				...over
			}
		});
		await flush();
		return document.querySelector('.quote-btn') as HTMLButtonElement | null;
	}

	it('shows on a user line and reports the whole line when nothing is selected', async () => {
		const btn = await renderQuotable(line());
		expect(btn).not.toBeNull();
		btn?.click();
		await flush();
		expect(onquote).toHaveBeenCalledTimes(1);
		expect(onquote.mock.calls[0][0].text).toBe('hello');
		expect(onquote.mock.calls[0][1]).toBeNull();
	});

	it('shows on assistant, peer, tool and result lines', async () => {
		for (const role of ['assistant', 'peer', 'tool', 'result'] as const) {
			const btn = await renderQuotable(line({ role }));
			expect(btn, role).not.toBeNull();
			if (comp) unmount(comp);
			comp = null;
			document.body.innerHTML = '';
		}
	});

	it('hides on thinking and marker lines', async () => {
		expect(await renderQuotable(line({ role: 'thinking', html: '<p>hmm</p>' }))).toBeNull();
		if (comp) unmount(comp);
		comp = null;
		document.body.innerHTML = '';
		expect(await renderQuotable(line({ role: 'marker', markerTexts: ['mode'] }))).toBeNull();
	});

	it('hides on an archived session and in fork select mode', async () => {
		expect(await renderQuotable(line(), { archived: true })).toBeNull();
		if (comp) unmount(comp);
		comp = null;
		document.body.innerHTML = '';
		expect(await renderQuotable(line(), { selectMode: true })).toBeNull();
	});

	it('hides when no onquote handler is given', async () => {
		const el = await render(line());
		expect(el.querySelector('.quote-btn')).toBeNull();
	});

	it('passes a selection contained in the bubble', async () => {
		const btn = await renderQuotable(line({ html: '<p>hello world</p>' }));
		const text = document.querySelector('.bubble p')?.firstChild as Text;
		const range = document.createRange();
		range.setStart(text, 0);
		range.setEnd(text, 5);
		const sel = window.getSelection();
		sel?.removeAllRanges();
		sel?.addRange(range);
		btn?.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
		btn?.click();
		await flush();
		expect(onquote.mock.calls[0][1]).toBe('hello');
		sel?.removeAllRanges();
	});
});
