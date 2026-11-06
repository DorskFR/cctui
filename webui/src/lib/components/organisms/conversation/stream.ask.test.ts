// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { AgentEvent } from '@bindings/AgentEvent';
import { ws } from '$lib/ws.svelte';
import { endpoints } from '$lib/queries';
import Host from './Conversation.host.test.svelte';
import { ConversationStream } from './stream.svelte';
import { buildLines, type LineBuildCtx } from './lines';
import { parseAsk } from './format';

const SID = 'cct1841';

const askInput = (prefix: string, n: number) => ({
	questions: Array.from({ length: n }, (_, i) => ({
		header: `${prefix}${i}`,
		question: `${prefix}${i}: keep it?`,
		multiSelect: false,
		options: [{ label: 'Yes (Recommended)' }, { label: 'No' }]
	}))
});
const ASK_A = askInput('D7-', 4);
const ASK_B = askInput('D11-', 4);

let seq = 0;
const ev = (e: Record<string, unknown>): AgentEvent => ({ ts: ++seq, seq, ...e }) as AgentEvent;
const askCall = (input: unknown, id: string) =>
	ev({ type: 'tool_call', tool: 'AskUserQuestion', input, tool_use_id: id });
const askResult = (id: string) =>
	ev({ type: 'tool_result', tool: 'AskUserQuestion', output_summary: 'answered', tool_use_id: id, error: false });
const prose = (content: string) => ev({ type: 'text', content, meta: false });

const frame = (msg: Record<string, unknown>) =>
	(ws as unknown as { onFrame(raw: string): void }).onFrame(JSON.stringify({ session_id: SID, ...msg }));
const stream_ = (data: AgentEvent) => frame({ type: 'stream', data });
const liveAsk = (input: { questions: { question: string }[] }) =>
	frame({ type: 'ask_question', question: input.questions[0].question, questions: input.questions });

const ctx: LineBuildCtx = {
	visible: () => true,
	renderMarkdown: (s) => `<p>${s}</p>`,
	renderCode: (t) => `<code>${t}</code>`,
	prettyJson: true,
	prettyDiff: true
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

let stream: ConversationStream;
let off: () => void;
let comp: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	seq = 0;
	vi.spyOn(ws, 'trackedSend').mockReturnValue(true);
	vi.spyOn(endpoints, 'userActions').mockResolvedValue({ items: [] } as never);
	stream = new ConversationStream({
		id: () => SID,
		archived: () => false,
		historyData: () => [],
		pin: () => {},
		invalidateConversation: () => {},
		invalidateSessions: () => {},
		mergeIntoCache: () => {}
	});
	off = stream.subscribe(SID);
});

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	off();
	frame({ type: 'ask_resolved' });
	vi.restoreAllMocks();
});

async function render() {
	comp = mount(Host, {
		target: document.body,
		props: {
			props: {
				stream,
				scroll,
				sessionId: SID,
				lines: buildLines(stream.live, ctx),
				isLoading: false,
				archived: false,
				askPreambleHtml: null,
				planPreambleHtml: null,
				onedit: () => {},
				onrespondperm: () => {}
			}
		} as never
	});
	await new Promise((r) => setTimeout(r, 0));
}

const sendButtons = () =>
	[...document.querySelectorAll('button')].filter((b) => b.textContent?.trim() === 'Send answer');
const shown = () => document.body.textContent ?? '';

describe('a second AskUserQuestion after answering the first', () => {
	it('is answerable when the first was answered from its transcript card', async () => {
		stream_(askCall(ASK_A, 'a'));
		stream.answerQuestion('D7 answers', [[0], [0], [0], [0]], parseAsk(ASK_A));
		stream_(askResult('a'));
		stream_(prose('D7 changes. Next are D11 to D14.'));
		stream_(askCall(ASK_B, 'b'));

		expect(stream.answering).toBe(false);
		await render();
		expect(sendButtons()).toHaveLength(1);
		expect(shown()).toContain('D11-0: keep it?');
		expect(shown()).not.toContain('D7-0: keep it?');
	});

	it('is answerable when both asks arrive live', async () => {
		stream_(askCall(ASK_A, 'a'));
		liveAsk(ASK_A);
		stream.answerQuestion('D7 answers', [[0], [0], [0], [0]], stream.liveAskQuestions);
		frame({ type: 'ask_resolved' });
		stream_(askResult('a'));
		stream_(prose('D7 changes. Next are D11 to D14.'));
		stream_(askCall(ASK_B, 'b'));
		liveAsk(ASK_B);

		expect(stream.ask?.questions).toHaveLength(4);
		await render();
		expect(sendButtons()).toHaveLength(1);
		expect(shown().split('D11-0: keep it?').length - 1).toBe(1);
	});

	it('stays answerable when the live ask lands before its own transcript line', async () => {
		stream_(askCall(ASK_A, 'a'));
		liveAsk(ASK_A);
		stream.answerQuestion('D7 answers', [[0], [0], [0], [0]], stream.liveAskQuestions);
		frame({ type: 'ask_resolved' });
		stream_(askResult('a'));
		liveAsk(ASK_B);
		stream_(prose('D7 changes. Next are D11 to D14.'));
		stream_(askCall(ASK_B, 'b'));

		await render();
		expect(sendButtons()).toHaveLength(1);
		expect(shown().split('D11-0: keep it?').length - 1).toBe(1);
	});

	it('still drops a live ask once output streams past its own tool call', () => {
		liveAsk(ASK_B);
		stream_(prose('Next are D11 to D14.'));
		stream_(askCall(ASK_B, 'b'));
		expect(stream.ask).not.toBeNull();
		stream_(askResult('b'));
		expect(stream.ask).toBeNull();
		expect(stream.isDupeOfLiveAsk(parseAsk(ASK_B) ?? [])).toBe(true);
	});
});
