import type { PluginInfo } from './types';

/** Where a `styles[]` entry is served from. This server resolves them already
 *  (`/plugins/<id>/<path>?v=<sha8>`), which passes through untouched; a relative
 *  entry from an older or third-party server is resolved against the plugin's
 *  own `web/` base and inherits its cache-buster. */
export function styleUrls(info: PluginInfo): string[] {
	const styles = info.styles ?? [];
	if (styles.length === 0) return [];
	const web = info.web ?? '';
	const [webPath, webQuery] = web.split('?');
	const base = webPath.slice(0, webPath.lastIndexOf('/') + 1) || `/plugins/${info.id}/`;
	const out: string[] = [];
	for (const entry of styles) {
		if (!entry || entry.includes('..') || /^[a-z][a-z0-9+.-]*:/i.test(entry) || entry.startsWith('//')) continue;
		if (entry.startsWith('/')) {
			out.push(entry);
			continue;
		}
		const href = new URL(entry, `https://h${base}`).pathname;
		out.push(webQuery ? `${href}?${webQuery}` : href);
	}
	return out;
}

export function ensurePluginStyles(info: PluginInfo, doc: Document = document): void {
	for (const href of styleUrls(info)) {
		if (doc.querySelector(`link[rel="stylesheet"][href="${href}"]`)) continue;
		const link = doc.createElement('link');
		link.rel = 'stylesheet';
		link.href = href;
		link.dataset.plugin = info.id;
		doc.head.append(link);
	}
}
