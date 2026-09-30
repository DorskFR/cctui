// Candidate filtering for the pairwise peer-share picker. Pure, so the rules
// (never offer yourself, never offer an existing grant, cap the list) are
// testable without mounting the menu.

export interface ShareCandidate {
	id: string;
	name: string | null;
	adapter: string | null;
	machine: string | null;
}

/** Rows past this are dropped: the picker is a search box, not a session list. */
export const SHARE_CANDIDATE_CAP = 12;

/** `name (adapter on machine)`, falling back to the id. */
export function shareLabel(c: ShareCandidate): string {
	const name = c.name?.trim() || c.id;
	return `${name} (${c.adapter ?? 'unknown'} on ${c.machine ?? 'unknown machine'})`;
}

/**
 * Sessions worth offering a grant to: not the subject, not already shared, and
 * matching `query` against the name, the id, the adapter or the machine. An
 * empty query still lists (capped), so the picker is usable without typing.
 */
export function shareCandidates(
	all: ShareCandidate[],
	subject: string,
	shared: ReadonlySet<string>,
	query: string
): ShareCandidate[] {
	const want = query.trim().toLowerCase();
	return all
		.filter((c) => c.id !== subject && !shared.has(c.id))
		.filter((c) => !want || shareLabel(c).toLowerCase().includes(want) || c.id.includes(want))
		.slice(0, SHARE_CANDIDATE_CAP);
}
