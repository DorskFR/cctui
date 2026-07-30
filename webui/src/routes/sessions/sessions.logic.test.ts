import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import {
	accountTrafficWarning,
	bucketInSection,
	colorHueOf,
	dimGroupsOf,
	DIM_NONE_KEY,
	draftEditPrefill,
	draftPayload,
	draftPromptPreview,
	fmtWhen,
	formatAgo,
	groupRows,
	isDimension,
	isSection,
	kanbanColOf,
	matchesLabelFilter,
	matchesUnreadFilter,
	parseLabelFilter,
	parseSections,
	pickFreshSession,
	rangeIds,
	scriptPrefill,
	sessionDebugRows,
	sessionHrefFor,
	sessionIdFromLocation,
	sortSessions,
	toolActivity,
	TOOL_ASLEEP_AFTER_MS,
	type Section
} from './sessions.logic';
import type { Label } from '@bindings/Label';

function session(over: Partial<SessionListItem>): SessionListItem {
	return { id: 'sess-abc', labels: [], working_dir: '', unread_count: 0, ...over } as SessionListItem;
}

const label = (id: string, name: string, color = ''): Label => ({ id, name, color });

describe('accountTrafficWarning', () => {
	it('warns only for an account-bound session with no observed gateway traffic', () => {
		// Bound + never observed → warn (may be riding ambient creds).
		expect(accountTrafficWarning(session({ account_name: 'work', account_traffic_observed: false }))).toBe(
			true
		);
		// Bound + observed → no warning.
		expect(accountTrafficWarning(session({ account_name: 'work', account_traffic_observed: true }))).toBe(
			false
		);
		// Unbound (no account) → nothing to warn about, whatever the flag.
		expect(accountTrafficWarning(session({ account_name: null, account_traffic_observed: false }))).toBe(
			false
		);
		// Field absent (older server) → never a false warning.
		expect(accountTrafficWarning(session({ account_name: 'work' }))).toBe(false);
	});
});

describe('unread section', () => {
	it('recognises "unread" as a section', () => {
		expect(isSection('unread')).toBe(true);
	});

	it('round-trips through parseSections', () => {
		expect(parseSections('live,unread')).toEqual(new Set<Section>(['live', 'unread']));
	});

	it('is off by default (never in the fallback set)', () => {
		expect(parseSections(null).has('unread')).toBe(false);
		expect(parseSections('').has('unread')).toBe(false);
	});

	describe('matchesUnreadFilter', () => {
		const on = new Set<Section>(['live', 'unread']);
		const off = new Set<Section>(['live']);

		it('passes every row when the filter is off', () => {
			expect(matchesUnreadFilter(session({ unread_count: 0 }), off)).toBe(true);
			expect(matchesUnreadFilter(session({ unread_count: 3 }), off)).toBe(true);
		});

		it('keeps only rows with unread messages when on', () => {
			expect(matchesUnreadFilter(session({ unread_count: 0 }), on)).toBe(false);
			expect(matchesUnreadFilter(session({ unread_count: 1 }), on)).toBe(true);
		});

		it('treats a missing count as zero unread', () => {
			expect(matchesUnreadFilter(session({ unread_count: undefined }), on)).toBe(false);
		});
	});
});

