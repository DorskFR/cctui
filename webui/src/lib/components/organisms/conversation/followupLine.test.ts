import { describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { ThreadConversationPage } from '@bindings/ThreadConversationPage';
import { FollowupLine, type FollowupLineDeps } from './followupLine.svelte';

const row = (id: string, parent: string | null, next: string | null = null) =>
	({
		id,
		parent_id: parent,
		next_session_id: next,
		metadata: parent
			? { relation: 'followup', followup_reason: 'cold', followup_instruction: `into ${id}` }
			: {},
		registered_at: '2026-11-20T10:00:00Z',
		status: parent ? 'active' : 'archived',
		working_dir: '/w'
	}) as unknown as SessionListItem;

const generations = ['A', 'B', 'C'].map((session_id, i) => ({
	session_id,
	generation: i + 1,
	readable: true,
	registered_at: '2026-11-20T10:00:00Z'
}));
const say = (content: string, seq: number): AgentEvent => ({
	type: 'text',
	content,
	meta: false,
	ts: 1000 + seq,
	seq
});
const page = (over: Partial<ThreadConversationPage>): ThreadConversationPage => ({
	thread_root_id: 'A',
	head: 'C',
	generations,
	entries: [],
	...over
});

function deps(): FollowupLineDeps & { calls: { id: string; cursor?: string; limit?: number }[] } {
	const calls: { id: string; cursor?: string; limit?: number }[] = [];
	const rows = { A: row('A', null, 'B'), B: row('B', 'A', 'C'), C: row('C', 'B') };
	return {
		calls,
		tree: vi.fn(async () => ({
			sessions: [
				{ id: 'A', working_dir: '/w', registered_at: 'x', parent_id: null },
				{ id: 'B', working_dir: '/w', registered_at: 'x', parent_id: 'A' },
				{ id: 'C', working_dir: '/w', registered_at: 'x', parent_id: 'B' }
			]
		})),
		session: vi.fn(async (id: string) => rows[id as keyof typeof rows]),
		thread: vi.fn(async (id: string, opts?: { limit?: number; cursor?: string }) => {
			calls.push({ id, ...opts });
			if (opts?.limit === 1) return page({});
			return page({
				entries: [
					{ entry: 'event', session_id: 'A', generation: 1, event: say('in A', 1) },
					{ entry: 'followup_marker', generation: 2, from: 'A', to: 'B', at: 'x', instruction: 'into B' },
					{ entry: 'event', session_id: 'B', generation: 2, event: say('in B', 1) },
					{ entry: 'followup_marker', generation: 3, from: 'B', to: 'C', at: 'x', instruction: 'into C' }
				],
				next_cursor: null
			});
		})
	};
}

describe('FollowupLine', () => {
	it('opens a deep link on the head and pages everything before it', async () => {
		const d = deps();
		const line = new FollowupLine(d);
		await line.open(row('B', 'A', 'C'));
		expect(line.headId).toBe('C');
		expect(line.head?.id).toBe('C');
		expect(line.headGen).toBe(3);
		expect(line.target).toBe('B');
		expect(line.rootId).toBe('A');
		expect(d.calls.at(-1)).toEqual({ id: 'C', cursor: '3:0' });
		expect(line.entries.every((e) => e.generation < 3)).toBe(true);
		expect(line.headMark).toMatchObject({ from: 'B', to: 'C', instruction: 'into C' });
		expect(line.segments.map((s) => s.sessionId)).toEqual(['A', 'B']);
		expect([...line.members].sort()).toEqual(['A', 'B', 'C']);
	});

	it('leaves a session outside any line alone', async () => {
		const d = deps();
		const line = new FollowupLine(d);
		await line.open(row('X', null));
		expect(line.active).toBe(false);
		expect(d.tree).not.toHaveBeenCalled();
	});

	it('swaps the head in place and keeps the old head as rendered', async () => {
		const d = deps();
		const line = new FollowupLine(d);
		const a = row('A', null);
		await line.open(a);
		const events = [say('hello', 1), say('world', 2)];
		const moved = line.supersede(
			{
				followup_id: 'f1',
				from: 'A',
				to: 'B',
				reason: 'cold',
				instruction: 'go on',
				archived: true,
				archive_error: null,
				at: '2026-11-20T11:00:00Z'
			},
			{ session: a, events, complete: true }
		);
		expect(moved).toBe(true);
		expect(line.headId).toBe('B');
		expect(line.head?.id).toBe('B');
		expect(line.head?.status).toBe('active');
		expect(line.headGen).toBe(2);
		expect(line.headMark).toMatchObject({ from: 'A', to: 'B', instruction: 'go on', generation: 2 });
		expect(line.segments).toHaveLength(1);
		expect(line.segments[0].events).toEqual(events);
		expect(line.members.has('B')).toBe(true);
		expect(line.cursor).toBeNull();
		expect(line.fresh).toBe(true);
	});

	it('pages the rest of an old head that was not fully loaded', async () => {
		const line = new FollowupLine(deps());
		const a = row('A', null);
		await line.open(a);
		line.supersede(
			{
				followup_id: 'f1',
				from: 'A',
				to: 'B',
				reason: 'manual',
				instruction: 'go on',
				archived: true,
				archive_error: null,
				at: 'x'
			},
			{ session: a, events: [say('late', 40), say('later', 41)], complete: false }
		);
		expect(line.cursor).toBe('1:40');
	});

	it('ignores a swap of another session and a repeated one', async () => {
		const line = new FollowupLine(deps());
		const a = row('A', null);
		await line.open(a);
		const ev = {
			followup_id: 'f1',
			from: 'Z',
			to: 'B',
			reason: 'cold' as const,
			instruction: '',
			archived: true,
			archive_error: null,
			at: 'x'
		};
		expect(line.supersede(ev, { session: a, events: [], complete: true })).toBe(false);
		expect(line.supersede({ ...ev, from: 'A' }, { session: a, events: [], complete: true })).toBe(true);
		expect(line.supersede({ ...ev, from: 'B' }, { session: a, events: [], complete: true })).toBe(false);
	});

	it('shows a pending marker until the follow-up fails', async () => {
		const line = new FollowupLine(deps());
		await line.open(row('A', null));
		line.begin({ followup_id: 'f1', from: 'A', to: null, state: 'pending', error: null }, 'go on', 'cold');
		expect(line.pending).toMatchObject({ pending: true, instruction: 'go on', generation: 2 });
		expect(
			line.fail({ followup_id: 'other', from: 'A', reason: 'cold', instruction: 'go on', error: 'x' })
		).toBe(false);
		expect(
			line.fail({ followup_id: 'f1', from: 'A', reason: 'cold', instruction: 'go on', error: 'x' })
		).toBe(true);
		expect(line.pending).toBeNull();
	});
});
