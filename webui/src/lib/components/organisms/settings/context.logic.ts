import type { ContextItem } from '@bindings/ContextItem';
import type { ContextItemSpec } from '@bindings/ContextItemSpec';
import type { ContextMatch } from '@bindings/ContextMatch';
import type { BulkContextOutcome } from '@bindings/BulkContextOutcome';
import type { BulkContextUpdate } from '@bindings/BulkContextUpdate';
import type { ContextReach } from '@bindings/ContextReach';

export const CONTEXT_KINDS = ['rule', 'memory', 'prompt'] as const;
export const MATCH_KEYS = ['machines', 'paths', 'labels', 'adapters'] as const;

export type ContextKind = (typeof CONTEXT_KINDS)[number];
export type MatchKey = (typeof MATCH_KEYS)[number];

/** Mirrors the server's `clean_name`: a slug that is also a safe filename. */
const NAME_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;
export const RULE_BODY_MAX = 4096;
export const BODY_MAX = 65536;

export const emptyMatch = (): ContextMatch => ({ machines: [], paths: [], labels: [], adapters: [] });

export function newContextItem(kind: ContextKind): ContextItemSpec {
	return {
		kind,
		name: '',
		title: '',
		body: '',
		scope: 'user',
		scope_ref: null,
		match: emptyMatch(),
		tags: [],
		enabled: true
	};
}

export function toSpec(item: Pick<ContextItem, keyof ContextItemSpec>): ContextItemSpec {
	return {
		kind: item.kind,
		name: item.name,
		title: item.title,
		body: item.body,
		scope: item.scope,
		scope_ref: item.scope_ref,
		match: { ...emptyMatch(), ...item.match },
		tags: item.tags,
		enabled: item.enabled
	};
}

/** Suggest a slug from a title, so the name field rarely needs typing. */
export function slugify(title: string): string {
	return title
		.toLowerCase()
		.normalize('NFD')
		.replace(/[̀-ͯ]/g, '')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, 64)
		.replace(/-+$/, '');
}

/** The server caps bodies in UTF-8 bytes, not characters. */
export const bodyBytes = (body: string) => new TextEncoder().encode(body).length;
export const bodyMax = (kind: string) => (kind === 'rule' ? RULE_BODY_MAX : BODY_MAX);

export const parsePaths = (text: string) =>
	text
		.split('\n')
		.map((p) => p.trim())
		.filter(Boolean);

/**
 * Everything wrong with a draft, as message keys the section maps to copy.
 * Mirrors the server's validation so the form refuses before the round trip.
 */
export function contextProblems(spec: ContextItemSpec): string[] {
	const out: string[] = [];
	if (!spec.title?.trim()) out.push('title');
	if (!NAME_RE.test(spec.name ?? '')) out.push('name');
	if ((spec.match?.paths ?? []).some((p) => !p.startsWith('/'))) out.push('abs_path');
	if (!spec.body?.trim()) out.push('body');
	else if (bodyBytes(spec.body) > bodyMax(spec.kind)) out.push('body_size');
	return out;
}

export const matchIsEmpty = (m: ContextMatch | null | undefined) =>
	!m || MATCH_KEYS.every((k) => !(m[k]?.length ?? 0));

export const reachOf = (m: ContextMatch | null | undefined): ContextReach => m?.reach ?? 'owner';

/** How a summary names the reach: never a bare "everywhere". */
export type ReachWords = { mine: string; allUsers: string; dispatches: string };

export function matchSummary(
	m: ContextMatch | null | undefined,
	words: ReachWords,
	names: { machine?: (id: string) => string; label?: (id: string) => string } = {}
): string {
	const head = [reachOf(m) === 'all_users' ? words.allUsers : words.mine];
	if (m?.dispatch) head.push(words.dispatches);
	const parts = [head.join(' ')];
	if (!m) return parts[0];
	if (m.machines.length) parts.push(m.machines.map(names.machine ?? ((x) => x)).join(' | '));
	if (m.paths.length) parts.push(m.paths.join(' | '));
	if (m.labels.length) parts.push(m.labels.map(names.label ?? ((x) => x)).join(' | '));
	if (m.adapters.length) parts.push(m.adapters.join(' | '));
	if (m.dispatchers?.length) parts.push(m.dispatchers.join(' | '));
	if (m.profiles?.length) parts.push(m.profiles.join(' | '));
	return parts.join(' · ');
}

export type OwnedMachine = { id: string; owner: string; name: string };
export type MachineGroup = { owner: string; mine: boolean; machines: OwnedMachine[] };

