import { filters, parse, serializeFilter, type FilterNode, type Schema } from '@dorsk/tsumikit';

export type TimeScope = '24h' | '7d' | '30d';
export const TIME_SCOPES: readonly TimeScope[] = ['24h', '7d', '30d'];
const SCOPE_DAYS: Record<TimeScope, number> = { '24h': 1, '7d': 7, '30d': 30 };
const DAY_MS = 86_400_000;

export type Scope =
	| { kind: 'machine'; value: string }
	| { kind: 'dir'; value: string }
	| { kind: 'time'; value: TimeScope };

/** `created` bounds are whole UTC days, as the list filter reads them. */
export function sinceDay(t: TimeScope, now: number): string {
	return new Date(now - SCOPE_DAYS[t] * DAY_MS).toISOString().slice(0, 10);
}

function clause(s: Scope, now: number): FilterNode {
	const base = { kind: 'filter', span: [0, 0] } as const;
	switch (s.kind) {
		case 'machine':
			return { ...base, field: 'machine', op: 'eq', values: [s.value] };
		case 'dir':
			return { ...base, field: 'dir', op: 'contains', values: [s.value] };
		case 'time':
			return { ...base, field: 'created', op: 'gte', values: [sinceDay(s.value, now)] };
	}
}

const sameKind = (f: FilterNode, c: FilterNode) => f.field === c.field && f.op === c.op;

const sameClause = (f: FilterNode, c: FilterNode) =>
	sameKind(f, c) && f.values.length === 1 && f.values[0] === c.values[0];

function strip(raw: string, drop: FilterNode[]): string {
	let out = raw;
	for (const f of [...drop].sort((a, b) => b.span[0] - a.span[0])) {
		let end = f.span[1];
		while (out[end] === ' ') end++;
		out = out.slice(0, f.span[0]) + out.slice(end);
	}
	return out.replace(/\s{2,}/g, ' ').trim();
}

export function isScoped(raw: string, schema: Schema, s: Scope, now = Date.now()): boolean {
	const c = clause(s, now);
	return filters(parse(raw, schema)).some((f) => sameClause(f, c));
}

/** One clause per kind: adding a scope replaces another machine, directory or time bound. */
export function toggleScope(raw: string, schema: Schema, s: Scope, now = Date.now()): string {
	const c = clause(s, now);
	const all = filters(parse(raw, schema));
	const on = all.filter((f) => sameClause(f, c));
	if (on.length > 0) return strip(raw, on);
	const rest = strip(raw, all.filter((f) => sameKind(f, c)));
	return [rest, serializeFilter(c)].filter(Boolean).join(' ');
}

export function dirLabel(dir: string): string {
	const parts = dir.replace(/\/+$/, '').split('/');
	return parts[parts.length - 1] || dir;
}
