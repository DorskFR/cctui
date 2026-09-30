import { describe, expect, it } from 'vitest';
import { indexBySession } from './roomIndex.svelte';
import type { Room, RoomMember } from './rooms';

function member(id: string, role: RoomMember['role'] = 'member'): RoomMember {
	return {
		session_id: id,
		name: `lane ${id}`,
		adapter: 'codex',
		machine: 'box-a',
		state: 'live',
		role,
		last_delivered_seq: 0
	};
}

function room(id: string, members: RoomMember[], archived = false): Room {
	return { id, name: `room ${id}`, archived, members };
}

describe('indexBySession', () => {
	it('inverts rooms into a per-session lookup', () => {
		const idx = indexBySession([room('r1', [member('a'), member('b')])]);
		expect(idx.get('a')).toEqual([{ id: 'r1', name: 'room r1', role: 'member' }]);
		expect(idx.get('b')?.[0].id).toBe('r1');
		expect(idx.get('nobody')).toBe(undefined);
	});

	it('lists every room a session is in', () => {
		const idx = indexBySession([room('r1', [member('a')]), room('r2', [member('a')])]);
		expect(idx.get('a')?.map((r) => r.id)).toEqual(['r1', 'r2']);
	});

	it('carries the role through, so an observer badge is possible', () => {
		const idx = indexBySession([room('r1', [member('a', 'observer')])]);
		expect(idx.get('a')?.[0].role).toBe('observer');
	});

	it('skips archived rooms: the badge is for rooms that still fan out', () => {
		const idx = indexBySession([room('r1', [member('a')], true), room('r2', [member('a')])]);
		expect(idx.get('a')?.map((r) => r.id)).toEqual(['r2']);
	});

	it('is empty for no rooms and for rooms with no members', () => {
		expect(indexBySession([]).size).toBe(0);
		expect(indexBySession([room('r1', [])]).size).toBe(0);
	});
});