describe('tool activity — asleep vs. grinding', () => {
	const NOW = 1_000_000_000_000;
	const working = (over: Partial<SessionListItem>) =>
		session({ bucket: 'working', status: 'active', ...over });

	it('is hidden with no tool activity and no headline', () => {
		expect(toolActivity(working({}), NOW).show).toBe(false);
	});

	it('shows the headline even before any tool call', () => {
		const a = toolActivity(working({ activity_detail: 'compiling…' }), NOW);
		expect(a.show).toBe(true);
		expect(a.detail).toBe('compiling…');
		expect(a.ageMs).toBeNull();
		expect(a.asleep).toBe(false);
	});

	it('reads as grinding when the last tool call is fresh', () => {
		const a = toolActivity(
			working({ last_tool_at: new Date(NOW - 10_000).toISOString(), tool_use_count: 42 }),
			NOW
		);
		expect(a.show).toBe(true);
		expect(a.count).toBe(42);
		expect(a.ageMs).toBe(10_000);
		expect(a.asleep).toBe(false);
	});

	it('reads as asleep once tool calls stop past the threshold', () => {
		const a = toolActivity(
			working({ last_tool_at: new Date(NOW - TOOL_ASLEEP_AFTER_MS - 1_000).toISOString() }),
			NOW
		);
		expect(a.asleep).toBe(true);
		expect(a.show).toBe(true);
	});

	it('is never asleep for a non-working bucket', () => {
		const a = toolActivity(
			session({
				bucket: 'done',
				status: 'active',
				last_tool_at: new Date(NOW - TOOL_ASLEEP_AFTER_MS - 1_000).toISOString()
			}),
			NOW
		);
		expect(a.asleep).toBe(false);
		expect(a.show).toBe(false);
	});
});

describe('debug tooltip rows', () => {
	const NOW = 1_000_000_000_000;

	it('renders nulls as "—", never "null"', () => {
		const rows = sessionDebugRows(session({ liveness: 'dead' }), NOW);
		const values = Object.fromEntries(rows.map((r) => [r.label, r.value]));
		expect(values.account).toBe('—');
		expect(values.created).toBe('—');
		expect(values.machine).toBe('—');
		expect(values.keepalive).toBe('—');
		expect(JSON.stringify(rows)).not.toContain('null');
	});

	it('shows account, machine (+ non-persistent kind) and credential state', () => {
		const rows = sessionDebugRows(
			session({
				account_name: 'work',
				machine_name: 'runner-1',
				machine_kind: 'dispatch',
				has_token_credentials: true
			}),
			NOW
		);
		const values = Object.fromEntries(rows.map((r) => [r.label, r.value]));
		expect(values.account).toBe('work');
		expect(values.machine).toBe('runner-1 (dispatch)');
		expect(values.creds).toBe('live token binding');
	});

	it('omits the kind suffix for persistent machines', () => {
		const rows = sessionDebugRows(
			session({ machine_name: 'laptop', machine_kind: 'persistent' }),
			NOW
		);
		expect(rows.find((r) => r.label === 'machine')?.value).toBe('laptop');
	});

	it('flags an account with no live token binding', () => {
		const rows = sessionDebugRows(
			session({ account_name: 'work', has_token_credentials: false }),
			NOW
		);
		expect(rows.find((r) => r.label === 'creds')?.value).toBe(
			'account only (token revoked/absent)'
		);
	});

	it('reports hibernated as the status word', () => {
		const rows = sessionDebugRows(session({ hibernated: true, liveness: 'dead' }), NOW);
		expect(rows.find((r) => r.label === 'status')?.value).toBe('hibernated');
	});
});

describe('kanbanColOf', () => {
	it('routes a draft to the Drafts column', () => {
		expect(kanbanColOf(session({ status: 'draft' }))).toBe('drafts');
	});

	it('routes a blocked session to Needs input', () => {
		expect(kanbanColOf(session({ status: 'active', bucket: 'blocked' }))).toBe('blocked');
	});

	it('routes a done session to Completed', () => {
		expect(kanbanColOf(session({ status: 'active', bucket: 'done' }))).toBe('done');
	});

	it('routes working / review / missing bucket to Working', () => {
		expect(kanbanColOf(session({ status: 'active', bucket: 'working' }))).toBe('working');
		expect(kanbanColOf(session({ status: 'active', bucket: 'review' }))).toBe('working');
		expect(kanbanColOf(session({ status: 'active', bucket: undefined }))).toBe('working');
	});

	it('classifies a pinned working session by its raw bucket, not a pinned column', () => {
		expect(kanbanColOf(session({ status: 'active', pinned: true, bucket: 'working' }))).toBe(
			'working'
		);
		expect(kanbanColOf(session({ status: 'active', pinned: true, bucket: 'blocked' }))).toBe(
			'blocked'
		);
	});

	it('folds dispatched sessions into their bucket column (working / blocked)', () => {
		expect(
			kanbanColOf(session({ status: 'active', machine_kind: 'dispatch', bucket: 'working' }))
		).toBe('working');
		expect(
			kanbanColOf(session({ status: 'active', machine_kind: 'dispatch', bucket: 'blocked' }))
		).toBe('blocked');
	});

	it('keeps archived sessions off the board (null)', () => {
		expect(kanbanColOf(session({ status: 'archived', bucket: 'done' }))).toBeNull();
	});
});

