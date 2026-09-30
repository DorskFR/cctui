import { describe, expect, it } from 'vitest';
import type { UserAction } from '@bindings/UserAction';
import { groupUserActions } from './userActions';

const action = (over: Partial<UserAction>): UserAction => ({
	id: over.id ?? 'id-1',
	title: over.title ?? 'Approve PR #12',
	detail: over.detail ?? null,
	kind: over.kind ?? 'action',
	blocking: over.blocking ?? false,
	status: over.status ?? 'open',
	note: over.note ?? null,
	created_at: over.created_at ?? '2026-09-30T10:00:00Z',
	resolved_at: over.resolved_at ?? null,
	resolved_by: over.resolved_by ?? null
});

describe('groupUserActions', () => {
	it('renders nothing for a session that never had a list', () => {
		expect(groupUserActions([])).toBeNull();
		expect(groupUserActions(undefined)).toBeNull();
	});

	it('puts blocking items first, then the rest oldest-first', () => {
		const got = groupUserActions([
			action({ id: 'a', created_at: '2026-09-30T10:00:00Z' }),
			action({ id: 'b', created_at: '2026-09-30T11:00:00Z' }),
			action({ id: 'c', blocking: true, created_at: '2026-09-30T12:00:00Z' })
		]);
		expect(got?.open.map((a) => a.id)).toEqual(['c', 'a', 'b']);
		expect(got?.blocking).toBe(1);
	});

	it('separates done and dropped items from the open ones', () => {
		const got = groupUserActions([
			action({ id: 'open', status: 'open' }),
			action({ id: 'done', status: 'done', resolved_at: '2026-09-30T11:00:00Z' }),
			action({ id: 'dropped', status: 'dropped', resolved_at: '2026-09-30T10:30:00Z' })
		]);
		expect(got?.open.map((a) => a.id)).toEqual(['open']);
		expect(got?.resolved.map((a) => a.id)).toEqual(['dropped', 'done']);
		expect(got?.blocking).toBe(0);
	});

	it('still groups a list whose items are all resolved, so the card keeps its "n done" line', () => {
		const got = groupUserActions([action({ id: 'done', status: 'done' })]);
		expect(got?.open).toEqual([]);
		expect(got?.resolved).toHaveLength(1);
	});

	it('tolerates an unparseable timestamp instead of scrambling the order', () => {
		const got = groupUserActions([
			action({ id: 'bad', created_at: 'not a date' }),
			action({ id: 'good', created_at: '2026-09-30T10:00:00Z' })
		]);
		expect(got?.open.map((a) => a.id)).toEqual(['bad', 'good']);
	});
});
