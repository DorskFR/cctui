/** A tracker issue id: `PROJECT-123`. Anchored on non-word boundaries so
 *  `refs/ABC-1` matches but `xABC-1` and `ABC-1a` do not. */
const ISSUE_ID = /(^|[^A-Za-z0-9_])([A-Z]+-\d+)(?![A-Za-z0-9_])/;

export function extractIssueId(text: string | null | undefined): string | null {
	if (!text) return null;
	return ISSUE_ID.exec(text)?.[2] ?? null;
}

/** The first issue id found across `sources`, in order. Callers pass the spawn
 *  prompt, then the session name, then the git branch: the prompt is the most
 *  deliberate statement of what the session is for. */
export function detectIssueId(...sources: (string | null | undefined)[]): string | null {
	for (const source of sources) {
		const found = extractIssueId(source);
		if (found) return found;
	}
	return null;
}

/** Accepts either a bare id or a YouTrack issue URL, so pasting a link works.
 *  Returns the id and, when one was pasted, the url to link the chip to. */
export function parseIssueEntry(input: string): { issue: string; url?: string } | null {
	const raw = input.trim();
	if (!raw) return null;
	if (/^https?:\/\//i.test(raw)) {
		let url: URL;
		try {
			url = new URL(raw);
		} catch {
			return null;
		}
		const issue = extractIssueId(decodeURIComponent(url.pathname));
		return issue ? { issue, url: url.toString() } : null;
	}
	const issue = extractIssueId(raw.toUpperCase());
	return issue ? { issue } : null;
}
