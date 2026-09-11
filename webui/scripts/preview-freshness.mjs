// A reused `vite preview` can answer with markup whose assets it will not
// serve — from an older build, or from another worktree holding the port. The
// capture that follows is a blank page that looks like a real UI defect.
const ASSET_RE = /\/_app\/immutable\/[^"'\s>]+?\.(?:js|css)/g;

export function referencedAssets(html) {
	return [...new Set(html.match(ASSET_RE) ?? [])];
}

/** Resolves to `null` when the URL is safe to shoot against — either nothing is
 *  listening (the harness starts its own server) or every asset the shell asks
 *  for is served. Otherwise resolves to the reason it is not. */
export async function previewStaleness(url) {
	let html;
	try {
		const res = await fetch(url, { redirect: 'follow' });
		if (!res.ok) return { url, reason: `the server answered ${res.status}`, missing: [] };
		html = await res.text();
	} catch {
		return null;
	}

	const assets = referencedAssets(html);
	if (!assets.length) {
		return { url, reason: 'the served page references no app assets', missing: [] };
	}

	// Every build rehashes the entry chunks, so they alone settle the question.
	const entries = assets.filter((a) => a.includes('/entry/'));
	const probe = entries.length ? entries : assets.slice(0, 10);

	const missing = [];
	for (const asset of probe) {
		const res = await fetch(new URL(asset, url), { method: 'GET' }).catch(() => null);
		if (!res?.ok) missing.push(`${asset} -> ${res ? res.status : 'no response'}`);
	}
	return missing.length
		? { url, reason: 'it will not serve the assets its own HTML references', missing }
		: null;
}