describe('dimension color / group', () => {
	it('recognises the dimension enum values', () => {
		expect(isDimension('none')).toBe(true);
		expect(isDimension('label')).toBe(true);
		expect(isDimension('working_dir')).toBe(true);
		expect(isDimension('machine')).toBe(true);
		expect(isDimension('nope')).toBe(false);
	});

	describe('dimGroupsOf', () => {
		it('splits a session into one membership per label', () => {
			const s = session({ labels: [label('l1', 'infra'), label('l2', 'urgent')] });
			const gs = dimGroupsOf(s, 'label');
			expect(gs.map((g) => g.label)).toEqual(['infra', 'urgent']);
			expect(gs.map((g) => g.key)).toEqual(['label:l1', 'label:l2']);
		});

		it('routes an unlabelled session to the "—" bucket', () => {
			const gs = dimGroupsOf(session({ labels: [] }), 'label');
			expect(gs).toEqual([{ key: DIM_NONE_KEY, label: '—', hue: null }]);
		});

		it('groups working_dir by its basename', () => {
			const gs = dimGroupsOf(session({ working_dir: '/home/dev/cctui' }), 'working_dir');
			expect(gs).toHaveLength(1);
			expect(gs[0].label).toBe('cctui');
			expect(gs[0].key).toBe('dir:/home/dev/cctui');
		});

		it('sends a session with no working dir to "—"', () => {
			expect(dimGroupsOf(session({ working_dir: '' }), 'working_dir')[0].key).toBe(DIM_NONE_KEY);
		});

		it('prefers an operator-set machine hue over the name hash', () => {
			const gs = dimGroupsOf(session({ machine_name: 'runner', machine_hue: 200 }), 'machine');
			expect(gs[0].label).toBe('runner');
			expect(gs[0].hue).toBe(200);
		});

		it('sends a machineless session to "—"', () => {
			expect(dimGroupsOf(session({ machine_name: null }), 'machine')[0].key).toBe(DIM_NONE_KEY);
		});
	});

	describe('colorHueOf', () => {
		it('is null for the none dimension', () => {
			expect(colorHueOf(session({ working_dir: '/a/b' }), 'none')).toBeNull();
		});

		it('is null when the session is missing the dimension', () => {
			expect(colorHueOf(session({ labels: [] }), 'label')).toBeNull();
		});

		it('is deterministic and stable for the same working dir', () => {
			const a = colorHueOf(session({ working_dir: '/home/dev/cctui' }), 'working_dir');
			const b = colorHueOf(session({ working_dir: '/home/dev/cctui' }), 'working_dir');
			expect(a).toBe(b);
			expect(a).not.toBeNull();
			expect(a as number).toBeGreaterThanOrEqual(0);
			expect(a as number).toBeLessThan(360);
		});

		it('gives different working dirs distinct hues', () => {
			expect(colorHueOf(session({ working_dir: '/x/api' }), 'working_dir')).not.toBe(
				colorHueOf(session({ working_dir: '/x/web' }), 'working_dir')
			);
		});

		it('takes the primary (first) label hue', () => {
			const s = session({ labels: [label('l1', 'a', '120'), label('l2', 'b', '240')] });
			expect(colorHueOf(s, 'label')).toBe(120);
		});
	});

	describe('groupRows', () => {
		it('wraps every row in one unlabelled section for none', () => {
			const rows = [session({ id: 'a' }), session({ id: 'b' })];
			const gs = groupRows(rows, 'none');
			expect(gs).toHaveLength(1);
			expect(gs[0].sessions).toHaveLength(2);
		});

		it('partitions by working dir and sorts groups by name', () => {
			const rows = [
				session({ id: 'a', working_dir: '/x/web' }),
				session({ id: 'b', working_dir: '/x/api' }),
				session({ id: 'c', working_dir: '/x/api' })
			];
			const gs = groupRows(rows, 'working_dir');
			expect(gs.map((g) => g.label)).toEqual(['api', 'web']);
			expect(gs[0].sessions.map((s) => s.id)).toEqual(['b', 'c']);
		});

		it('puts the "—" bucket last regardless of name', () => {
			const rows = [
				session({ id: 'a', machine_name: null }),
				session({ id: 'b', machine_name: 'zeta' })
			];
			const gs = groupRows(rows, 'machine');
			expect(gs.map((g) => g.label)).toEqual(['zeta', '—']);
			expect(gs[gs.length - 1].key).toBe(DIM_NONE_KEY);
		});

		it('lists a multi-labelled session under each of its labels', () => {
			const rows = [session({ id: 'a', labels: [label('l1', 'infra'), label('l2', 'urgent')] })];
			const gs = groupRows(rows, 'label');
			expect(gs.map((g) => g.label)).toEqual(['infra', 'urgent']);
			expect(gs.every((g) => g.sessions[0].id === 'a')).toBe(true);
		});

		it('preserves incoming row order within a group', () => {
			const rows = [
				session({ id: 'first', working_dir: '/x/api' }),
				session({ id: 'second', working_dir: '/x/api' })
			];
			expect(groupRows(rows, 'working_dir')[0].sessions.map((s) => s.id)).toEqual([
				'first',
				'second'
			]);
		});
	});
});

