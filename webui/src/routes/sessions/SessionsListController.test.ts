// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { SessionsListController, type SessionsListInputs } from './SessionsListController.svelte';
import {
	inEnabledSections,
	sectionVisibility,
	type Section,
	type Dimension,
	type SessionSort
} from './sessions.logic';

function session(over: Partial<SessionListItem>): SessionListItem {
	return {
		id: 'sess',
		labels: [],
		working_dir: '',
		unread_count: 0,
		status: 'active',
		bucket: 'working',
		pinned: false,
		...over
	} as SessionListItem;
}

function make(over: Partial<SessionsListInputs> = {}) {
	let items: SessionListItem[] = [];
	let sections = new Set<Section>(['starred', 'live', 'dispatched']);
	let groupBy: Dimension = 'none';
	let sort: SessionSort = 'activity';
	let order: string[] = [];
	const inputs: SessionsListInputs = {
		items: () => items,
		pinnedArchivedKids: () => [],
		sections: () => sections,
		groupBy: () => groupBy,
		sort: () => sort,
		sortDir: () => 'desc',
		matchesLabel: () => true,
		matchesClient: () => true,
		renderedOrder: () => order,
		...over
	};
	const ctl = new SessionsListController(inputs);
	return {
		ctl,
		setItems: (v: SessionListItem[]) => (items = v),
		setSections: (v: Set<Section>) => (sections = v),
		setGroupBy: (v: Dimension) => (groupBy = v),
		setSort: (v: SessionSort) => (sort = v),
		setOrder: (v: string[]) => (order = v)
	};
}

const working = (ctl: SessionsListController) =>
	ctl.groups.find((g) => g.key === 'working')?.sessions ?? [];

describe('SessionsListController — buckets', () => {
	it('partitions top-level rows into attention buckets, keeping empty ones', () => {
		const { ctl, setItems } = make();
		setItems([
			session({ id: 'w', bucket: 'working' }),
			session({ id: 'd', bucket: 'done' }),
			session({ id: 'p', pinned: true, bucket: 'working' })
		]);
		const groups = ctl.groups;
		const byKey = Object.fromEntries(groups.map((g) => [g.key, g.sessions.map((s) => s.id)]));
		expect(byKey.pinned).toEqual(['p']);
		expect(byKey.working).toEqual(['w']);
		expect(byKey.done).toEqual(['d']);
		expect(byKey.blocked).toEqual([]);
	});

	it('hides a bucket whose owning section toggle is off', () => {
		const { ctl, setItems, setSections } = make();
		setItems([session({ id: 'p', pinned: true }), session({ id: 'w', bucket: 'working' })]);
		setSections(new Set<Section>(['live']));
		expect(ctl.groups.map((g) => g.key)).toEqual(['blocked', 'review', 'working', 'done']);
		expect(ctl.groups.flatMap((g) => g.sessions.map((x) => x.id))).toEqual(['w']);
	});

	it('excludes drafts from the live nest and exposes them separately', () => {
		const { ctl, setItems } = make();
		setItems([session({ id: 'dr', status: 'draft' }), session({ id: 'w', bucket: 'working' })]);
		expect(ctl.draftRows.map((s) => s.id)).toEqual(['dr']);
		expect(ctl.groups.flatMap((g) => g.sessions.map((s) => s.id))).toEqual(['w']);
	});

	it('applies the created sort within a bucket', () => {
		const { ctl, setItems, setSort } = make();
		setSort('created');
		setItems([
			session({ id: 'old', bucket: 'working', registered_at: '2020-01-01T00:00:00Z' }),
			session({ id: 'new', bucket: 'working', registered_at: '2024-01-01T00:00:00Z' })
		]);
		expect(working(ctl).map((s) => s.id)).toEqual(['new', 'old']);
	});

	it('applies the sort direction within a bucket', () => {
		const rows = [
			session({ id: 'old', bucket: 'working', registered_at: '2020-01-01T00:00:00Z' }),
			session({ id: 'new', bucket: 'working', registered_at: '2024-01-01T00:00:00Z' })
		];
		const desc = make({ sort: () => 'created', sortDir: () => 'desc' });
		desc.setItems(rows);
		expect(working(desc.ctl).map((s) => s.id)).toEqual(['new', 'old']);
		const asc = make({ sort: () => 'created', sortDir: () => 'asc' });
		asc.setItems(rows);
		expect(working(asc.ctl).map((s) => s.id)).toEqual(['old', 'new']);
	});

	it('respects the injected label/client filters', () => {
		const { ctl, setItems } = make({ matchesClient: (s) => s.id !== 'hidden' });
		setItems([session({ id: 'hidden', bucket: 'working' }), session({ id: 'shown', bucket: 'working' })]);
		expect(working(ctl).map((s) => s.id)).toEqual(['shown']);
	});
});

