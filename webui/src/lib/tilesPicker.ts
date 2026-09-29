import type { SessionListItem } from '@bindings/SessionListItem';
import { filters, freeText, parse, type FilterNode, type Schema } from '@dorsk/tsumikit';
import { tokenizeQuery } from './search';

/**
 * Narrow the tiles "+ Add" picker over the already-loaded live list.
 *
 * The picker is a chooser, not a search: it must answer while the popover is
 * open, so it evaluates the session schema against the rows in hand instead of
 * round-tripping `/sessions/search`. Only the fields a `SessionListItem`
 * actually carries are honoured — a clause naming anything else (a transcript
 * term, `tag:`, `account:`) matches everything rather than nothing, so the
 * picker never silently empties.
 */
export function pickerMatches(s: SessionListItem, raw: string, schema: Schema): boolean {
	const ast = parse(raw, schema);
	const terms = tokenizeQuery(freeText(ast));
	const hay = `${s.name ?? ''} ${s.working_dir} ${s.id}`.toLowerCase();
	if (!terms.every((t) => hay.includes(t.toLowerCase()))) return false;
	return filters(ast).every((f) => matchesClause(s, f));
}

function fieldValue(s: SessionListItem, field: string): string | null {
	switch (field) {
		case 'title':
		case 'name':
			return s.name ?? '';
		case 'dir':
		case 'cwd':
			return s.working_dir;
		case 'id':
			return s.id;
		case 'status':
			return s.status;
		case 'adapter':
			return s.adapter_id ?? '';
		case 'model':
			return s.model ?? '';
		case 'effort':
			return s.effort ?? '';
		case 'machine':
		case 'm':
			return s.machine_id;
		default:
			return null;
	}
}

function matchesClause(s: SessionListItem, f: FilterNode): boolean {
	if (f.field === 'pinned' || f.field === 'starred') {
		const want = f.values[0] !== 'false';
		return (s.pinned === true) === want;
	}
	const value = fieldValue(s, f.field);
	if (value === null) return true;
	const hay = value.toLowerCase();
	const hit = f.values.some((v) => {
		const needle = v.toLowerCase();
		return f.op === 'eq' || f.op === 'ne' || f.op === 'in' ? hay === needle : hay.includes(needle);
	});
	return f.op === 'ne' || f.op === 'not_contains' ? !hit : hit;
}