describe('fmtWhen', () => {
	const NOW = 1_000_000_000_000;
	it('returns "—" for missing or unparseable timestamps', () => {
		expect(fmtWhen(null, NOW)).toBe('—');
		expect(fmtWhen(undefined, NOW)).toBe('—');
		expect(fmtWhen('not-a-date', NOW)).toBe('—');
	});
	it('pairs a relative age with the raw ISO', () => {
		const iso = new Date(NOW - 90_000).toISOString();
		expect(fmtWhen(iso, NOW)).toBe(`1m ago · ${iso}`);
	});
});

describe('formatAgo', () => {
	it('formats seconds, minutes, hours', () => {
		expect(formatAgo(5_000)).toBe('5s');
		expect(formatAgo(90_000)).toBe('1m');
		expect(formatAgo(3 * 3600_000)).toBe('3h');
		expect(formatAgo(-100)).toBe('0s');
	});
});

describe('sessionIdFromLocation', () => {
	const search = (qs = '') => new URL(`http://x${qs}`).searchParams;

	it('reads the id from the shallow-routed /sessions/<id> path', () => {
		expect(sessionIdFromLocation('/sessions/abc-123', search())).toBe('abc-123');
	});

	it('decodes a percent-encoded path id', () => {
		expect(sessionIdFromLocation('/sessions/a%2Fb', search())).toBe('a/b');
	});

	it('falls back to the ?session= query param when the path is bare', () => {
		expect(sessionIdFromLocation('/sessions', search('?session=q-9'))).toBe('q-9');
	});

	it('prefers the path id over the query param', () => {
		expect(sessionIdFromLocation('/sessions/path-id', search('?session=query-id'))).toBe('path-id');
	});

	it('is null with neither a path id nor a query param', () => {
		expect(sessionIdFromLocation('/sessions', search())).toBeNull();
	});
});