describe('SessionsListController — group-by dimension', () => {
	it('is empty when group-by is none, and hasLiveRows tracks the buckets', () => {
		const { ctl, setItems } = make();
		setItems([session({ id: 'w', bucket: 'working' })]);
		expect(ctl.groupedSections).toEqual([]);
		expect(ctl.hasLiveRows).toBe(true);
	});

	it('reports no live rows when every bucket header is empty', () => {
		const { ctl } = make();
		expect(ctl.groups.length).toBeGreaterThan(0);
		expect(ctl.hasLiveRows).toBe(false);
	});

	it('re-partitions the live rows by the chosen dimension', () => {
		const { ctl, setItems, setGroupBy } = make();
		setGroupBy('working_dir');
		setItems([
			session({ id: 'a', bucket: 'working', working_dir: '/x/api' }),
			session({ id: 'b', bucket: 'working', working_dir: '/x/web' })
		]);
		expect(ctl.groupedSections.map((g) => g.label)).toEqual(['api', 'web']);
		expect(ctl.hasLiveRows).toBe(true);
	});
});

describe('SessionsListController — expand/collapse', () => {
	it('toggles a subagent group open then closed', () => {
		const { ctl } = make();
		expect(ctl.isExpanded('p', 'plain')).toBe(false);
		ctl.toggleGroup('p', 'plain');
		expect(ctl.isExpanded('p', 'plain')).toBe(true);
		ctl.toggleGroup('p', 'plain');
		expect(ctl.isExpanded('p', 'plain')).toBe(false);
	});
});

describe('SessionsListController — multi-select', () => {
	it('toggles a single row in and out of the selection and sets the anchor', () => {
		const { ctl, setItems } = make();
		setItems([session({ id: 'a' }), session({ id: 'b' })]);
		ctl.toggleSelect(session({ id: 'a' }));
		expect([...ctl.selected]).toEqual(['a']);
		expect(ctl.anchorId).toBe('a');
		ctl.toggleSelect(session({ id: 'a' }));
		expect([...ctl.selected]).toEqual([]);
	});

	it('shift-range selects the visible span between anchor and target', () => {
		const { ctl, setItems, setOrder } = make();
		setItems([session({ id: 'a' }), session({ id: 'b' }), session({ id: 'c' }), session({ id: 'd' })]);
		setOrder(['a', 'b', 'c', 'd']);
		ctl.toggleSelect(session({ id: 'a' }));
		ctl.toggleSelect(session({ id: 'c' }), true);
		expect([...ctl.selected].sort()).toEqual(['a', 'b', 'c']);
	});

	it('falls back to a plain toggle when the range endpoints are off screen', () => {
		const { ctl, setItems, setOrder } = make();
		setItems([session({ id: 'a' }), session({ id: 'z' })]);
		setOrder([]);
		ctl.toggleSelect(session({ id: 'a' }));
		ctl.toggleSelect(session({ id: 'z' }), true);
		expect([...ctl.selected].sort()).toEqual(['a', 'z']);
	});

	it('selectAll picks every loaded item; exitSelect clears everything', () => {
		const { ctl, setItems } = make();
		setItems([session({ id: 'a' }), session({ id: 'b' })]);
		ctl.selecting = true;
		ctl.selectAll();
		expect([...ctl.selected].sort()).toEqual(['a', 'b']);
		ctl.exitSelect();
		expect(ctl.selecting).toBe(false);
		expect(ctl.selected.size).toBe(0);
		expect(ctl.anchorId).toBeNull();
	});
});

