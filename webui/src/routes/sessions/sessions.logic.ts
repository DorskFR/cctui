// Pure helpers + constants for the sessions page — no Svelte/reactive state, so
// they live outside the component and are unit-testable on their own. Anything
// that reads $state (e.g. the section toggles, the expand set) stays in the
// component; only data-shape transforms and static config live here.
import type { SessionListItem } from '@bindings/SessionListItem';
import { SYSTEM_MACHINE_KINDS } from '$lib/queries';

// ── View picker (CCT-307) ───────────────────────────────────────────────────
// The 4 explicit layout × density combinations offered by the view picker.
export const VIEW_OPTIONS = [
	{ value: 'list-compact', label: 'List · Compact', card: false, dense: true },
	{ value: 'list-detailed', label: 'List · Detailed', card: false, dense: false },
	{ value: 'card-compact', label: 'Card · Compact', card: true, dense: true },
	{ value: 'card-detailed', label: 'Card · Detailed', card: true, dense: false }
] as const;

// ── Section filter (CCT-322 / CCT-345) ──────────────────────────────────────
export type Section = 'starred' | 'live' | 'dispatched' | 'archived';
export const SECTIONS: { value: Section; label: string; icon: 'star' | 'live' | 'send' | 'archive' }[] = [
	{ value: 'starred', label: 'Starred', icon: 'star' },
	{ value: 'live', label: 'Live', icon: 'live' },
	{ value: 'dispatched', label: 'Dispatched', icon: 'send' },
	{ value: 'archived', label: 'Archived', icon: 'archive' }
];
export const isSection = (v: string): v is Section =>
	v === 'starred' || v === 'live' || v === 'dispatched' || v === 'archived';
export const parseSections = (raw: string | null): Set<Section> => {
	const set = new Set<Section>((raw ?? '').split(',').filter(isSection));
	// Never strand the user on an empty list (would render nothing).
	return set.size ? set : new Set<Section>(['starred', 'live', 'dispatched']);
};

// Search/archive pager page size (CCT-184).
export const PAGE = 50;

// ── Subagent grouping (CCT-225 / CCT-269) ───────────────────────────────────
// A subagent group folded under a parent. Workflow-tool subagents (CCT-225)
// carry a `workflow_run_id`; plain (Task-tool) children share the synthetic
// "plain" group. Each group renders inline (always expanded) when it has
// fewer than 3 agents; larger groups collapse behind a count badge on the
// parent row that toggles expand/collapse (CCT-269).
export type SubGroup = {
	// Stable key, unique within a parent: "plain" or "wf:<runId>".
	key: string;
	// Run id for workflow groups; null for the plain group.
	runId: string | null;
	// Tooltip label, e.g. "Workflow: deploy" or "subagents".
	label: string;
	agents: SessionListItem[];
	running: number;
};
export const INLINE_THRESHOLD = 3; // < this → always expanded inline, no badge
export function metaStr(s: SessionListItem, key: string): string | null {
	const m = s.metadata as Record<string, unknown> | null;
	const v = m?.[key];
	return typeof v === 'string' ? v : null;
}
export function metaBool(s: SessionListItem, key: string): boolean {
	const m = s.metadata as Record<string, unknown> | null;
	return m?.[key] === true;
}
export const relationOf = (s: SessionListItem) =>
	metaStr(s, 'relation') ?? (metaBool(s, 'subagent') ? 'subagent' : 'root');
export const runningCount = (agents: SessionListItem[]) =>
	agents.filter((a) => a.status !== 'archived' && a.liveness !== 'dead' && !a.hibernated).length;
// Fold a parent's children into plain + per-workflow groups.
export function groupChildren(kids: SessionListItem[]): SubGroup[] {
	const plain: SessionListItem[] = [];
	const byRun = new Map<string, { name: string | null; agents: SessionListItem[] }>();
	for (const k of kids) {
		const runId = metaStr(k, 'workflow_run_id');
		if (runId) {
			let g = byRun.get(runId);
			if (!g) {
				g = { name: metaStr(k, 'workflow_name'), agents: [] };
				byRun.set(runId, g);
			}
			g.agents.push(k);
		} else {
			plain.push(k);
		}
	}
	const groups: SubGroup[] = [];
	if (plain.length > 0) {
		groups.push({
			key: 'plain',
			runId: null,
			label: 'subagents',
			agents: plain,
			running: runningCount(plain)
		});
	}
	for (const [runId, g] of byRun) {
		groups.push({
			key: `wf:${runId}`,
			runId,
			label: g.name ? `Workflow: ${g.name}` : 'Workflow',
			agents: g.agents,
			running: runningCount(g.agents)
		});
	}
	return groups;
}

