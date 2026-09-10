import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import Conversation from './Conversation.svelte';
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
	comp = mount(Conversation, {
		target: document.body,
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
