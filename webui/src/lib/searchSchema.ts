import { filters, parse, type FilterNode, type Schema, type ValueOption } from '@dorsk/tsumikit';
import type { SessionListItem } from '@bindings/SessionListItem';

export const SERVER_FIELDS = new Set([
	'machine',
	'account',
	'tag',
	'title',
	'status',
	'model',
	'effort',
	'adapter',
	'pinned',
	'dir'
]);

export const SESSION_SEARCH_PLACEHOLDER =
	'Search — status:active machine:… label:… or free text';

export type FetchValues = (field: string, q: string) => Promise<string[]>;

function providerFor(field: string, fetchValues: FetchValues) {
	return async (q: string): Promise<ValueOption[]> => {
		const vals = await fetchValues(field, q);
		return vals.map((v) => ({ value: v, label: v }));
	};
}

export function buildSessionSearchSchema(fetchValues: FetchValues): Schema {
	return {
		fields: [
			{
				name: 'title',
				label: 'Title',
				type: 'string',
				aliases: ['name'],
				operators: ['contains', 'not_contains'],
				valuePlaceholder: 'session title…'
			},
			{
				name: 'machine',
				label: 'Machine',
				type: 'id',
				aliases: ['m'],
				operators: ['eq', 'ne', 'in'],
				provider: providerFor('machine', fetchValues)
			},
			{
				name: 'account',
				label: 'Account',
				type: 'id',
				aliases: ['acct'],
				operators: ['eq', 'ne', 'in'],
				provider: providerFor('account', fetchValues)
			},
			{
				name: 'tag',
				label: 'Label',
				type: 'enum',
				aliases: ['label'],
				operators: ['eq', 'ne', 'in'],
				provider: providerFor('tag', fetchValues)
			},
			{
				name: 'status',
				label: 'Status',
				type: 'enum',
				operators: ['eq', 'ne', 'in'],
				options: ['new', 'active', 'inactive', 'archived', 'draft'].map((v) => ({
					value: v,
					label: v
				}))
			},
			{
				name: 'model',
				label: 'Model',
				type: 'string',
				operators: ['contains', 'not_contains'],
				provider: providerFor('model', fetchValues)
			},
			{
				name: 'effort',
				label: 'Effort',
				type: 'enum',
				operators: ['eq', 'ne'],
				options: ['low', 'high'].map((v) => ({ value: v, label: v }))
			},
			{
				name: 'adapter',
				label: 'Adapter',
				type: 'enum',
				operators: ['eq', 'ne'],
				options: ['claude-code', 'codex'].map((v) => ({ value: v, label: v }))
			},
			{
				name: 'pinned',
				label: 'Pinned',
				type: 'bool',
				aliases: ['starred'],
				operators: ['eq'],
				options: [
					{ value: 'true', label: 'true' },
					{ value: 'false', label: 'false' }
				]
			},
			{
				name: 'dir',
				label: 'Directory',
				type: 'string',
				aliases: ['cwd'],
				operators: ['contains', 'not_contains'],
				valuePlaceholder: '/path/fragment…'
			},
			{
				name: 'id',
				label: 'Session ID',
				type: 'string',
				operators: ['contains', 'not_contains'],
				valuePlaceholder: 'id fragment…'
			},
			{
				name: 'created',
				label: 'Created',
				type: 'date',
				aliases: ['time'],
				operators: ['gte', 'lte', 'gt', 'lt', 'range'],
				valuePlaceholder: 'YYYY-MM-DD'
			}
		]
	};
}

export interface SplitQuery {
	serverQuery: string;
	clientFilters: FilterNode[];
}

// Client-only clauses are peeled off the raw string: the server treats an
// unknown `field:value` as literal free text (matching nothing), so `id`/
// `created` must be stripped before send and applied to the loaded list.
export function splitQuery(raw: string, schema: Schema): SplitQuery {
	const ast = parse(raw, schema);
	const clientFilters = filters(ast).filter((f) => !SERVER_FIELDS.has(f.field));
	let serverQuery = raw;
	for (const f of [...clientFilters].sort((a, b) => b.span[0] - a.span[0])) {
		const [a, b] = f.span;
		let end = b;
		while (serverQuery[end] === ' ') end++;
		serverQuery = serverQuery.slice(0, a) + serverQuery.slice(end);
	}
	return { serverQuery: serverQuery.replace(/\s{2,}/g, ' ').trim(), clientFilters };
}

const DAY_MS = 86_400_000;

function matchesDate(t: number, f: FilterNode): boolean {
	const at = Date.parse(f.values[0] ?? '');
	if (Number.isNaN(at)) return true;
	switch (f.op) {
		case 'gt':
			return t >= at + DAY_MS;
		case 'gte':
			return t >= at;
		case 'lt':
			return t < at;
		case 'lte':
			return t < at + DAY_MS;
		case 'range': {
			const bt = Date.parse(f.values[1] ?? '');
			return t >= at && (Number.isNaN(bt) || t < bt + DAY_MS);
		}
		default:
			return true;
	}
}

function matchesOne(s: SessionListItem, f: FilterNode): boolean {
	if (f.field === 'id') {
		const hit = f.values.some((v) => s.id.toLowerCase().includes(v.toLowerCase()));
		return f.op === 'not_contains' ? !hit : hit;
	}
	if (f.field === 'created') {
		const t = s.registered_at ? Date.parse(s.registered_at) : Number.NaN;
		return Number.isNaN(t) ? false : matchesDate(t, f);
	}
	return true;
}

/** AND-narrow a session against the client-only clauses from {@link splitQuery}. */
export function matchesClientFilters(s: SessionListItem, clientFilters: FilterNode[]): boolean {
	return clientFilters.every((f) => matchesOne(s, f));
}