/** The caller's machines first, then one group per other owner by name. */
export function groupMachines(
	machines: readonly OwnedMachine[],
	me: string | null | undefined,
	ownerName: (id: string) => string = (id) => id
): MachineGroup[] {
	const byOwner = new Map<string, OwnedMachine[]>();
	for (const mc of machines) byOwner.set(mc.owner, [...(byOwner.get(mc.owner) ?? []), mc]);
	const groups = [...byOwner].map(([owner, list]) => ({
		owner,
		mine: owner === me,
		machines: [...list].sort((a, b) => a.name.localeCompare(b.name))
	}));
	return groups.sort(
		(a, b) => Number(b.mine) - Number(a.mine) || ownerName(a.owner).localeCompare(ownerName(b.owner))
	);
}

/** Listed machines that belong to someone else: they need the all-users reach. */
export const foreignMachines = (m: ContextMatch, machines: readonly OwnedMachine[], me: string | null | undefined) =>
	m.machines.filter((id) => {
		const owner = machines.find((x) => x.id === id)?.owner;
		return owner !== undefined && owner !== me;
	});

export type BulkKey = 'machines' | 'paths' | 'labels' | 'adapters';
export type BulkOp =
	| { op: 'add' | 'remove'; key: BulkKey; values: string[] }
	| { op: 'clear_machines' }
	| { op: 'reach'; reach: ContextReach }
	| { op: 'dispatch'; on: boolean };

export function bulkRequest(ids: readonly string[], change: BulkOp): BulkContextUpdate {
	const req: BulkContextUpdate = { ids: [...new Set(ids)] };
	switch (change.op) {
		case 'add':
		case 'remove':
			req[change.op] = { [change.key]: change.values.map((v) => v.trim()).filter(Boolean) };
			break;
		case 'clear_machines':
			req.set = { clear_machines: true };
			break;
		case 'reach':
			req.set = { reach: change.reach };
			break;
		case 'dispatch':
			req.set = { dispatch: change.on };
			break;
	}
	return req;
}

export type BulkResult = { updated: number; failed: { id: string; error: string }[] };

export const bulkResult = (outcomes: readonly BulkContextOutcome[]): BulkResult => ({
	updated: outcomes.filter((o) => o.item).length,
	failed: outcomes
		.filter((o) => !o.item)
		.map((o) => ({ id: o.id, error: o.error ?? String(o.status ?? '') }))
});

export function toggleAll(selected: ReadonlySet<string>, shown: readonly string[]): Set<string> {
	const all = shown.length > 0 && shown.every((id) => selected.has(id));
	const next = new Set(selected);
	for (const id of shown) {
		if (all) next.delete(id);
		else next.add(id);
	}
	return next;
}

export const itemFilter = (q: string) => {
	const needle = q.trim().toLowerCase();
	return (i: Pick<ContextItem, 'title' | 'name'>) =>
		!needle || i.title.toLowerCase().includes(needle) || i.name.includes(needle);
};

export const toggleIn = (list: readonly string[], v: string) =>
	list.includes(v) ? list.filter((x) => x !== v) : [...list, v];

export type DiffLine = { op: 'same' | 'add' | 'del'; text: string };

export function lineDiff(before: string, after: string): DiffLine[] {
	const a = before.split('\n');
	const b = after.split('\n');
	const lcs: number[][] = Array.from({ length: a.length + 1 }, () => new Array(b.length + 1).fill(0));
	for (let i = a.length - 1; i >= 0; i--)
		for (let j = b.length - 1; j >= 0; j--)
			lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
	const out: DiffLine[] = [];
	let i = 0;
	let j = 0;
	while (i < a.length && j < b.length) {
		if (a[i] === b[j]) {
			out.push({ op: 'same', text: a[i] });
			i++;
			j++;
		} else if (lcs[i + 1][j] >= lcs[i][j + 1]) out.push({ op: 'del', text: a[i++] });
		else out.push({ op: 'add', text: b[j++] });
	}
	while (i < a.length) out.push({ op: 'del', text: a[i++] });
	while (j < b.length) out.push({ op: 'add', text: b[j++] });
	return out;
}

export type FieldChange = { field: 'title' | 'name' | 'enabled' | 'match' | 'tags'; before: string; after: string };

export function pendingChanges(
	live: ContextItem,
	pending: ContextItemSpec,
	summary: (m: ContextMatch) => string
): FieldChange[] {
	const rows: FieldChange[] = [
		{ field: 'title', before: live.title, after: pending.title },
		{ field: 'name', before: live.name, after: pending.name },
		{ field: 'enabled', before: String(live.enabled), after: String(pending.enabled) },
		{ field: 'match', before: summary(live.match), after: summary(pending.match ?? emptyMatch()) },
		{ field: 'tags', before: (live.tags ?? []).join(', '), after: (pending.tags ?? []).join(', ') }
	];
	return rows.filter((r) => r.before !== r.after);
}
