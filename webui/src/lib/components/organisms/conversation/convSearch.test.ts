// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { ConversationHit } from '@bindings/ConversationHit';
import type { ConversationSearchResponse } from '@bindings/ConversationSearchResponse';
import { ConversationSearch, eventSearchText } from './convSearch.svelte';

function hit(seq: number): ConversationHit {
	return { seq, ts: seq * 1000, role: 'assistant', tool: null, snippet: `snippet ${seq}` };
}

function res(over: Partial<ConversationSearchResponse> = {}): ConversationSearchResponse {
	return {
		hits: [hit(10), hit(20), hit(30)],
		total: 3,
		truncated: false,
		tools: ['Bash'],
		...over
	};
}

function ctl(over: Partial<{ res: ConversationSearchResponse; fail: boolean }> = {}) {
	const visited: number[] = [];
	const errors: unknown[] = [];
	const fetchHits = vi.fn(async () => {
		if (over.fail) throw new Error('boom');
		return over.res ?? res();
	});
	const search = new ConversationSearch({
		id: () => 's1',
		fetchHits,
		ensureSeqVisible: async (seq) => {
			visited.push(seq);
			return true;
		},
		onerror: (e) => errors.push(e),
		debounceMs: 0
	});
	return { search, visited, errors, fetchHits };
}

const text = (content: string, seq: number): AgentEvent =>
	({ type: 'text', content, ts: seq * 1000, seq }) as unknown as AgentEvent;

describe('ConversationSearch', () => {
	it('loads the server hit list and highlights only the free-text part', async () => {
		const { search, fetchHits } = ctl();
		search.openBar('role:user needle');
		await search.run();

		expect(fetchHits).toHaveBeenCalledWith('s1', 'role:user needle');
		expect(search.count).toBe(3);
		expect(search.tools).toEqual(['Bash']);
		expect(search.terms).toEqual(['needle']);
		expect(search.index).toBe(-1);
	});

	it('steps forward and back over the server hits, wrapping at both ends', async () => {
		const { search, visited } = ctl();
		search.setQuery('needle');
		await search.run();

		await search.next();
		await search.next();
		expect(search.index).toBe(1);
		expect(search.currentSeq).toBe(20);

		await search.prev();
		await search.prev();
		expect(visited).toEqual([10, 20, 10, 30]);
		expect(search.index, 'prev from the first hit wraps to the last').toBe(2);

		await search.next();
		expect(search.index, 'next from the last hit wraps to the first').toBe(0);
	});

	it('steps nothing when there are no hits', async () => {
		const { search, visited } = ctl({ res: res({ hits: [], total: 0 }) });
		search.setQuery('nope');
		await search.run();
		await search.next();
		await search.prev();
		expect(visited).toEqual([]);
		expect(search.index).toBe(-1);
	});

	it('keeps the cursor on the same message when a refined query still matches it', async () => {
		const { search } = ctl();
		search.setQuery('needle');
		await search.run();
		await search.next();
		await search.next();
		expect(search.currentSeq).toBe(20);

		search.setQuery('needle role:user');
		await search.run();
		expect(search.currentSeq).toBe(20);
	});

	it('appends a live event that matches every free-text term', async () => {
		const { search } = ctl();
		search.setQuery('needle');
		await search.run();

		search.appendLive(text('a needle in here', 40));
		expect(search.count).toBe(4);
		expect(search.hits.at(-1)?.seq).toBe(40);

		search.appendLive(text('a needle in here', 40));
		expect(search.count, 'the same seq is never appended twice').toBe(4);

		search.appendLive(text('nothing relevant', 50));
		expect(search.count).toBe(4);
	});

	it('leaves a fielded query to the server rather than guessing locally', async () => {
		const { search, fetchHits } = ctl();
		search.rawQuery = 'needle role:user';
		await search.run();
		expect(fetchHits).toHaveBeenCalledTimes(1);

		search.appendLive(text('a needle in here', 40));
		expect(search.count, 'role: cannot be evaluated on the client').toBe(3);

		search.turnEnded();
		await vi.waitFor(() => expect(fetchHits).toHaveBeenCalledTimes(2));
	});

	it('debounces typing into one request', async () => {
		vi.useFakeTimers();
		try {
			const { search, fetchHits } = ctl();
			for (const raw of ['n', 'ne', 'nee', 'need', 'needle']) search.setQuery(raw);
			expect(fetchHits).not.toHaveBeenCalled();
			await vi.advanceTimersByTimeAsync(1);
			expect(fetchHits).toHaveBeenCalledTimes(1);
			expect(fetchHits).toHaveBeenCalledWith('s1', 'needle');
		} finally {
			vi.useRealTimers();
		}
	});

	it('reports a truncated list as more than it returned', async () => {
		const { search } = ctl({ res: res({ truncated: true }) });
		search.setQuery('needle');
		await search.run();
		expect(search.truncated).toBe(true);
	});

	it('clears on Escape once, then closes', async () => {
		const { search } = ctl();
		search.openBar('needle');
		await search.run();
		expect(search.escape()).toBe(true);
		expect(search.rawQuery).toBe('');
		expect(search.open).toBe(true);

		expect(search.escape()).toBe(true);
		expect(search.open).toBe(false);
		expect(search.escape(), 'a closed bar lets Escape close the drawer').toBe(false);
	});

	it('drops everything on a session switch', async () => {
		const { search } = ctl();
		search.openBar('needle');
		await search.run();
		search.reset();
		expect(search.open).toBe(false);
		expect(search.rawQuery).toBe('');
		expect(search.count).toBe(0);
		expect(search.index).toBe(-1);
	});

	it('surfaces a failed request instead of silently emptying the bar', async () => {
		const { search, errors } = ctl({ fail: true });
		search.setQuery('needle');
		await search.run();
		expect(errors).toHaveLength(1);
		expect(search.loading).toBe(false);
	});

	it('runs nothing for a blank query', async () => {
		const { search, fetchHits } = ctl();
		search.setQuery('   ');
		await search.run();
		expect(fetchHits).not.toHaveBeenCalled();
		expect(search.count).toBe(0);
	});
});

describe('eventSearchText', () => {
	it('mirrors the columns the server generates search_text from', () => {
		expect(eventSearchText(text('hello', 1))).toContain('hello');
		const call = {
			type: 'tool_call',
			tool: 'Bash',
			input: { command: 'rm -rf /tmp/x' },
			ts: 1,
			seq: 2
		} as unknown as AgentEvent;
		const t = eventSearchText(call);
		expect(t).toContain('Bash');
		expect(t).toContain('rm -rf /tmp/x');
	});
});
