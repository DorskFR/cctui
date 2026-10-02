import type { AgentEvent } from '@bindings/AgentEvent';

export type ConversationPageFetch = (
	id: string,
	q: { limit: number; before: number }
) => Promise<AgentEvent[]>;

const EXPORT_PAGE_LIMIT = 500;

// The drawer holds only the pages scrolled into view; an export walks the
// rest back to the first event. A page shorter than the limit is the head.
export async function loadWholeConversation(
	id: string,
	loaded: AgentEvent[],
	fetchPage: ConversationPageFetch,
	onprogress?: (count: number) => void
): Promise<AgentEvent[]> {
	const older: AgentEvent[][] = [];
	let count = loaded.length;
	let before = loaded.find((e) => typeof e.seq === 'number')?.seq;
	while (before != null) {
		const cursor = before;
		const page = await fetchPage(id, { limit: EXPORT_PAGE_LIMIT, before: cursor });
		const sequenced = page.filter((e) => typeof e.seq === 'number' && e.seq < cursor);
		if (sequenced.length) older.unshift(page);
		count += page.length;
		if (page.length < EXPORT_PAGE_LIMIT || !sequenced.length) break;
		before = Math.min(...sequenced.map((e) => e.seq as number));
		onprogress?.(count);
	}
	if (!older.length) return loaded;
	const seen = new Set<number>();
	const out: AgentEvent[] = [];
	for (const e of [...older.flat(), ...loaded]) {
		if (typeof e.seq === 'number') {
			if (seen.has(e.seq)) continue;
			seen.add(e.seq);
		}
		out.push(e);
	}
	return out;
}
