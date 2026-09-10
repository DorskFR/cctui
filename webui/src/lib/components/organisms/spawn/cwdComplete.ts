import { endpoints } from '$lib/queries';

/** Recent dirs while empty; otherwise a listing of the typed path's parent, so
 *  one request fires per directory level rather than per keystroke. */
export async function cwdSuggestions(
	machineId: string,
	query: string,
	recentDirs: string[]
): Promise<string[]> {
	const out: string[] = [];
	const seen = new Set<string>();
	const push = (v: string) => {
		if (v && !seen.has(v)) {
			seen.add(v);
			out.push(v);
		}
	};
	if (!query) for (const d of recentDirs) push(d);
	if (!machineId) return out;
	const i = query.lastIndexOf('/');
	if (i < 0) return out;
	const parent = i === 0 ? '/' : query.slice(0, i);
	const prefix = query.slice(i + 1).toLowerCase();
	try {
		const { dirs } = await endpoints.machineDirs(machineId, parent);
		const showHidden = prefix.startsWith('.');
		for (const d of dirs) {
			if (!showHidden && d.startsWith('.')) continue;
			if (!d.toLowerCase().startsWith(prefix)) continue;
			push(`${parent === '/' ? '' : parent}/${d}`);
			if (out.length >= 50) break;
		}
	} catch {
		/* transient FS error shouldn't break typing */
	}
	return out;
}
