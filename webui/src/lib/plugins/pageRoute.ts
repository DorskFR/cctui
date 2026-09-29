import type { IconName } from '@dorsk/tsumikit';
import { DEFAULT_PLUGIN_ICON, enabledPagePlugins, isPluginId } from './discovery';
import type { PluginInfo } from './types';

export const APPS_BASE = '/apps';

export function pageBasePath(id: string): string {
	return `${APPS_BASE}/${id}`;
}

/** The plugin-relative path for a host URL, always rooted at `/`. A trailing
 *  slash is dropped so a plugin router only ever sees one form of `/`. */
export function pluginPath(basePath: string, pathname: string): string {
	if (!pathname.startsWith(basePath)) return '/';
	const rest = pathname.slice(basePath.length);
	if (rest === '' || rest === '/') return '/';
	const withSlash = rest.startsWith('/') ? rest : `/${rest}`;
	return withSlash.length > 1 && withSlash.endsWith('/') ? withSlash.slice(0, -1) : withSlash;
}

/** The host URL a plugin's `navigate(path)` means. Query and hash ride along;
 *  an absolute URL or a protocol-relative one is refused (a plugin may not
 *  navigate the host off its own base). */
export function hostHref(basePath: string, path: string): string {
	if (/^[a-z][a-z0-9+.-]*:/i.test(path) || path.startsWith('//')) return basePath;
	const rooted = path.startsWith('/') ? path : `/${path}`;
	const href = `${basePath}${rooted}`;
	return href.endsWith('/') && href.length > basePath.length ? href.slice(0, -1) : href;
}

export type PageState =
	| { status: 'invalid' }
	| { status: 'unknown' }
	| { status: 'not-enabled'; info: PluginInfo }
	| { status: 'no-page'; info: PluginInfo }
	| { status: 'loading'; info: PluginInfo }
	| { status: 'failed'; info: PluginInfo; error: string }
	| { status: 'ready'; info: PluginInfo };

/** What `/apps/<id>` should render, given the plugin list and the user's
 *  switches. `moduleState` reports the loader's view of the bundle. */
export function resolvePageState(args: {
	id: string;
	list: readonly PluginInfo[];
	enabled: Record<string, boolean>;
	moduleState: (info: PluginInfo) => { status: 'loading' | 'ready' | 'failed'; error?: string } | null;
}): PageState {
	if (!isPluginId(args.id)) return { status: 'invalid' };
	const info = args.list.find((p) => p.id === args.id);
	if (!info) return { status: 'unknown' };
	if (!info.page) return { status: 'no-page', info };
	if (args.enabled[info.id] !== true || !info.web) return { status: 'not-enabled', info };
	const mod = args.moduleState(info);
	if (!mod || mod.status === 'loading') return { status: 'loading', info };
	if (mod.status === 'failed') return { status: 'failed', info, error: mod.error ?? 'load failed' };
	return { status: 'ready', info };
}

export interface PageNavItem {
	href: string;
	label: string;
	iconName: IconName;
}

export function pageNavItems(list: readonly PluginInfo[], enabled: Record<string, boolean>): PageNavItem[] {
	return enabledPagePlugins(list, enabled).map((p) => ({
		href: pageBasePath(p.id),
		label: p.page?.title || p.name,
		iconName: (p.page?.icon || p.icon || DEFAULT_PLUGIN_ICON) as IconName
	}));
}
