// Pure helpers + constants for the sessions page — no Svelte/reactive state, so
// they live outside the component and are unit-testable on their own. Anything
// that reads $state (e.g. the section toggles, the expand set) stays in the
// component; only data-shape transforms and static config live here.
import type { SessionListItem } from '@bindings/SessionListItem';
import { SYSTEM_MACHINE_KINDS } from '$lib/queries';
import { hashHue } from '$lib/format';
import { labelHue } from '$lib/labels';
import { m } from '$lib/paraglide/messages';

// ── View picker (CCT-307) ───────────────────────────────────────────────────
// The 4 explicit layout × density combinations offered by the view picker.
export const VIEW_OPTIONS = [
	{ value: 'list-compact', get label() { return m.sessions_view_list_compact(); }, card: false, dense: true },
	{ value: 'list-detailed', get label() { return m.sessions_view_list_detailed(); }, card: false, dense: false },
	{ value: 'card-compact', get label() { return m.sessions_view_card_compact(); }, card: true, dense: true },
	{ value: 'card-detailed', get label() { return m.sessions_view_card_detailed(); }, card: true, dense: false },
	// Kanban (CCT-579) needs a flag of its own: its card×dense pair collides with card-compact.
	{ value: 'kanban', get label() { return m.sessions_view_kanban(); }, card: true, dense: true }
] as const;

// ── Section filter (CCT-322 / CCT-345) ──────────────────────────────────────
export type Section = 'starred' | 'live' | 'dispatched' | 'drafts' | 'archived' | 'unread';
export const SECTIONS: { value: Section; label: string; icon: 'star' | 'live' | 'send' | 'file-text' | 'archive' | 'bell' }[] = [
	{ value: 'starred', get label() { return m.sessions_section_starred(); }, icon: 'star' },
	{ value: 'live', get label() { return m.sessions_section_live(); }, icon: 'live' },
	{ value: 'dispatched', get label() { return m.sessions_dispatched(); }, icon: 'send' },
	// Draft/staged sessions (CCT-394) — buffered spawns not yet launched.
	{ value: 'drafts', get label() { return m.sessions_section_drafts(); }, icon: 'file-text' },
	{ value: 'archived', get label() { return m.sessions_section_archived(); }, icon: 'archive' },
	// Unread (CCT-580): a cross-cutting AND-filter (unread_count > 0), not an
	// ownership bucket — it narrows whatever buckets are shown.
	{ value: 'unread', get label() { return m.sessions_section_unread(); }, icon: 'bell' }
];
export const isSection = (v: string): v is Section =>
	v === 'starred' ||
	v === 'live' ||
	v === 'dispatched' ||
	v === 'drafts' ||
	v === 'archived' ||
	v === 'unread';

