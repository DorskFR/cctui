/** A `/tiles` link pre-loaded with a session set. Ids are comma-joined, which
 *  the tiles view reads back as its initial workspace. */
export function tilesHref(ids: string[]): string {
	const clean = [...new Set(ids.filter(Boolean))];
	return clean.length ? `/tiles?s=${clean.join(',')}` : '/tiles';
}
