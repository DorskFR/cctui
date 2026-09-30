import { describe, expect, it } from 'vitest';
import {
	MAX_POST_BYTES,
	behindBy,
	canPost,
	isDormant,
	joinableRooms,
	memberLabel,
	mergeMessages,
	nextCursor,
	postProblem,
	type Room,
	type RoomMember,
	type RoomMessage
} from './rooms';

function member(id: string, over: Partial<RoomMember> = {}): RoomMember {
	return {
		session_id: id,
		name: `lane ${id}`,
		adapter: 'claude-code',
		machine: 'box-a',
		state: 'live',
		role: 'member',
		last_delivered_seq: 0,
		...over
	};
}

function room(over: Partial<Room> = {}): Room {
	return {
		id: 'r-1',
		name: 'wave 23',
		archived: false,
		members: [member('a'), member('b')],
		...over
	};
}

function msg(seq: number, body = `m${seq}`): RoomMessage {
	return {
		seq,
		sender_session_id: 'a',
		sender_label: 'lane a (codex on box-b)',
		body,
		created_at: '2026-09-30T10:00:00Z'
	};
}

describe('postProblem', () => {
	it('refuses an empty, oversized or archived post before the round-trip', () => {
		expect(postProblem(room(), 'hello')).toBe(null);
		expect(postProblem(room(), '   ')).toBe('empty');
		expect(postProblem(room(), 'x'.repeat(MAX_POST_BYTES + 1))).toBe('too-large');
		expect(postProblem({ archived: true }, 'hello')).toBe('archived');
	});

	it('measures the cap in bytes, not characters', () => {
		// Four bytes each, so a quarter of the cap in characters is exactly at it.
		const emoji = '🙂'.repeat(MAX_POST_BYTES / 4);
		expect(postProblem(room(), emoji)).toBe(null);
		expect(postProblem(room(), `${emoji}🙂`)).toBe('too-large');
	});

	it('reports archived before empty, so the reason is the durable one', () => {
		expect(postProblem({ archived: true }, '  ')).toBe('archived');
	});
});

describe('memberLabel', () => {
	it('falls back to the session id and to unknown parts', () => {
		expect(memberLabel(member('a'))).toBe('lane a (claude-code on box-a)');
		expect(memberLabel(member('a', { name: '  ' }))).toBe('a (claude-code on box-a)');
		expect(memberLabel(member('a', { name: null, adapter: null, machine: null }))).toBe(
			'a (unknown on unknown machine)'
		);
	});
});

describe('isDormant', () => {
	it('is true for anything that can no longer take a turn', () => {
		expect(isDormant(member('a'))).toBe(false);
		expect(isDormant(member('a', { state: 'ended' }))).toBe(true);
		expect(isDormant(member('a', { state: 'archived' }))).toBe(true);
	});
});

describe('mergeMessages', () => {
	it('de-duplicates by seq so the WS event and the poll do not double a post', () => {
		const have = [msg(1), msg(2)];
		expect(mergeMessages(have, [msg(2), msg(3)]).map((m) => m.seq)).toEqual([1, 2, 3]);
		expect(mergeMessages(have, [])).toBe(have);
	});

	it('sorts by seq whatever order the pages arrived in', () => {
		expect(mergeMessages([msg(3)], [msg(1), msg(2)]).map((m) => m.seq)).toEqual([1, 2, 3]);
	});

	it('lets a later copy of the same seq win, so an edited body is not stale', () => {
		const merged = mergeMessages([msg(1, 'old')], [msg(1, 'new')]);
		expect(merged).toHaveLength(1);
		expect(merged[0].body).toBe('new');
	});
});

describe('nextCursor', () => {
	it('is the newest seq held, or undefined for a fresh panel', () => {
		expect(nextCursor([msg(1), msg(7)])).toBe(7);
		expect(nextCursor([])).toBe(undefined);
	});
});

describe('behindBy', () => {
	it('counts what a member has not been handed yet and never goes negative', () => {
		expect(behindBy(5, member('a', { last_delivered_seq: 2 }))).toBe(3);
		expect(behindBy(5, member('a', { last_delivered_seq: 5 }))).toBe(0);
		expect(behindBy(2, member('a', { last_delivered_seq: 9 }))).toBe(0);
	});
});

describe('canPost', () => {
	it('allows a member of a live room and refuses observers, strangers and archives', () => {
		const r = room({ members: [member('a'), member('b', { role: 'observer' })] });
		expect(canPost(r, 'a')).toBe(true);
		expect(canPost(r, 'b')).toBe(false);
		expect(canPost(r, 'stranger')).toBe(false);
		expect(canPost({ ...r, archived: true }, 'a')).toBe(false);
	});
});

describe('joinableRooms', () => {
	it('offers only live rooms the session is not already in', () => {
		const rooms = [
			room({ id: 'r-1', members: [member('a')] }),
			room({ id: 'r-2', members: [member('b')] }),
			room({ id: 'r-3', archived: true, members: [] })
		];
		expect(joinableRooms(rooms, 'a').map((r) => r.id)).toEqual(['r-2']);
		expect(joinableRooms(rooms, 'z').map((r) => r.id)).toEqual(['r-1', 'r-2']);
	});
});
