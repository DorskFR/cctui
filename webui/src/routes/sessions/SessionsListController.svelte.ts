import type { SessionListItem } from '@bindings/SessionListItem';
import {
	BUCKETS,
	bucketInSection,
	groupId,
	groupOf,
	groupRows,
	matchesUnreadFilter,
	nest,
	rangeIds,
	sortSessions,
	type Dimension,
	type GroupKey,
	type Section,
	type SessionSort,
	type SessionSortDir,
	type SubGroup
} from './sessions.logic';

const bucketed = (dim: Dimension) => dim === 'none' || dim === 'status';

export interface SessionsListInputs {
	items: () => SessionListItem[];
	// Archived descendants of pinned parents, spliced back under their parent.
	pinnedArchivedKids: () => SessionListItem[];
	sections: () => Set<Section>;
	groupBy: () => Dimension;
	sort: () => SessionSort;
	sortDir: () => SessionSortDir;
	matchesLabel: (s: SessionListItem) => boolean;
	matchesClient: (s: SessionListItem) => boolean;
	// Visual document order of the rendered rows, for shift-range selection.
	renderedOrder: () => string[];
}

export class SessionsListController {
	#in: SessionsListInputs;

	selecting = $state(false);
	selected = $state(new Set<string>());
	anchorId = $state<string | null>(null);
	// Expand/collapse of collapsible (>=3) subagent groups, keyed by
	// `${parentId}/${group.key}`. Default collapsed.
	expanded = $state(new Set<string>());

	constructor(inputs: SessionsListInputs) {
		this.#in = inputs;
	}

	#sort = (rows: SessionListItem[]) => sortSessions(rows, this.#in.sort(), this.#in.sortDir());
	// A parent stays when a subagent nested under it matches, so a filter on a
	// child's state still shows the row it lives under.
	#matchesClientDeep = (s: SessionListItem, depth = 0): boolean =>
		this.#in.matchesClient(s) ||
		(depth < 5 &&
			(this.#liveNest.childGroups.get(s.id) ?? []).some((g) =>
				g.agents.some((a) => this.#matchesClientDeep(a, depth + 1))
			));
	#keep = (s: SessionListItem): boolean =>
		this.#in.matchesLabel(s) &&
		this.#matchesClientDeep(s) &&
		matchesUnreadFilter(s, this.#in.sections());

	draftRows = $derived.by(() => this.#in.items().filter((s) => s.status === 'draft'));

	#liveNest = $derived.by(() =>
		nest([...this.#in.items().filter((s) => s.status !== 'draft'), ...this.#in.pinnedArchivedKids()])
	);
	get topLevel(): SessionListItem[] {
		return this.#liveNest.topLevel;
	}
	get childGroupsOf(): Map<string, SubGroup[]> {
		return this.#liveNest.childGroups;
	}

	groups = $derived.by(() =>
		BUCKETS.filter((b) => bucketInSection(b.key, this.#in.sections()))
			.map((b) => ({
				...b,
				sessions: this.#sort(
					this.#liveNest.topLevel.filter((s) => groupOf(s) === b.key && this.#keep(s))
				)
			}))
	);

	#liveTopFiltered = $derived.by(() =>
		this.#liveNest.topLevel.filter(
			(s) => bucketInSection(groupOf(s), this.#in.sections()) && this.#keep(s)
		)
	);
	// Tiles are one flat, sorted run of the live rows the list would show:
	// buckets change as a session works, and a tile must not move when it does.
	tileRows = $derived.by(() =>
		this.#sort(this.#liveTopFiltered.filter((s) => s.status !== 'archived'))
	);

	groupedSections = $derived.by(() =>
		bucketed(this.#in.groupBy())
			? []
			: groupRows(this.#sort(this.#liveTopFiltered), this.#in.groupBy())
	);
	get hasLiveRows(): boolean {
		return this.liveSectionCounts.some((g) => g.count > 0);
	}
	// Row count per rendered live section, keyed like their collapse toggles.
	get liveSectionCounts(): { key: string; count: number }[] {
		return bucketed(this.#in.groupBy())
			? this.groups.map((g) => ({ key: g.key, count: g.sessions.length }))
			: this.groupedSections.map((g) => ({ key: `dim:${g.key}`, count: g.sessions.length }));
	}

	toggleGroup = (parentId: string, key: string) => {
		const id = groupId(parentId, key);
		const next = new Set(this.expanded);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		this.expanded = next;
	};
	isExpanded = (parentId: string, key: string): boolean => this.expanded.has(groupId(parentId, key));

	toggleSelect = (s: SessionListItem, range = false) => {
		const next = new Set(this.selected);
		if (range && this.anchorId && this.anchorId !== s.id) {
			const selectable = new Set(this.#in.items().map((x) => x.id));
			const ids = rangeIds(this.#in.renderedOrder(), this.anchorId, s.id, selectable);
			if (ids.length) {
				for (const id of ids) next.add(id);
				this.selected = next;
				return;
			}
		}
		if (next.has(s.id)) next.delete(s.id);
		else next.add(s.id);
		this.anchorId = s.id;
		this.selected = next;
	};
	exitSelect = () => {
		this.selecting = false;
		this.selected = new Set();
		this.anchorId = null;
	};
	selectAll = () => {
		this.selected = new Set(this.#in.items().map((s) => s.id));
	};

	bucketInSection = (key: GroupKey): boolean => bucketInSection(key, this.#in.sections());
}
