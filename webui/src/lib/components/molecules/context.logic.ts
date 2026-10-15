import type { ContextItem } from '@bindings/ContextItem';

export type Pickable = Pick<ContextItem, 'id' | 'kind' | 'name' | 'title' | 'tags'>;
export type PickBy = 'name' | 'id';

const FIELD_WEIGHT = { name: 3, title: 2, tag: 1 } as const;

export function fuzzyScore(q: string, text: string): number | null {
	const t = text.toLowerCase();
	let from = 0;
	let prev = -2;
	let score = 0;
	for (const ch of q) {
		const at = t.indexOf(ch, from);
		if (at < 0) return null;
		score += 1;
		if (at === prev + 1) score += 3;
		if (at === 0 || !/[a-z0-9]/.test(t[at - 1])) score += 2;
		prev = at;
		from = at + 1;
	}
	return score;
}

function itemScore<T extends Pickable>(q: string, item: T): number | null {
	let best: number | null = null;
	const consider = (s: number | null, w: number) => {
		if (s !== null && (best === null || s * w > best)) best = s * w;
	};
	consider(fuzzyScore(q, item.name), FIELD_WEIGHT.name);
	consider(fuzzyScore(q, item.title), FIELD_WEIGHT.title);
	for (const tag of item.tags) consider(fuzzyScore(q, tag), FIELD_WEIGHT.tag);
	return best;
}

const byTitle = <T extends Pickable>(a: T, b: T) =>
	a.title.localeCompare(b.title, undefined, { sensitivity: 'base' }) || a.name.localeCompare(b.name);

export function fuzzyRank<T extends Pickable>(query: string, items: readonly T[]): T[] {
	const q = query.toLowerCase().replace(/\s+/g, '');
	if (!q) return [...items].sort(byTitle);
	return items
		.map((item) => ({ item, score: itemScore(q, item) }))
		.filter((r): r is { item: T; score: number } => r.score !== null)
		.sort((a, b) => b.score - a.score || byTitle(a.item, b.item))
		.map((r) => r.item);
}

export const keyOf = (item: Pickable, by: PickBy) => (by === 'id' ? item.id : item.name);

/** A prompt template is a single slot: picking one drops the other. */
export function togglePick(
	value: readonly string[],
	item: Pickable,
	items: readonly Pickable[],
	by: PickBy = 'name'
): string[] {
	const key = keyOf(item, by);
	if (value.includes(key)) return value.filter((k) => k !== key);
	if (item.kind !== 'prompt') return [...value, key];
	const prompts = new Set(items.filter((i) => i.kind === 'prompt').map((i) => keyOf(i, by)));
	return [...value.filter((k) => !prompts.has(k)), key];
}

export function dropLast(value: readonly string[], locked: readonly string[] = []): string[] {
	for (let i = value.length - 1; i >= 0; i--) {
		if (!locked.includes(value[i])) return [...value.slice(0, i), ...value.slice(i + 1)];
	}
	return [...value];
}

export function chipsOf<T extends Pickable>(value: readonly string[], items: readonly T[], by: PickBy = 'name'): T[] {
	return value.flatMap((k) => items.find((i) => keyOf(i, by) === k) ?? []);
}

export interface Precheck {
	picks: string[];
	/** Names checked because they matched, not because the user picked them. */
	auto: string[];
	/** Matched names the user unchecked: not re-checked while they keep matching. */
	dismissed: string[];
}

export function reconcilePrecheck(state: Precheck, matched: readonly string[]): Precheck {
	const stale = state.auto.filter((n) => !matched.includes(n));
	const picks = state.picks.filter((n) => !stale.includes(n));
	const auto = state.auto.filter((n) => matched.includes(n));
	for (const n of matched) {
		if (state.dismissed.includes(n) || picks.includes(n)) continue;
		auto.push(n);
		picks.push(n);
	}
	return { picks, auto, dismissed: state.dismissed.filter((n) => matched.includes(n)) };
}

export function notePicks(state: Precheck, picks: readonly string[]): Precheck {
	const removed = state.auto.filter((n) => !picks.includes(n));
	return {
		picks: [...picks],
		auto: state.auto.filter((n) => picks.includes(n)),
		dismissed: [...new Set([...state.dismissed, ...removed])]
	};
}

/**
 * `auto` makes the server add every scope-matched memory, so it may only stay
 * on while every match is still checked; otherwise an unchecked one comes back.
 */
export function contextAuto(picks: readonly string[], matched: readonly string[]): boolean {
	return matched.every((n) => picks.includes(n));
}