// Unread section (CCT-580) is a predicate over rendered rows, not a bucket: a
// row survives when the filter is off, or when it has unread messages.
export const matchesUnreadFilter = (s: SessionListItem, sections: Set<Section>): boolean =>
	!sections.has('unread') || (s.unread_count ?? 0) > 0;
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
			label: m.sessions_subagents(),
			agents: plain,
			running: runningCount(plain)
		});
	}
	for (const [runId, g] of byRun) {
		groups.push({
			key: `wf:${runId}`,
			runId,
			label: g.name ? m.sessions_workflow_named({ name: g.name }) : m.sessions_workflow(),
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
	{ key: 'pinned', get label() { return m.sessions_bucket_pinned(); } },
	{ key: 'blocked', get label() { return m.sessions_bucket_blocked(); } },
	{ key: 'review', get label() { return m.sessions_bucket_review(); } },
	{ key: 'working', get label() { return m.sessions_bucket_working(); } },
	{ key: 'done', get label() { return m.sessions_bucket_done(); } },
	{ key: 'dispatched', get label() { return m.sessions_dispatched(); } }
];
export const isDispatched = (s: SessionListItem) =>
	s.machine_kind != null && SYSTEM_MACHINE_KINDS.has(s.machine_kind);

// ── Stale Working sessions (CCT-365) ────────────────────────────────────────
// A session in the Working bucket whose latest activity (`last_heartbeat`, also
// bumped by subagent work up the parent chain — CCT-366) is older than this
// threshold is *not* actively working: it's blocked-undetected, waiting on us,
// or the connection dropped. We signal it as stale (dimmed card + dot) rather
// than presenting it as live. This is a DERIVED, time-based display signal
// (like the liveness tiers) computed client-side from `last_heartbeat`, NOT a
// persisted state — it re-evaluates on the UI clock tick and clears the instant
// fresh activity arrives. It is a separate, longer-horizon signal from the 90s
// `inactive_after_secs` tempo window on the server (which drives `liveness`).
//
// Single named constant for now; CCT-357 (Settings/Preferences) will promote it
// to a per-user/per-account setting.
export const STALE_WORKING_AFTER_MS = 30 * 60 * 1000; // 30 minutes

// Whether a session should be signalled stale: a Working-bucket session whose
// last heartbeat is older than the threshold. `now` is passed in so callers can
// drive re-evaluation off a clock tick. Non-Working buckets are never stale
// (blocked/review/done already carry their own signal), and a session with no
// heartbeat timestamp (stub rows) is treated as not-stale.
export function isStaleWorking(s: SessionListItem, now: number): boolean {
	if (groupOf(s) !== 'working') return false;
	if (!s.last_heartbeat) return false;
	return now - new Date(s.last_heartbeat).getTime() > STALE_WORKING_AFTER_MS;
}

// ── Live tool activity: asleep vs. grinding (CCT-594) ────────────────────────
// A Working session with a fresh `last_tool_at` (bumped by its own tool calls
// AND, rolled up the parent chain like the heartbeat, by any subagent's) is
// visibly grinding; one whose newest tool call is older than this — while still
// in the Working bucket — reads as asleep/wedged. Much tighter than the 30-min
// STALE_WORKING horizon and evidence-based (a real ToolUse resets it), so a
// churning subagent keeps the parent alive while a truly wedged parent surfaces.
export const TOOL_ASLEEP_AFTER_MS = 2 * 60 * 1000; // 2 minutes

export type ToolActivity = {
	// Whether to show the activity snippet at all (working + something to show).
	show: boolean;
	// This session's own per-turn tool count.
	count: number;
	// ms since the newest tool call (own or rolled-up subagent); null when none.
	ageMs: number | null;
	// Current spinner headline, when the daemon reported one.
	detail: string | null;
	// Working but no tool call for longer than TOOL_ASLEEP_AFTER_MS → wedged.
	asleep: boolean;
};

export function toolActivity(s: SessionListItem, now: number): ToolActivity {
	const working = groupOf(s) === 'working' && s.status !== 'archived';
	const ageMs = s.last_tool_at ? Math.max(0, now - new Date(s.last_tool_at).getTime()) : null;
	const count = s.tool_use_count ?? 0;
	const detail = s.activity_detail ?? null;
	const asleep = working && ageMs !== null && ageMs > TOOL_ASLEEP_AFTER_MS;
	const show = working && (ageMs !== null || !!detail);
	return { show, count, ageMs, detail, asleep };
}

// Compact "12s" / "3m" / "1h" age label for the tool-cadence indicator.
export function formatAgo(ms: number): string {
	const s = Math.max(0, Math.round(ms / 1000));
	if (s < 60) return `${s}s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m}m`;
	return `${Math.floor(m / 60)}h`;
}
// ── Debug tooltip on the activity dot (CCT-555) ──────────────────────────────
// Key/value rows for the dot's hover panel. Pure + testable; the session id is
// rendered separately (copyable) by the component. Nulls render as "—", never
// "null". Timestamps carry both a relative age and the raw ISO.
export type DebugRow = { label: string; value: string };

export function fmtWhen(iso: string | null | undefined, now: number): string {
	if (!iso) return '—';
	const t = new Date(iso).getTime();
	if (Number.isNaN(t)) return '—';
	return `${formatAgo(now - t)} ago · ${iso}`;
}

export function sessionDebugRows(s: SessionListItem, now: number): DebugRow[] {
	const machine = s.machine_name
		? s.machine_kind && s.machine_kind !== 'persistent'
			? `${s.machine_name} (${s.machine_kind})`
			: s.machine_name
		: '—';
	const statusWord = s.hibernated
		? 'hibernated'
		: isStaleWorking(s, now)
			? 'stale'
			: (s.liveness ?? 'dead');
	const creds = s.has_token_credentials
		? 'live token binding'
		: s.account_name
			? 'account only (token revoked/absent)'
			: 'none';
	return [
		{ label: 'account', value: s.account_name ?? '—' },
		{ label: 'created', value: fmtWhen(s.registered_at, now) },
		{ label: 'machine', value: machine },
		{ label: 'keepalive', value: fmtWhen(s.last_heartbeat, now) },
		{ label: 'creds', value: creds },
		{ label: 'status', value: statusWord }
	];
}

export const groupOf = (s: SessionListItem): GroupKey => {
	if (s.pinned) return 'pinned';
	const bucket = s.bucket ?? 'working';
	if (bucket === 'blocked') return 'blocked';
	return isDispatched(s) ? 'dispatched' : bucket;
};

// Map a session to the single view section that owns it, mirroring the live
// bucket→section mapping (pinned→starred, dispatched→dispatched, else→live)
// plus the archive split. Used so search results respect the active section
// toggles (CCT-354): a match is hidden when its owning section is disabled.
export const sectionOf = (s: SessionListItem): Section => {
	if (s.status === 'draft') return 'drafts';
	if (s.status === 'archived') return 'archived';
	if (s.pinned) return 'starred';
	return isDispatched(s) ? 'dispatched' : 'live';
};
export const inEnabledSections = (s: SessionListItem, sections: Set<Section>): boolean =>
	sections.has(sectionOf(s));

// ── Kanban board (CCT-579) ──────────────────────────────────────────────────
export type KanbanCol = 'drafts' | 'blocked' | 'working' | 'done';
export const KANBAN_COLS: { key: KanbanCol; label: string }[] = [
	{ key: 'drafts', get label() { return m.sessions_section_drafts(); } },
	{ key: 'blocked', get label() { return m.sessions_bucket_blocked(); } },
	{ key: 'working', get label() { return m.sessions_bucket_working(); } },
	{ key: 'done', get label() { return m.sessions_bucket_done(); } }
];

// Pinned rows classify by raw `bucket`, not `groupOf` (which short-circuits to
// 'pinned'); dispatched + review fold into Working.
export function kanbanColOf(s: SessionListItem): KanbanCol | null {
	if (s.status === 'archived') return null;
	if (s.status === 'draft') return 'drafts';
	const bucket = s.bucket ?? 'working';
	if (bucket === 'blocked') return 'blocked';
	if (bucket === 'done') return 'done';
	return 'working';
}

// ── Color / group dimension (CCT-466 color · CCT-467 group) ──────────────────
export type Dimension = 'none' | 'label' | 'working_dir' | 'machine';
export const DIMENSIONS: { value: Dimension; label: string }[] = [
	{ value: 'none', get label() { return m.common_none(); } },
	{ value: 'label', get label() { return m.sessions_dim_label(); } },
	{ value: 'working_dir', get label() { return m.sessions_dim_working_dir(); } },
	{ value: 'machine', get label() { return m.sessions_dim_machine(); } }
];
export const isDimension = (v: string): v is Dimension =>
	v === 'none' || v === 'label' || v === 'working_dir' || v === 'machine';

export const DIM_NONE_KEY = '__none__';
export const DIM_NONE_LABEL = '—';

export type DimGroup = { key: string; label: string; hue: number | null };

// A multi-labelled session is a member of EACH of its labels (the card repeats);
// working_dir/machine yield exactly one membership.
export function dimGroupsOf(s: SessionListItem, dim: Dimension): DimGroup[] {
	if (dim === 'label') {
		if (s.labels.length === 0)
			return [{ key: DIM_NONE_KEY, label: DIM_NONE_LABEL, hue: null }];
		return s.labels.map((l) => ({ key: `label:${l.id}`, label: l.name, hue: labelHue(l) }));
	}
	if (dim === 'working_dir') {
		const dir = s.working_dir ?? '';
		if (!dir) return [{ key: DIM_NONE_KEY, label: DIM_NONE_LABEL, hue: null }];
		const name = dir.split('/').filter(Boolean).pop() || dir;
		return [{ key: `dir:${dir}`, label: name, hue: hashHue(dir) }];
	}
	if (dim === 'machine') {
		const name = s.machine_name;
		if (!name) return [{ key: DIM_NONE_KEY, label: DIM_NONE_LABEL, hue: null }];
		// Operator-set machine hue (CCT-222) wins over the name hash so a machine
		// reads the same color as its badge everywhere.
		return [{ key: `machine:${name}`, label: name, hue: s.machine_hue ?? hashHue(name) }];
	}
	return [];
}

export function colorHueOf(s: SessionListItem, dim: Dimension): number | null {
	if (dim === 'none') return null;
	return dimGroupsOf(s, dim)[0]?.hue ?? null;
}

export type RowGroup = { key: string; label: string; hue: number | null; sessions: SessionListItem[] };

// Sorted by name, "—" bucket last; input row order preserved within each group.
export function groupRows(rows: SessionListItem[], dim: Dimension): RowGroup[] {
	if (dim === 'none') return [{ key: '__all__', label: '', hue: null, sessions: rows }];
	const map = new Map<string, RowGroup>();
	for (const s of rows) {
		for (const g of dimGroupsOf(s, dim)) {
			let rg = map.get(g.key);
			if (!rg) {
				rg = { key: g.key, label: g.label, hue: g.hue, sessions: [] };
				map.set(g.key, rg);
			}
			rg.sessions.push(s);
		}
	}
	return [...map.values()].sort((a, b) => {
		const au = a.key === DIM_NONE_KEY;
		const bu = b.key === DIM_NONE_KEY;
		if (au !== bu) return au ? 1 : -1;
		return a.label.localeCompare(b.label);
	});
}

// Shift-click range selection: the ids between the anchor and the clicked row in
// visual order, restricted to the rows that are actually selectable. Returns an
// empty array when either end isn't on screen, which the caller reads as "fall
// back to a plain toggle".
export function rangeIds(
	order: string[],
	anchorId: string,
	targetId: string,
	selectable: Set<string>
): string[] {
	const a = order.indexOf(anchorId);
	const b = order.indexOf(targetId);
	if (a < 0 || b < 0) return [];
	return order
		.slice(Math.min(a, b), Math.max(a, b) + 1)
		.filter((id) => selectable.has(id));
}
