// XSS guard: Svelte `href` bindings and `window.open` honour `javascript:`, so
// only `http(s)` may reach those sinks; anything else returns `undefined`.
export function safeHref(url: string | null | undefined): string | undefined {
	if (!url) return undefined;
	try {
		const { protocol } = new URL(url);
		return protocol === 'http:' || protocol === 'https:' ? url : undefined;
	} catch {
		return undefined;
	}
}
