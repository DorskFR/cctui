import type { SessionListItem } from '@bindings/SessionListItem';
import type { ThreadSession } from '@bindings/ThreadSession';
import { previousSessionId } from '$lib/followup';

export interface ThreadEntry {
	id: string;
	/** Unique within the thread: the menu keys its rows by label. */
	label: string;
	title: string;
	at: string | null;
	current: boolean;
	/** Tree glyphs leading the row; empty until the thread forks. */
	tree: string;
}

type ThreadSource = Pick<
	SessionListItem,
	'id' | 'name' | 'working_dir' | 'parent_id' | 'metadata' | 'next_session_id' | 'registered_at'
>;

/** Glyphs per entry of a depth-first thread. A lone follow-up continues in
 *  its parent's column; siblings branch off it. */
function treeGlyphs(thread: readonly ThreadSession[]): string[] {
	const ids = new Set(thread.map((t) => t.id));
	const parentOf = (t: ThreadSession) =>
		t.parent_id && ids.has(t.parent_id) ? t.parent_id : null;
	const children = new Map<string | null, string[]>();
	for (const t of thread) {
		const p = parentOf(t);
		children.set(p, [...(children.get(p) ?? []), t.id]);
	}
	const below = new Map<string, string>();
	return thread.map((t) => {
		const p = parentOf(t);
		const base = p === null ? '' : (below.get(p) ?? '');
		const siblings = children.get(p) ?? [];
		if (p === null || siblings.length < 2) {
			below.set(t.id, base);
			return base;
		}
		const last = siblings[siblings.length - 1] === t.id;
		below.set(t.id, base + (last ? '   ' : '│  '));
		return base + (last ? '└─ ' : '├─ ');
	});
}

function uniqueLabels(entries: Omit<ThreadEntry, 'label'>[]): ThreadEntry[] {
	const seen = new Set<string>();
	return entries.map((e, i) => {
		let label = e.title;
		if (seen.has(label)) label = `${e.title} (${i + 1})`;
		seen.add(label);
		return { ...e, label };
	});
}

/** `fallback` names the session's own links until the fetched thread lands. */
export function threadEntries(
	session: ThreadSource,
	thread: readonly ThreadSession[] | undefined,
	fallback: { previous: string; next: string }
): ThreadEntry[] {
	if (thread && thread.length > 1 && thread.some((t) => t.id === session.id)) {
		const tree = treeGlyphs(thread);
		return uniqueLabels(
			thread.map((t, i) => ({
				id: t.id,
				title: t.name || t.working_dir,
				at: t.registered_at,
				current: t.id === session.id,
				tree: tree[i]
			}))
		);
	}
	const previous = previousSessionId(session);
	const next = session.next_session_id ?? null;
	if (!previous && !next) return [];
	return uniqueLabels([
		...(previous
			? [{ id: previous, title: fallback.previous, at: null, current: false, tree: '' }]
			: []),
		{
			id: session.id,
			title: session.name || session.working_dir,
			at: session.registered_at ?? null,
			current: true,
			tree: ''
		},
		...(next ? [{ id: next, title: fallback.next, at: null, current: false, tree: '' }] : [])
	]);
}
