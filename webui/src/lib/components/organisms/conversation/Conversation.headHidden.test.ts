// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import Host from './Conversation.host.test.svelte';
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

async function render(extra: Record<string, unknown>) {
	comp = mount(Host, {
		target: document.body,
		props: {
			props: {
				stream,
				scroll,
				sessionId: 's1',
				lines: [line(1, 'assistant', 'you are right, my list was stale')],
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

const rows = () => [...document.querySelectorAll('.older-row')];

describe('head-of-transcript row', () => {
	it('announces the start of the conversation when the filter hides the head', async () => {
		await render({
			canFetchOlder: false,
			headHidden: { count: 2, categories: ['system'] }
		});
		const text = rows()
			.map((r) => r.textContent ?? '')
			.join(' ');
		expect(rows().length).toBe(1);
		expect(text).toContain('hidden by filters');
		expect(text).toContain('2');
	});

	it('reveals the hidden categories in one click', async () => {
		const revealed: string[][] = [];
		await render({
			canFetchOlder: false,
			headHidden: { count: 2, categories: ['system', 'marker'] },
			onrevealhead: (categories: string[]) => revealed.push(categories)
		});
		rows()[0]?.querySelector('button')?.click();
		await flush();
		expect(revealed).toEqual([['system', 'marker']]);
	});

	it('stays hidden while older history can still be paged in', async () => {
		await render({
			canFetchOlder: true,
			headHidden: { count: 2, categories: ['system'] }
		});
		// The one row present is the existing "Load older" control.
		expect(rows().length).toBe(1);
		expect(rows()[0]?.textContent ?? '').not.toContain('hidden by filters');
	});

	it('stays hidden when nothing is hidden at the head', async () => {
		await render({ canFetchOlder: false, headHidden: { count: 0, categories: [] } });
		expect(rows().length).toBe(0);
	});
});
