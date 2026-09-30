import type { YouTrackSlot } from './sessionSlots';

/** Resolve an issue id to the slot cctui stores for it. The YouTrack connector
 *  installs one; until then none is installed and nothing is looked up. Must
 *  resolve to `null` rather than throw when the issue is unknown. */
export type YouTrackLookup = (issue: string) => Promise<YouTrackSlot | null>;

let installed: YouTrackLookup | null = null;

/** The single seam the connector wires itself into. Pass `null` to remove it
 *  when the connector is deconfigured. */
export function setYouTrackLookup(lookup: YouTrackLookup | null): void {
	installed = lookup;
}

export function hasYouTrackLookup(): boolean {
	return installed !== null;
}

/** The connector's answer, or `{ issue }` alone when there is no connector.
 *  Never rejects: a failed lookup degrades to the bare id, which is exactly
 *  what the chip renders when no summary or state is known. */
export async function lookupYouTrackIssue(issue: string): Promise<YouTrackSlot> {
	if (!installed) return { issue };
	try {
		return (await installed(issue)) ?? { issue };
	} catch (e) {
		console.warn('youtrack lookup failed', e);
		return { issue };
	}
}
