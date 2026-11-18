import type { Bucket } from '@bindings/Bucket';
import type { OutcomeStatus } from '@bindings/OutcomeStatus';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionOutcome } from '@bindings/SessionOutcome';

/** An idle session is Completed unless its newest outcome asks for the user:
 *  a declared need for input, or a failure. */
export function outcomeBucket(s: SessionListItem): Bucket {
	const bucket = s.bucket ?? 'working';
	if (bucket !== 'done') return bucket;
	switch (s.latest_outcome?.status) {
		case 'needs_input':
		case 'failed':
			return 'blocked';
		default:
			return 'done';
	}
}

const GLYPH: Record<OutcomeStatus, string> = {
	result: '✓',
	needs_input: '✋',
	failed: '✗',
	ended_without_outcome: '⚠'
};

/** `null` for an inferred outcome: its stock headline says less than the last message. */
export function outcomeHeadline(o: SessionOutcome | null | undefined): string | null {
	if (!o || o.source === 'inferred') return null;
	return `${GLYPH[o.status]} ${o.headline}`;
}

export function outcomeTone(status: OutcomeStatus): 'ok' | 'warn' | 'danger' {
	switch (status) {
		case 'result':
			return 'ok';
		case 'failed':
			return 'danger';
		default:
			return 'warn';
	}
}

/** Sessions whose `result` outcome is not in `seen` yet; records them as seen. */
export function freshResults(seen: Set<string>, sessions: SessionListItem[]): SessionListItem[] {
	const fresh: SessionListItem[] = [];
	for (const s of sessions) {
		const o = s.latest_outcome;
		if (o?.status !== 'result' || seen.has(o.id)) continue;
		seen.add(o.id);
		fresh.push(s);
	}
	return fresh;
}
