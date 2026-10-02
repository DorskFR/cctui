import { describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { loadWholeConversation, type ConversationPageFetch } from './wholeConversation';

const ev = (seq: number): AgentEvent => ({
	type: 'text',
	content: `m${seq}`,
	meta: false,
	ts: seq,
	seq
});

const range = (from: number, to: number) =>
	Array.from({ length: to - from + 1 }, (_, i) => ev(from + i));

// Serves `total` events newest-first windows, the way the conversation route does.
const server = (total: number): ConversationPageFetch =>
	vi.fn(async (_id: string, { limit, before }: { limit: number; before: number }) => {
		const below = range(1, Math.min(before - 1, total));
		return below.slice(Math.max(0, below.length - limit));
	});

describe('loadWholeConversation', () => {
	it('walks every earlier page back to the first event of a multi-page session', async () => {
		const fetchPage = server(1_260);
		const loaded = range(1_201, 1_260);
		const progress: number[] = [];
		const all = await loadWholeConversation('s1', loaded, fetchPage, (n) => progress.push(n));
		expect(all.map((e) => e.seq)).toEqual(range(1, 1_260).map((e) => e.seq));
		expect(fetchPage).toHaveBeenCalledTimes(3);
		expect(progress).toEqual([560, 1_060]);
	});

	it('asks once and keeps the loaded events when they already reach the start', async () => {
		const fetchPage = server(40);
		const loaded = range(1, 40);
		expect(await loadWholeConversation('s1', loaded, fetchPage)).toBe(loaded);
		expect(fetchPage).toHaveBeenCalledTimes(1);
	});

	it('drops an event that both the fetched pages and the loaded ones hold', async () => {
		const fetchPage: ConversationPageFetch = async () => [ev(1), ev(2), ev(3)];
		const all = await loadWholeConversation('s1', [ev(3), ev(4)], fetchPage);
		expect(all.map((e) => e.seq)).toEqual([1, 2, 3, 4]);
	});

	it('fetches nothing when no loaded event carries a seq', async () => {
		const fetchPage = vi.fn();
		const loaded = [{ ...ev(1), seq: null }];
		expect(await loadWholeConversation('s1', loaded, fetchPage)).toBe(loaded);
		expect(fetchPage).not.toHaveBeenCalled();
	});
});
