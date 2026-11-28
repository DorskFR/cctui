// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { ws, type DeliverySnapshot } from '#lib/ws.svelte.ts';
import { endpoints } from '#lib/queries/index.ts';
import { ConversationStream } from './stream.svelte';

const SID = 'reconnect-sid';

const frame = (msg: Record<string, unknown>) =>
	(ws as unknown as { onFrame(raw: string): void }).onFrame(JSON.stringify({ session_id: SID, ...msg }));
const askFrame = (question: string) => frame({ type: 'ask_question', question });
const planFrame = (plan: string) => frame({ type: 'plan_request', plan });

const failed = (): DeliverySnapshot => ({
	pending: new Set(),
	retrying: new Map(),
	failed: new Map([[1, 'could not deliver']])
});

let stream: ConversationStream;
let stop: (() => void) | null = null;
let delivery: ((snap: DeliverySnapshot) => void) | null = null;

beforeEach(() => {
	vi.spyOn(endpoints, 'userActions').mockResolvedValue({ items: [] } as never);
	vi.spyOn(ws, 'onDelivery').mockImplementation((_sid, cb) => {
		delivery = cb;
		return () => {};
	});
	stream = new ConversationStream({
		id: () => SID,
		archived: () => false,
		historyData: () => [],
		pin: () => {},
		invalidateConversation: () => {},
		invalidateSessions: () => {},
		mergeIntoCache: () => {}
	});
	stop = $effect.root(() => {
		$effect(() => {
			void stream.resubTick;
			return stream.subscribe(SID);
		});
	});
	flushSync();
});

afterEach(() => {
	stop?.();
	stop = null;
	ws.clearAsk(SID);
	ws.clearPlan(SID);
	vi.restoreAllMocks();
});

describe('subscribe effect', () => {
	it('survives the first ask_question without unsubscribing or clearing the live buffer', () => {
		const unsubscribe = vi.spyOn(ws, 'unsubscribe');
		const clear = vi.spyOn(ws, 'clearStream');
		const ev = { type: 'text', content: 'streamed', meta: false, ts: 1, seq: 1 } as AgentEvent;
		frame({ type: 'stream', data: ev });
		flushSync();
		askFrame('Keep it?');
		flushSync();
		expect(stream.ask?.question).toBe('Keep it?');
		expect(unsubscribe).not.toHaveBeenCalled();
		expect(clear).not.toHaveBeenCalled();
		expect(stream.live).toEqual([ev]);
	});

	it('survives a plan prompt and a permission prompt', () => {
		const unsubscribe = vi.spyOn(ws, 'unsubscribe');
		planFrame('Do it');
		frame({
			type: 'permission_request',
			request_id: 'r1',
			tool_name: 'Bash',
			description: 'run',
			input_preview: 'ls',
			options: []
		});
		flushSync();
		expect(stream.plan?.plan).toBe('Do it');
		expect(unsubscribe).not.toHaveBeenCalled();
	});
});

describe('answering while the socket is down', () => {
	beforeEach(() => vi.spyOn(ws, 'trackedSend').mockReturnValue(false));

	it('locks and dismisses the question although the send was parked', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		expect(stream.ask).toBeNull();
		expect(stream.answering).toBe(true);
	});

	it('keeps the answered question hidden when the reconnect replays it', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		askFrame('Keep it?');
		expect(stream.ask).toBeNull();
		expect(stream.answering).toBe(true);
	});

	it('shows a different question the replay brings', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		askFrame('Something else?');
		expect(stream.ask?.question).toBe('Something else?');
	});

	it('restores the held question when the answer fails to deliver', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		askFrame('Keep it?');
		delivery?.(failed());
		expect(stream.ask?.question).toBe('Keep it?');
		expect(stream.answering).toBe(false);
	});

	it('forgets the held question once the daemon resolves it', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		askFrame('Keep it?');
		frame({ type: 'ask_resolved' });
		delivery?.(failed());
		expect(stream.ask).toBeNull();
	});

	it('drops the held question once the agent moves on', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		askFrame('Keep it?');
		frame({ type: 'stream', data: { type: 'text', content: 'next', meta: false, ts: 5, seq: 5 } });
		delivery?.(failed());
		expect(stream.ask).toBeNull();
	});

	it('locks and dismisses the plan, holding its replay', () => {
		planFrame('Do it');
		stream.answerPlan('1', [[0]]);
		expect(stream.plan).toBeNull();
		expect(stream.answering).toBe(true);
		planFrame('Do it');
		expect(stream.plan).toBeNull();
		delivery?.(failed());
		expect(stream.plan?.plan).toBe('Do it');
	});

	it('keeps the lock across a resubscribe of the same session', () => {
		askFrame('Keep it?');
		stream.answerQuestion('Yes', [[0]]);
		stream.resubTick++;
		flushSync();
		askFrame('Keep it?');
		expect(stream.ask).toBeNull();
		expect(stream.answering).toBe(true);
	});
});
