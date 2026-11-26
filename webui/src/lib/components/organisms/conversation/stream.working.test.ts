// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { ws } from '#lib/ws.svelte.ts';
import { endpoints } from '#lib/queries/index.ts';
import { ConversationStream } from './stream.svelte';

const SID = 'working-seed';
let seq = 0;
const ev = (e: Record<string, unknown>): AgentEvent => ({ ts: ++seq, seq, ...e }) as AgentEvent;

let off: (() => void) | null = null;
afterEach(() => {
	off?.();
	off = null;
	vi.restoreAllMocks();
});

function open(buffered: AgentEvent[]) {
	vi.spyOn(ws, 'bufferedEvents').mockReturnValue(buffered);
	vi.spyOn(endpoints, 'userActions').mockResolvedValue({ items: [] } as never);
	const stream = new ConversationStream({
		id: () => SID,
		archived: () => false,
		historyData: () => [],
		pin: () => {},
		invalidateConversation: () => {},
		invalidateSessions: () => {},
		mergeIntoCache: () => {}
	});
	off = stream.subscribe(SID);
	return stream;
}

describe('stream.working on (re)subscribe', () => {
	it('reads a turn already in flight from the buffered events', () => {
		const s = open([ev({ type: 'text', content: 'on it', meta: false }), ev({ type: 'tool_call', tool: 'Bash', input: {}, tool_use_id: 't1' })]);
		expect(s.working).toBe(true);
	});

	it('stays idle when the buffer ends on turn_end', () => {
		const s = open([ev({ type: 'tool_call', tool: 'Bash', input: {}, tool_use_id: 't1' }), ev({ type: 'turn_end' })]);
		expect(s.working).toBe(false);
	});

	it('stays idle with nothing buffered', () => {
		expect(open([]).working).toBe(false);
	});
});
