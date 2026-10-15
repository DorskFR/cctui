import type { ContextItem } from '@bindings/ContextItem';
import type { ContextItemSpec } from '@bindings/ContextItemSpec';
import type { ContextMatch } from '@bindings/ContextMatch';

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

export function matchSummary(
	m: ContextMatch | null | undefined,
	always: string,
	names: { machine?: (id: string) => string; label?: (id: string) => string } = {}
): string {
	if (!m || matchIsEmpty(m)) return always;
	const parts: string[] = [];
	if (m.machines.length) parts.push(m.machines.map(names.machine ?? ((x) => x)).join(' | '));
	if (m.paths.length) parts.push(m.paths.join(' | '));
	if (m.labels.length) parts.push(m.labels.map(names.label ?? ((x) => x)).join(' | '));
	if (m.adapters.length) parts.push(m.adapters.join(' | '));
	return parts.join(' · ');
}

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