describe('SessionsListController — tile rows', () => {
	it('is one flat sorted run, not the bucket groups', () => {
		const { ctl, setItems, setSort } = make();
		setSort('name');
		setItems([
			session({ id: 'b', name: 'beta', bucket: 'done' }),
			session({ id: 'a', name: 'alpha', bucket: 'working' }),
			session({ id: 'c', name: 'gamma', pinned: true })
		]);
		expect(ctl.tileRows.map((s) => s.id)).toEqual(['c', 'b', 'a']);
	});

	it('never includes an archived or draft session', () => {
		const { ctl, setItems } = make();
		setItems([
			session({ id: 'live' }),
			session({ id: 'gone', status: 'archived' }),
			session({ id: 'draft', status: 'draft' })
		]);
		expect(ctl.tileRows.map((s) => s.id)).toEqual(['live']);
	});

	it('drops dispatched rows while their section is off', () => {
		const { ctl, setItems, setSections } = make();
		setSections(new Set<Section>(['starred', 'live']));
		setItems([session({ id: 'live' }), session({ id: 'disp', machine_kind: 'dispatch' })]);
		expect(ctl.tileRows.map((s) => s.id)).toEqual(['live']);
	});

	it('brings dispatched rows back once their section is on', () => {
		const { ctl, setItems, setSections } = make();
		setSections(new Set<Section>(['starred', 'live', 'dispatched']));
		setItems([session({ id: 'live' }), session({ id: 'disp', machine_kind: 'dispatch' })]);
		expect(ctl.tileRows.map((s) => s.id).sort()).toEqual(['disp', 'live']);
	});
});

describe('SessionsListController — section visibility', () => {
	const W = session({ id: 'w', bucket: 'working' });
	const D = session({ id: 'd', bucket: 'working', machine_kind: 'dispatch' });
	const X = session({ id: 'x', status: 'archived', bucket: 'done' });

	// Archived rows come from the pager, narrowed like `SessionsPage.keepRow`.
	function rendered(sections: Section[], rows: SessionListItem[]) {
		const enabled = new Set(sections);
		const { ctl, setItems, setSections, setGroupBy } = make();
		setSections(enabled);
		setGroupBy('status');
		setItems(rows.filter((s) => s.status !== 'archived'));
		const archived = rows.filter((s) => s.status === 'archived' && inEnabledSections(s, enabled));
		const vis = sectionVisibility({
			groups: ctl.liveSectionCounts,
			drafts: enabled.has('drafts') ? ctl.draftRows.length : null,
			archived: enabled.has('archived')
				? { rows: archived.length, loading: false, more: false, error: false }
				: null
		});
		const shown = ctl.liveSectionCounts
			.filter((g) => vis.groups.has(g.key))
			.map((g) => `${g.key} ${g.count}`);
		if (vis.archived) shown.push(`archived ${archived.length}`);
		expect(vis.empty).toBe(shown.length === 0);
		return shown;
	}

	const cases: [string, Section[], SessionListItem[], string[]][] = [
		['live only, nothing', ['live'], [], []],
		['live only, one working', ['live'], [W], ['working 1']],
		['live only, one archived', ['live'], [X], []],
		['live only, working + archived', ['live'], [W, X], ['working 1']],
		['archived only, nothing', ['archived'], [], []],
		['archived only, one working', ['archived'], [W], []],
		['archived only, one archived', ['archived'], [X], ['archived 1']],
		['archived only, working + archived', ['archived'], [W, X], ['archived 1']],
		['live + archived, nothing', ['live', 'archived'], [], []],
		['live + archived, one working', ['live', 'archived'], [W], ['working 1']],
		['live + archived, one archived', ['live', 'archived'], [X], ['archived 1']],
		['live + archived, both', ['live', 'archived'], [W, X], ['working 1', 'archived 1']],
		['dispatched only, nothing', ['dispatched'], [], []],
		['dispatched only, one working', ['dispatched'], [W], []],
		['dispatched only, one dispatched', ['dispatched'], [D], ['dispatched 1']],
		['dispatched only, dispatched + archived', ['dispatched'], [D, X], ['dispatched 1']]
	];
	for (const [name, sections, rows, want] of cases) {
		it(`renders exactly the non-empty enabled sections: ${name}`, () => {
			expect(rendered(sections, rows)).toEqual(want);
		});
	}

	it('keys group-by sections like their collapse toggles and drops empty ones', () => {
		const { ctl, setItems, setGroupBy } = make();
		setGroupBy('working_dir');
		setItems([session({ id: 'a', working_dir: '/w/a' })]);
		const counts = ctl.liveSectionCounts;
		expect(counts.length).toBe(1);
		expect(counts[0].key.startsWith('dim:')).toBe(true);
		expect(counts[0].count).toBe(1);
	});
});
