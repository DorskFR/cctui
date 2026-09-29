/** Is the host's Svelte / Tsumikit new enough for what this plugin was built
 *  against? A plugin shares the host's runtime, so a mismatch is a broken pane,
 *  not a slow one — check it at build time (or on mount) rather than guessing. */
import type { PluginRuntimeManifest } from './types';

export interface PeerRanges {
	svelte?: string;
	tsumikit?: string;
	cctuiApi?: number;
}

export interface PeerMismatch {
	what: 'svelte' | 'tsumikit' | 'cctuiApi';
	wanted: string;
	found: string;
}

type Parts = [number, number, number];

function parse(version: string): Parts | null {
	const m = /^(\d+)\.(\d+)\.(\d+)/.exec(version.trim());
	return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

function compare(a: Parts, b: Parts): number {
	for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
	return 0;
}

/** A deliberately small subset of the npm range grammar: an exact version,
 *  `^x.y.z`, `~x.y.z`, `>=x.y.z`, and space-separated conjunctions of those
 *  (`>=0.63.1 <1.0.0`). Anything it cannot parse is treated as unsatisfied, so
 *  a typo fails loudly instead of passing silently. */
export function satisfies(version: string, range: string): boolean {
	const found = parse(version);
	if (!found) return false;
	const terms = range.trim().split(/\s+/).filter(Boolean);
	if (terms.length === 0) return false;
	for (const term of terms) {
		if (term === '*') continue;
		const op = /^(\^|~|>=|<=|>|<|=)?\s*(.+)$/.exec(term);
		const wanted = op?.[2] ? parse(op[2]) : null;
		if (!wanted) return false;
		const cmp = compare(found, wanted);
		switch (op?.[1]) {
			case '^': {
				// A 0.y.z release treats the minor as the breaking unit.
				const upper: Parts = wanted[0] === 0 ? [0, wanted[1] + 1, 0] : [wanted[0] + 1, 0, 0];
				if (cmp < 0 || compare(found, upper) >= 0) return false;
				break;
			}
			case '~':
				if (cmp < 0 || compare(found, [wanted[0], wanted[1] + 1, 0]) >= 0) return false;
				break;
			case '>=':
				if (cmp < 0) return false;
				break;
			case '<=':
				if (cmp > 0) return false;
				break;
			case '>':
				if (cmp <= 0) return false;
				break;
			case '<':
				if (cmp >= 0) return false;
				break;
			default:
				if (cmp !== 0) return false;
		}
	}
	return true;
}

/** Everything in `ranges` the host does not satisfy; empty means compatible. */
export function peerMismatches(manifest: PluginRuntimeManifest, ranges: PeerRanges): PeerMismatch[] {
	const out: PeerMismatch[] = [];
	if (ranges.cctuiApi !== undefined && manifest.cctuiApi !== ranges.cctuiApi) {
		out.push({ what: 'cctuiApi', wanted: String(ranges.cctuiApi), found: String(manifest.cctuiApi) });
	}
	if (ranges.svelte && !satisfies(manifest.svelte, ranges.svelte)) {
		out.push({ what: 'svelte', wanted: ranges.svelte, found: manifest.svelte });
	}
	if (ranges.tsumikit && !satisfies(manifest.tsumikit, ranges.tsumikit)) {
		out.push({ what: 'tsumikit', wanted: ranges.tsumikit, found: manifest.tsumikit });
	}
	return out;
}

export function describeMismatches(list: readonly PeerMismatch[]): string {
	return list.map((x) => `${x.what} ${x.found} does not satisfy ${x.wanted}`).join('; ');
}

export interface PeerCheckOptions extends PeerRanges {
	/** Where the host serves its runtime manifest; only change it in tests. */
	manifestUrl?: string;
	fetchImpl?: typeof fetch;
}

/** Read `/plugin-runtime/manifest.json` and report what does not line up.
 *  `null` means the manifest could not be read — an older host, or an offline
 *  one — which is not itself a mismatch. */
export async function checkHostPeers(opts: PeerCheckOptions = {}): Promise<PeerMismatch[] | null> {
	const url = opts.manifestUrl ?? '/plugin-runtime/manifest.json';
	const get = opts.fetchImpl ?? globalThis.fetch;
	try {
		const res = await get(url);
		if (!res.ok) return null;
		return peerMismatches((await res.json()) as PluginRuntimeManifest, opts);
	} catch {
		return null;
	}
}
