import { describe, expect, it } from 'vitest';
import type { HiddenOwner } from '@bindings/HiddenOwner';
import type { MachineRow } from '@bindings/MachineRow';
import type { UserRow } from '@bindings/UserRow';
import {
	canHideUser,
	isHiddenOwner,
	isHiddenSession,
	ownerLabel,
	sessionOwnerTargets,
	sortedOwners,
	visibleSessions
} from './hiddenOwners';

const machine = (over: Partial<MachineRow> = {}): MachineRow => ({
	id: 'm-bob',
	user_id: 'u-bob',
	name: 'bob-box',
	display_name: null,
	first_seen_at: '',
	last_seen_at: '',
	revoked_at: null,
	kind: 'personal',
	hue: null,
	key_preview: null,
	liveness: 'online',
	...over
});
const user = (id: string, name: string): UserRow => ({
	id,
	name,
	created_at: '',
	revoked_at: null,
	disabled_at: null,
	can_dispatch: true,
	last_seen_at: null
});
const owners: HiddenOwner[] = [
	{ kind: 'machine', target_id: 'm-zed', label: 'zed' },
	{ kind: 'user', target_id: 'u-bob', label: 'bob' },
	{ kind: 'machine', target_id: 'm-alpha', label: null }
];

describe('isHiddenOwner', () => {
	it('matches on kind and id together', () => {
		expect(isHiddenOwner(owners, 'user', 'u-bob')).toBe(true);
		expect(isHiddenOwner(owners, 'machine', 'u-bob')).toBe(false);
		expect(isHiddenOwner(owners, 'user', 'u-amy')).toBe(false);
	});

	it('treats a list not loaded yet as nothing hidden', () => {
		expect(isHiddenOwner(undefined, 'machine', 'm-zed')).toBe(false);
	});
});

describe('canHideUser', () => {
	it('refuses the viewer’s own user', () => {
		expect(canHideUser('u-me', 'u-me')).toBe(false);
	});

	it('allows any other user', () => {
		expect(canHideUser('u-me', 'u-bob')).toBe(true);
	});

	it('refuses while the viewer is unknown', () => {
		expect(canHideUser(null, 'u-bob')).toBe(false);
		expect(canHideUser(undefined, 'u-bob')).toBe(false);
	});
});

describe('show hidden filtering', () => {
	const rows = [{ id: 'a' }, { id: 'b', hidden: true }, { id: 'c', hidden: false }];

	it('drops hidden rows while Show hidden is off', () => {
		expect(visibleSessions(rows, false).map((s) => s.id)).toEqual(['a', 'c']);
	});

	it('keeps hidden rows while Show hidden is on', () => {
		expect(visibleSessions(rows, true).map((s) => s.id)).toEqual(['a', 'b', 'c']);
	});

	it('marks only rows the server flagged hidden', () => {
		expect(rows.map(isHiddenSession)).toEqual([false, true, false]);
	});
});

describe('sessionOwnerTargets', () => {
	const session = { machine_id: 'm-bob', machine_name: 'Bob laptop' };

	it('offers the owner user and the machine', () => {
		const t = sessionOwnerTargets(session, {
			meId: 'u-me',
			machines: [machine()],
			users: [user('u-bob', 'bob')],
			owners: []
		});
		expect(t).toEqual([
			{ kind: 'user', target_id: 'u-bob', label: 'bob', hidden: false },
			{ kind: 'machine', target_id: 'm-bob', label: 'Bob laptop', hidden: false }
		]);
	});

	it('never offers the viewer’s own user, only their machine', () => {
		const t = sessionOwnerTargets(session, {
			meId: 'u-bob',
			machines: [machine()],
			users: [user('u-bob', 'bob')],
			owners: []
		});
		expect(t.map((x) => x.kind)).toEqual(['machine']);
	});

	it('flags what is already hidden so the entry reads as Unhide', () => {
		const t = sessionOwnerTargets(session, {
			meId: 'u-me',
			machines: [machine()],
			users: [],
			owners
		});
		expect(t.find((x) => x.kind === 'user')).toMatchObject({ hidden: true, label: 'u-bob' });
		expect(t.find((x) => x.kind === 'machine')?.hidden).toBe(false);
	});

	it('offers just the machine when its owner is unknown', () => {
		const t = sessionOwnerTargets(
			{ machine_id: 'm-gone', machine_name: null },
			{ meId: 'u-me', machines: [machine()], users: [], owners: [] }
		);
		expect(t).toEqual([{ kind: 'machine', target_id: 'm-gone', label: 'm-gone', hidden: false }]);
	});

	it('falls back to the machine row’s names', () => {
		const t = sessionOwnerTargets(
			{ machine_id: 'm-bob', machine_name: null },
			{ meId: 'u-bob', machines: [machine({ display_name: 'Desk' })], users: [], owners: [] }
		);
		expect(t[0].label).toBe('Desk');
	});
});

describe('sortedOwners', () => {
	it('groups machines before users, each by label', () => {
		expect(sortedOwners(owners).map(ownerLabel)).toEqual(['m-alpha', 'zed', 'bob']);
	});
});