// Build the parent→subagent-group nesting for an arbitrary row set. Used for
// the live buckets AND, since CCT-298 item 1, for the archive + search views
// so they keep the same nesting + count badges instead of a flat list. A
// child whose parent is absent from `rows` falls back to top-level so nothing
// is dropped.
export type Nest = {
	topLevel: SessionListItem[];
	childGroups: Map<string, SubGroup[]>;
	hasCollapsible: boolean;
};
export function nest(rows: SessionListItem[]): Nest {
	const ids = new Set(rows.map((s) => s.id));
	const childrenOf = new Map<string, SessionListItem[]>();
	for (const s of rows) {
		if (s.parent_id && ids.has(s.parent_id) && relationOf(s) !== 'fork') {
			childrenOf.set(s.parent_id, [...(childrenOf.get(s.parent_id) ?? []), s]);
		}
	}
	const topLevel = rows.filter((s) => !s.parent_id || !ids.has(s.parent_id) || relationOf(s) === 'fork');
	const childGroups = new Map<string, SubGroup[]>();
	let hasCollapsible = false;
	for (const [parentId, kids] of childrenOf) {
		const groups = groupChildren(kids);
		if (groups.length > 0) childGroups.set(parentId, groups);
		if (groups.some((g) => g.agents.length >= INLINE_THRESHOLD)) hasCollapsible = true;
	}
	return { topLevel, childGroups, hasCollapsible };
}

// Collect the transitive archived descendants of a set of (pinned) parent ids
// from the full session list. A starred parent should keep its whole subagent
// group visible in the Pinned section even after the children were archived
// (CCT-297): the live list excludes archived rows, so we splice these back into
// the nest under their parent. BFS so archived sub-subagents come along too.
export function archivedDescendantsOf(
	parentIds: Set<string>,
	archived: SessionListItem[]
): SessionListItem[] {
	if (parentIds.size === 0 || archived.length === 0) return [];
	const byParent = new Map<string, SessionListItem[]>();
	for (const a of archived) {
		if (a.parent_id)
			byParent.set(a.parent_id, [...(byParent.get(a.parent_id) ?? []), a]);
	}
	const out: SessionListItem[] = [];
	const seen = new Set<string>();
	const queue = [...parentIds];
	while (queue.length) {
		const pid = queue.shift() as string;
		for (const child of byParent.get(pid) ?? []) {
			if (seen.has(child.id)) continue;
			seen.add(child.id);
			out.push(child);
			queue.push(child.id);
		}
	}
	return out;
}

// Aggregated subagent usage for a parent (CCT-297 #19, tokens per CCT-301 #2):
// the parent's own total tokens plus every subagent's, with the agent count.
// Reported in tokens (not dollars). Null when there are no agents.
export const totalTokens = (u: SessionListItem['token_usage']) =>
	Number(u.tokens_in) +
	Number(u.tokens_out) +
	Number(u.cache_read_tokens) +
	Number(u.cache_creation_tokens);
export function costRollup(
	s: SessionListItem,
	groups: SubGroup[]
): { tokens: number; count: number } | null {
	const agents = groups.flatMap((g) => g.agents);
	if (agents.length === 0) return null;
	const tokens =
		totalTokens(s.token_usage) + agents.reduce((n, a) => n + totalTokens(a.token_usage), 0);
	return { tokens, count: agents.length };
}

// Stable key for a collapsible subagent group's expand/collapse state.
export const groupId = (parentId: string, key: string) => `${parentId}/${key}`;

// ── Classifier buckets (CCT-90) ─────────────────────────────────────────────
// In attention-first display order. Sessions that want the user's eyes float to
// the top; empty buckets are dropped. Sessions on server-managed machines
// (dispatch / ephemeral workers) get their own "Dispatched" group at the bottom
// (CCT-231) — they're unattended noise next to interactive sessions — EXCEPT
// blocked ones, which still surface under Needs input so attention never gets
// buried.
export type GroupKey = SessionListItem['bucket'] | 'dispatched' | 'pinned';
export const BUCKETS: { key: GroupKey; label: string }[] = [
	// Pinned/starred sessions (CCT-267) float above every bucket.
	{ key: 'pinned', label: 'Pinned' },
	{ key: 'blocked', label: 'Needs input' },
	{ key: 'review', label: 'Ready for review' },
	{ key: 'working', label: 'Working' },
	{ key: 'done', label: 'Completed' },
	{ key: 'dispatched', label: 'Dispatched' }
];
export const isDispatched = (s: SessionListItem) =>
	s.machine_kind != null && SYSTEM_MACHINE_KINDS.has(s.machine_kind);
export const groupOf = (s: SessionListItem): GroupKey => {
	if (s.pinned) return 'pinned';
	const bucket = s.bucket ?? 'working';
	if (bucket === 'blocked') return 'blocked';
	return isDispatched(s) ? 'dispatched' : bucket;
};
