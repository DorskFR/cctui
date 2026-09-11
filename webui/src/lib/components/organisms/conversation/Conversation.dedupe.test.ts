import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { AgentEvent } from '@bindings/AgentEvent';
import Host from './Conversation.host.test.svelte';
import { buildLines, type LineBuildCtx } from './lines';
import type { Line } from './types';

const flush = () => new Promise((r) => setTimeout(r, 0));

const stream = {
	answering: false,
	ask: null,
	plan: null,
	liveAskQuestions: null,
	perms: [],
	isDupeOfLiveAsk: () => false,
	retryFailed: () => {},
	answerQuestion: () => {},
	answerPlan: () => {}
};

const scroll = {
	scroller: null,
	stuck: true,
	gestures: 0,
	onScroll: () => {},
	markScrollGesture: () => {},
	markUserScroll: () => {},
	holdForPrepend: () => {},
	jumpToBottom: () => {}
};

function line(seq: number, role: string, text: string): Line {
	return { key: `k${seq}`, seq, role, text, html: `<p>${text}</p>`, ts: seq } as unknown as Line;
}

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

async function render(lines: Line[], extra: Record<string, unknown> = {}) {
	comp = mount(Host, {
		target: document.body,
		props: {
			props: {
			stream,
			scroll,
			sessionId: 's1',
			lines,
			isLoading: false,
			archived: false,
			askPreambleHtml: null,
			planPreambleHtml: null,
			onedit: () => {},
			onrespondperm: () => {},
			...extra
			}
		} as never
	});
	await flush();
}

describe('Conversation renders each line exactly once', () => {
	it('emits one .line node per line', async () => {
		await render([line(1, 'user', 'hello there'), line(2, 'assistant', 'general kenobi')]);
		expect(document.querySelectorAll('.line').length).toBe(2);
	});

	it('does not repeat a line body', async () => {
		await render([line(1, 'assistant', 'resumed with my partial findings')]);
		const hits = document.body.textContent?.split('resumed with my partial findings').length ?? 0;
		expect(hits - 1).toBe(1);
	});

	it('still renders once when pin and bookmark handlers are both supplied', async () => {
		await render([line(1, 'assistant', 'both handlers')], {
			onpin: () => {},
			pinnedSeqs: new Set([1]),
			onbookmark: () => {},
			isBookmarked: () => true
		});
		expect(document.querySelectorAll('.line').length).toBe(1);
	});
});

// The three encodings Claude really stores for ONE user turn that carried one
// image, taken verbatim (paths/names aside) from a transcript JSONL: the
// composer's submitted text, Claude's own copy, and the synthetic notice line.
const SID = '8b613c16-51c2-4896-859a-3f93c6fabf20';
const SHOT = 'Screenshot 2026-09-11 at 9.36.56.png';
const COMPOSER = `[${SHOT}]\nhave a look at this please\n\nAttached file:\n- /tmp/cctui-uploads/${SID}/${SHOT}`;
const CLAUDE_COPY = `[Image #1][${SHOT}]\nhave a look at this please`;
const SYNTH_NOTICE = `[Image: source: /tmp/cctui-uploads/${SID}/${SHOT}]`;

const userEvent = (content: string, ts: number, seq: number): AgentEvent => ({
	type: 'text',
	content: `▷ User: ${content}`,
	meta: false,
	kind: null,
	ts,
	message_id: null,
	usage: null,
	seq
});

const buildCtx: LineBuildCtx = {
	visible: () => true,
	renderMarkdown: (s) => `<p>${s}</p>`,
	renderCode: (t) => `<code>${t}</code>`,
	prettyJson: true,
	prettyDiff: true
};

describe('one user message with an image renders as one bubble (CCT-1008)', () => {
	const threeEncodings = [
		userEvent(COMPOSER, 1, 1),
		userEvent(CLAUDE_COPY, 2, 2),
		userEvent(SYNTH_NOTICE, 3, 3)
	];

	it('builds exactly one line from the three stored encodings', () => {
		const lines = buildLines(threeEncodings, buildCtx);
		expect(lines.length).toBe(1);
		expect(lines.map((l) => l.role)).toEqual(['user']);
	});

	it('renders exactly one .line node for the turn', async () => {
		await render(buildLines(threeEncodings, buildCtx));
		expect(document.querySelectorAll('.line').length).toBe(1);
	});

	it('leaks neither the upload path nor the [Image #N] marker', async () => {
		await render(buildLines(threeEncodings, buildCtx));
		const shown = document.body.textContent ?? '';
		expect(shown).toContain('have a look at this please');
		expect(shown).not.toContain('/tmp/cctui-uploads');
		expect(shown).not.toContain('[Image #1]');
		expect(shown).not.toContain('Attached file');
	});

	it('keeps the attachment on the surviving turn', () => {
		const ln = buildLines(threeEncodings, buildCtx)[0];
		expect(ln.uploads?.names).toEqual([SHOT]);
		expect(ln.uploads?.sessionId).toBe(SID);
	});

	it('renders one bubble for a two-image turn', () => {
		const b = 'Screenshot 2026-09-11 at 9.40.00.png';
		const events = [
			userEvent(
				`[${SHOT}] [${b}]\nboth of these\n\nAttached files (2):\n- /tmp/cctui-uploads/${SID}/${SHOT}\n- /tmp/cctui-uploads/${SID}/${b}`,
				1,
				1
			),
			userEvent(`[Image #1] [Image #2][${SHOT}] [${b}]\nboth of these`, 2, 2),
			userEvent(
				`[Image: source: /tmp/cctui-uploads/${SID}/${SHOT}]\n[Image: source: /tmp/cctui-uploads/${SID}/${b}]`,
				3,
				3
			)
		];
		const lines = buildLines(events, buildCtx);
		expect(lines.length).toBe(1);
		expect(lines[0].uploads?.names).toEqual([SHOT, b]);
	});

	it('renders one bubble for an image-only turn with no prose', () => {
		const events = [
			userEvent(`[${SHOT}]\n\nAttached file:\n- /tmp/cctui-uploads/${SID}/${SHOT}`, 1, 1),
			userEvent(`[Image #1][${SHOT}]`, 2, 2),
			userEvent(SYNTH_NOTICE, 3, 3)
		];
		const lines = buildLines(events, buildCtx);
		expect(lines.length).toBe(1);
		expect(lines[0].text).toBe('');
		expect(lines[0].uploads?.names).toEqual([SHOT]);
	});

	it('does not collapse two genuinely different user turns', () => {
		const events = [userEvent('first thing', 1, 1), userEvent('second thing', 2, 2)];
		expect(buildLines(events, buildCtx).length).toBe(2);
	});

	it('does not collapse two distinct image-only turns', () => {
		const b = 'other.png';
		const events = [
			userEvent(`[Image #1][${SHOT}]`, 1, 1),
			userEvent(`[Image #2][${b}]`, 2, 2)
		];
		const lines = buildLines(events, buildCtx);
		expect(lines.length).toBe(2);
		expect(lines.map((l) => l.uploads?.names)).toEqual([[SHOT], [b]]);
	});
});
