import type { SessionListItem } from '@bindings/SessionListItem';
import { branchOf } from '../../routes/sessions/sessions.logic';
import { detectIssueId, parseIssueEntry } from './issueId';
import { lookupYouTrackIssue } from './youtrackLookup';

export const YOUTRACK_PLUGIN_ID = 'youtrack';

/** The slot to store for a typed id or a pasted issue URL, or `null` when the
 *  input carries no issue id at all. Summary and state come from the connector
 *  when one is installed; otherwise the slot is the bare id. */
export async function resolveIssueSlot(
	input: string
): Promise<Record<string, unknown> | null> {
	const parsed = parseIssueEntry(input);
	if (!parsed) return null;
	const slot = await lookupYouTrackIssue(parsed.issue);
	return { ...slot, ...(parsed.url ? { url: parsed.url } : {}) };
}

/** An issue id this session looks like it is about: the spawn prompt (only
 *  drafts keep one on the row), else the session name, else the git branch. */
export function detectSessionIssueId(session: SessionListItem): string | null {
	const prompt = (session.metadata as { draft?: { prompt?: unknown } } | null)?.draft?.prompt;
	return detectIssueId(
		typeof prompt === 'string' ? prompt : null,
		session.name,
		branchOf(session)
	);
}
