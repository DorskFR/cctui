// XSS guard: Svelte `href` bindings and `window.open` honour `javascript:`, so
// only `http(s)` absolute URLs and same-origin relative paths (`/api/…`, never
// `//host` or `file:`) may reach those sinks; anything else returns `undefined`.
export function safeHref(url: string | null | undefined): string | undefined {
	if (!url) return undefined;
	if (url.startsWith('/')) return url.startsWith('//') ? undefined : url;
	try {
		const { protocol } = new URL(url);
		return protocol === 'http:' || protocol === 'https:' ? url : undefined;
	} catch {
		return undefined;
	}
}