describe('sessionHrefFor', () => {
	it('builds the /sessions/<id> path and drops a stale ?session=', () => {
		expect(sessionHrefFor('http://h/sessions?session=old', 'new-1')).toBe(
			'http://h/sessions/new-1'
		);
	});

	it('encodes the id in the path', () => {
		expect(sessionHrefFor('http://h/sessions', 'a/b')).toBe('http://h/sessions/a%2Fb');
	});

	it('closes to /sessions when id is null', () => {
		expect(sessionHrefFor('http://h/sessions/abc', null)).toBe('http://h/sessions');
	});

	it('is null (no navigation) when the target already matches the current href', () => {
		expect(sessionHrefFor('http://h/sessions/abc', 'abc')).toBeNull();
		expect(sessionHrefFor('http://h/sessions', null)).toBeNull();
	});
});

describe('pickFreshSession', () => {
	it('is null when nothing is open', () => {
		expect(pickFreshSession(null, [session({ id: 'a' })])).toBeNull();
	});

	it('returns the live copy from the pools, not the stale fallback', () => {
		const stale = session({ id: 'a', name: 'old' });
		const fresh = session({ id: 'a', name: 'new' });
		expect(pickFreshSession(stale, [session({ id: 'b' }), fresh])?.name).toBe('new');
	});

	it('falls back to the held object when the id is not in any pool', () => {
		const held = session({ id: 'a', name: 'held' });
		expect(pickFreshSession(held, [session({ id: 'b' })])).toBe(held);
	});
});

describe('label filter', () => {
	it('parseLabelFilter splits a comma list and drops empties', () => {
		expect(parseLabelFilter('l1,l2')).toEqual(['l1', 'l2']);
		expect(parseLabelFilter('')).toEqual([]);
		expect(parseLabelFilter(null)).toEqual([]);
		expect(parseLabelFilter('l1,,l2,')).toEqual(['l1', 'l2']);
	});

	it('matchesLabelFilter passes every row when the filter is empty', () => {
		expect(matchesLabelFilter(session({ labels: [label('l1', 'a')] }), new Set())).toBe(true);
	});

	it('matchesLabelFilter keeps rows carrying at least one selected label (OR)', () => {
		const s = session({ labels: [label('l1', 'a'), label('l2', 'b')] });
		expect(matchesLabelFilter(s, new Set(['l2']))).toBe(true);
		expect(matchesLabelFilter(s, new Set(['l9']))).toBe(false);
	});
});

describe('sortSessions', () => {
	it('keeps the server order for the activity sort (same reference)', () => {
		const rows = [session({ id: 'a' }), session({ id: 'b' })];
		expect(sortSessions(rows, 'activity')).toBe(rows);
	});

	it('sorts by registered_at descending for created', () => {
		const rows = [
			session({ id: 'old', registered_at: '2020-01-01T00:00:00Z' }),
			session({ id: 'new', registered_at: '2024-01-01T00:00:00Z' })
		];
		expect(sortSessions(rows, 'created').map((s) => s.id)).toEqual(['new', 'old']);
	});

	it('sorts by name, falling back to working-dir basename then id', () => {
		const rows = [
			session({ id: 'z', name: 'zebra' }),
			session({ id: 'a', name: 'apple' }),
			session({ id: 'm', name: '', working_dir: '/x/mango' })
		];
		expect(sortSessions(rows, 'name').map((s) => s.id)).toEqual(['a', 'm', 'z']);
	});

	it('does not mutate the input array', () => {
		const rows = [session({ id: 'b', name: 'b' }), session({ id: 'a', name: 'a' })];
		sortSessions(rows, 'name');
		expect(rows.map((s) => s.id)).toEqual(['b', 'a']);
	});
});

