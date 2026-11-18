import { describe, expect, it, vi } from 'vitest';
import type { HiddenOwner } from '@bindings/HiddenOwner';
import type { MachineRow } from '@bindings/MachineRow';
import type { UserRow } from '@bindings/UserRow';
import { setShown, visibilityRows } from './sessionVisibility.logic';

const user = (id: string, name: string, over: Partial<UserRow> = {}): UserRow => ({
	id,
	name,
	created_at: '',
	revoked_at: null,
	disabled_at: null,
	can_dispatch: true,
	last_seen_at: null,
	...over
});

const machine = (id: string, userId: string, name: string, over: Partial<MachineRow> = {}): MachineRow =>
	({
		id,
		user_id: userId,
		name,
		display_name: null,
		first_seen_at: '',
		last_seen_at: '',
		revoked_at: null,
		kind: 'daemon',
		hue: null,
		key_preview: '',
		liveness: 'online',
		...over
	}) as MachineRow;

describe('visibilityRows', () => {
	const users = [user('u-me', 'me'), user('u-bob', 'bob'), user('u-old', 'old', { revoked_at: 'x' })];
	const machines = [
		machine('m-2', 'u-bob', 'zeta'),
		machine('m-1', 'u-bob', 'alpha', { display_name: 'Alpha box' }),
		machine('m-gone', 'u-bob', 'gone', { revoked_at: 'x' }),
		machine('m-me', 'u-me', 'mine')
	];

	it('defaults every switch ON and skips revoked users and machines', () => {
		const rows = visibilityRows(users, machines, [], 'u-me');
		expect(rows.map((r) => r.id)).toEqual(['u-bob', 'u-me']);
		const bob = rows[0];
		expect(bob.shown).toBe(true);
		expect(bob.machines).toEqual([
			{ id: 'm-1', name: 'Alpha box', shown: true },
			{ id: 'm-2', name: 'zeta', shown: true }
		]);
	});

	it('reads OFF from the admin hidden list, per user and per machine', () => {
		const owners: HiddenOwner[] = [
			{ kind: 'user', target_id: 'u-bob', label: 'bob' },
			{ kind: 'machine', target_id: 'm-2', label: 'zeta' }
		];
		const [bob] = visibilityRows(users, machines, owners, 'u-me');
		expect(bob.shown).toBe(false);
		expect(bob.machines.find((mc) => mc.id === 'm-2')?.shown).toBe(false);
		expect(bob.machines.find((mc) => mc.id === 'm-1')?.shown).toBe(true);
	});

	it('gives the admin no switch for their own user, only for their machines', () => {
		const me = visibilityRows(users, machines, [], 'u-me').find((r) => r.id === 'u-me');
		expect(me?.shown).toBeNull();
		expect(me?.machines.map((mc) => mc.id)).toEqual(['m-me']);
	});

	it('is empty before the queries resolve', () => {
		expect(visibilityRows(undefined, undefined, undefined, undefined)).toEqual([]);
	});
});

describe('setShown', () => {
	it('OFF hides the owner, ON shows it again', async () => {
		const api = { hide: vi.fn(async () => undefined), unhide: vi.fn(async () => undefined) };
		await setShown(api, 'user', 'u-bob', 'bob', false);
		expect(api.hide).toHaveBeenCalledWith('user', 'u-bob', 'bob');
		await setShown(api, 'machine', 'm-1', 'alpha', true);
		expect(api.unhide).toHaveBeenCalledWith('machine', 'm-1');
		expect(api.hide).toHaveBeenCalledTimes(1);
	});
});
