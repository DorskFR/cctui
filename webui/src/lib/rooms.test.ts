import { describe, expect, it } from 'vitest';
import {
	canCreate,
	isDormant,
	matchByName,
	memberLabel,
	pickable,
	type Room,
	type RoomMember
} from './rooms';

function member(id: string, over: Partial<RoomMember> = {}): RoomMember {
	return {
		session_id: id,
		name: `lane ${id}`,
		adapter: 'claude-code',
		machine: 'box-a',
		state: 'live',
		last_delivered_seq: 0,
		...over
	};
}

function room(id: string, name: string, archived = false): Room {
	return { id, name, archived, members: [member('a')] };
}

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
	it('is true for anything that can no longer receive a broadcast', () => {
		expect(isDormant(member('a'))).toBe(false);
		expect(isDormant(member('a', { state: 'ended' }))).toBe(true);
		expect(isDormant(member('a', { state: 'archived' }))).toBe(true);
	});
});

describe('pickable', () => {
	it('offers only live rooms', () => {
		const rooms = [room('r1', 'wave 23'), room('r2', 'old', true)];
		expect(pickable(rooms).map((r) => r.id)).toEqual(['r1']);
	});
});

describe('matchByName', () => {
	it('matches case-insensitively and trims, mirroring the server index', () => {
		const rooms = [room('r1', 'Wave 23')];
		expect(matchByName(rooms, 'wave 23')?.id).toBe('r1');
		expect(matchByName(rooms, '  WAVE 23  ')?.id).toBe('r1');
		expect(matchByName(rooms, 'wave 24')).toBe(undefined);
	});

	it('matches an archived room too, so reusing its name revives it', () => {
		const rooms = [room('r1', 'old', true)];
		expect(matchByName(rooms, 'old')?.id).toBe('r1');
	});

	it('never matches an empty name', () => {
		expect(matchByName([room('r1', 'wave 23')], '   ')).toBe(undefined);
	});
});

describe('canCreate', () => {
	it('offers create only for a name that is not already a room', () => {
		const rooms = [room('r1', 'wave 23')];
		expect(canCreate(rooms, 'wave 24')).toBe(true);
		expect(canCreate(rooms, 'wave 23')).toBe(false);
		expect(canCreate(rooms, 'WAVE 23')).toBe(false);
		expect(canCreate(rooms, '  ')).toBe(false);
		expect(canCreate([], 'first')).toBe(true);
	});
});
