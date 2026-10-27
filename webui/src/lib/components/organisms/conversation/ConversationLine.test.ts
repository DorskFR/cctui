// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ConversationLine from './ConversationLine.svelte';
import lineSource from './ConversationLine.svelte?raw';
import bubbleSource from './MessageBubble.svelte?raw';
import type { Line } from './types';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

async function render(ln: Line, sessionId: string | null = null) {
	comp = mount(ConversationLine, {
		target: document.body,
		props: {
			ln,
			archived: false,
			sessionId,
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
		expect(bubbleSource).toMatch(/\.bubble\.peer\s*\{/);
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
		expect(bubbleSource).toMatch(/\.bubble\.user\.queued\s*\{/);
		expect(bubbleSource).toMatch(/\.bubble\.user\.cancelled\s*\{/);
		expect(bubbleSource).toMatch(/--role-queued/);
		expect(bubbleSource).not.toMatch(/\.bubble\.user\.queued\s*\{[^}]*--warn/);
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

describe('fork select checkbox', () => {
	const forkLine = line({ role: 'assistant', messageId: 'msg-1' });

	async function renderSelect(over: Record<string, unknown> = {}) {
		const ontoggleselect = vi.fn();
		comp = mount(ConversationLine, {
			target: document.body,
			props: {
				ln: forkLine,
				archived: false,
				onretry: vi.fn(),
				onedit: vi.fn(),
				onsaveimage: vi.fn(),
				oncopymarkdown: vi.fn(),
				forkable: true,
				selectMode: true,
				ontoggleselect,
				...over
			}
		});
		await flush();
		return {
			ontoggleselect,
			box: document.querySelector<HTMLInputElement>('.fork-check input[type="checkbox"]')
		};
	}

	it('renders a kit checkbox carrying the accessible name and the row title', async () => {
		const { box } = await renderSelect();
		expect(box).not.toBeNull();
		expect(box?.closest('[data-tsu="Checkbox"]')).not.toBeNull();
		expect(document.querySelector('.fork-check')?.getAttribute('title')).toBe(
			'Include this message in the fork selection'
		);
		expect(document.querySelector('.fork-check')?.textContent).toContain(
			'Select this message for fork'
		);
	});

	it('reflects the selected state and reports toggles', async () => {
		const { box, ontoggleselect } = await renderSelect({ selectedForFork: true });
		expect(box?.checked).toBe(true);
		box?.click();
		await flush();
		expect(ontoggleselect).toHaveBeenCalledWith('msg-1');
	});

	it('uses no raw checkbox input', () => {
		expect(lineSource).not.toContain('type="checkbox"');
	});

	it('stays hidden outside select mode', async () => {
		await renderSelect({ selectMode: false });
		expect(document.querySelector('.fork-check')).toBeNull();
	});
});

describe('gateway tool block', () => {
	it('renders a refused call as a danger notice, not an assistant bubble', async () => {
		const el = await render(
			line({ role: 'marker', html: undefined, text: 'x', toolBlock: { tool: 'Bash', term: 'a****e' } })
		);
		expect(el.classList.contains('blocked')).toBe(true);
		expect(el.classList.contains('assistant')).toBe(false);
		const notice = el.querySelector('.tool-block');
		expect(notice?.textContent).toContain('Bash');
		expect(notice?.textContent).toContain('a****e');
	});
});

describe('voice message', () => {
	it('badges a spoken turn VOICE and puts the mic before the transcript', async () => {
		const el = await render(line({ voice: true, html: '<p>what time is it</p>', text: 'what time is it' }));
		const msg = el.querySelector('[data-testid="voice-message"]');
		expect(msg).not.toBeNull();
		expect(msg?.querySelector('[role="img"]')?.textContent).toBe('🎙');
		expect(msg?.querySelector('.bubble.user')?.textContent).toBe('what time is it');
		const meta = el.querySelector('.lmeta')?.textContent?.toLowerCase() ?? '';
		expect(meta).toContain('voice');
		expect(meta).not.toContain('user');
		expect(el.textContent?.split('what time is it').length).toBe(2);
	});

	it('keeps a typed turn a user bubble with no mic', async () => {
		const el = await render(line());
		expect(el.querySelector('[data-testid="voice-message"]')).toBeNull();
		expect(el.querySelector('.bubble.user')?.textContent).toBe('hello');
		expect(el.textContent).not.toContain('🎙');
	});
});

describe('thinking-only turn', () => {
	it('footnotes thinking that ended a turn with no reply', async () => {
		const el = await render(line({ role: 'thinking', html: '<p>posted it</p>', noReply: true }));
		expect(el.querySelector('[role="note"]')?.textContent).toContain('no reply was sent');
	});

	it('leaves ordinary thinking without the footnote', async () => {
		const el = await render(line({ role: 'thinking', html: '<p>plan</p>' }));
		expect(el.querySelector('[role="note"]')).toBeNull();
	});
});

describe('images in CctuiAgent / CctuiSend results', () => {
	const result = (tool: string, text: string): Line => ({
		role: 'result',
		ts: 1_700_000_000_000,
		tool,
		text,
		htmlCode: text
	});
	const srcs = (el: HTMLElement) =>
		[...el.querySelectorAll('.result-images img')].map((i) => i.getAttribute('src'));

	it('renders bare and owner-qualified markers below the unchanged code block', async () => {
		const text =
			'done ![qa](cctui-img://11111111-2222-3333-4444-555555555555) and ' +
			'![child](cctui-img://ses_child/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee)';
		const el = await render(result('mcp__cctui__CctuiAgent', text), 'ses_parent');
		expect(el.querySelector('pre.code')?.textContent).toBe(text);
		expect(srcs(el)).toEqual([
			'/api/v1/sessions/ses_parent/images/11111111-2222-3333-4444-555555555555',
			'/api/v1/sessions/ses_child/images/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
		]);
		const pre = el.querySelector('pre.code') as HTMLElement;
		const imgs = el.querySelector('.result-images') as HTMLElement;
		expect(pre.compareDocumentPosition(imgs) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
	});

	it('covers CctuiSend too', async () => {
		const text = 'Images sent: ![s](cctui-img://ses_me/11111111-2222-3333-4444-555555555555)';
		const el = await render(result('mcp__cctui__CctuiSend', text), 'ses_me');
		expect(srcs(el)).toHaveLength(1);
	});

	it('adds nothing for a plain result or another tool', async () => {
		let el = await render(result('mcp__cctui__CctuiAgent', 'no images here'), 'ses_parent');
		expect(el.querySelector('.result-images')).toBeNull();
		unmount(comp!);
		comp = null;
		const marker = '![x](cctui-img://11111111-2222-3333-4444-555555555555)';
		el = await render(result('Bash', marker), 'ses_parent');
		expect(el.querySelector('.result-images')).toBeNull();
	});
});