describe('bucketInSection', () => {
	it('maps the pinned bucket to the starred section toggle', () => {
		expect(bucketInSection('pinned', new Set<Section>(['starred']))).toBe(true);
		expect(bucketInSection('pinned', new Set<Section>(['live']))).toBe(false);
	});

	it('maps the dispatched bucket to the dispatched toggle', () => {
		expect(bucketInSection('dispatched', new Set<Section>(['dispatched']))).toBe(true);
		expect(bucketInSection('dispatched', new Set<Section>(['live']))).toBe(false);
	});

	it('maps every other bucket to the live toggle', () => {
		expect(bucketInSection('working', new Set<Section>(['live']))).toBe(true);
		expect(bucketInSection('blocked', new Set<Section>(['live']))).toBe(true);
		expect(bucketInSection('done', new Set<Section>(['starred']))).toBe(false);
	});
});

describe('draft payload + prefills', () => {
	it('draftPayload reads the draft object off metadata, else empty', () => {
		expect(draftPayload(session({ metadata: { draft: { prompt: 'hi' } } }))).toEqual({
			prompt: 'hi'
		});
		expect(draftPayload(session({ metadata: null }))).toEqual({});
		expect(draftPayload(session({ metadata: { draft: 'nope' } }))).toEqual({});
	});

	it('draftPromptPreview returns the stored prompt string, else empty', () => {
		expect(draftPromptPreview(session({ metadata: { draft: { prompt: 'go' } } }))).toBe('go');
		expect(draftPromptPreview(session({ metadata: { draft: {} } }))).toBe('');
	});

	it('scriptPrefill seeds the claude model field by default', () => {
		const p = scriptPrefill(
			session({ machine_id: 'm1', working_dir: '/w', model: 'opus', adapter_id: 'claude-code' })
		);
		expect(p).toEqual({
			machine_id: 'm1',
			working_dir: '/w',
			adapter_id: 'claude-code',
			name: '',
			model_claude: 'opus'
		});
	});

	it('scriptPrefill seeds the codex model field for a codex session', () => {
		const p = scriptPrefill(session({ machine_id: 'm', working_dir: '/w', adapter_id: 'codex', model: 'gpt' }));
		expect(p.model_codex).toBe('gpt');
		expect(p.adapter_id).toBe('codex');
	});

	it('draftEditPrefill prefers stored payload, falling back to the row', () => {
		const p = draftEditPrefill(
			session({
				machine_id: 'row-m',
				working_dir: '/row',
				adapter_id: 'claude-code',
				metadata: { draft: { name: 'd', prompt: 'p', model: 'sonnet', effort: 'high', working_dir: '/draft' } }
			})
		);
		expect(p.name).toBe('d');
		expect(p.prompt).toBe('p');
		expect(p.model_claude).toBe('sonnet');
		expect(p.effort_claude).toBe('high');
		expect(p.working_dir).toBe('/draft');
		expect(p.machine_id).toBe('row-m');
	});

	it('draftEditPrefill routes model/effort to codex fields when the draft is codex', () => {
		const p = draftEditPrefill(
			session({ machine_id: 'm', working_dir: '/w', metadata: { draft: { adapter_id: 'codex', model: 'gpt', effort: 'low' } } })
		);
		expect(p.model_codex).toBe('gpt');
		expect(p.effort_codex).toBe('low');
	});
});

describe('rangeIds', () => {
	const order = ['a', 'b', 'c', 'd', 'e'];
	const all = new Set(order);

	it('returns the inclusive span downwards', () => {
		expect(rangeIds(order, 'b', 'd', all)).toEqual(['b', 'c', 'd']);
	});

	it('returns the same span when clicked upwards', () => {
		expect(rangeIds(order, 'd', 'b', all)).toEqual(['b', 'c', 'd']);
	});

	it('drops ids that are not selectable', () => {
		expect(rangeIds(order, 'a', 'e', new Set(['a', 'c', 'e']))).toEqual(['a', 'c', 'e']);
	});

	it('is empty when an endpoint is not on screen', () => {
		expect(rangeIds(order, 'z', 'c', all)).toEqual([]);
		expect(rangeIds(order, 'c', 'z', all)).toEqual([]);
	});

	it('handles a single-row range', () => {
		expect(rangeIds(order, 'c', 'c', all)).toEqual(['c']);
	});
});
