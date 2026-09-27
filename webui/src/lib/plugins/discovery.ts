import type { IconName } from '@dorsk/tsumikit';
import { CCTUI_PLUGIN_API, type CctuiPluginModule, type PluginInfo } from './types';

const PLUGIN_ID = /^[a-z0-9-]{1,40}$/;

export function isPluginId(id: unknown): id is string {
	return typeof id === 'string' && PLUGIN_ID.test(id);
}

export const DEFAULT_PLUGIN_ICON: IconName = 'grid';

/** Server-reported plugins the user switched on and that ship a web bundle. */
export function enabledWebPlugins(list: readonly PluginInfo[], enabled: Record<string, boolean>): PluginInfo[] {
	return list.filter((p) => p.web && enabled[p.id] === true);
}

export class PluginModuleError extends Error {}

/** The default export of a plugin's `web/index.js`, checked against the
 *  contract: refuses a missing export, another API major, or contributions
 *  of the wrong shape. */
export function validatePluginModule(loaded: unknown): CctuiPluginModule {
	const mod = (loaded as { default?: unknown } | null)?.default;
	if (!mod || typeof mod !== 'object') throw new PluginModuleError('plugin module has no default export');
	const m = mod as Record<string, unknown>;
	if (m.cctuiApi !== CCTUI_PLUGIN_API) {
		throw new PluginModuleError(`plugin targets cctuiApi ${String(m.cctuiApi)}, host is ${CCTUI_PLUGIN_API}`);
	}
	if (m.sessionPane !== undefined && typeof m.sessionPane !== 'function') {
		throw new PluginModuleError('sessionPane is not a component');
	}
	if (m.messageActions !== undefined && typeof m.messageActions !== 'function') {
		throw new PluginModuleError('messageActions is not a function');
	}
	return m as unknown as CctuiPluginModule;
}

const cache = new Map<string, Promise<CctuiPluginModule>>();

/** `import()` a plugin bundle once per URL (the `?v=` cache-buster makes a
 *  rescan a new URL). A failed load is not cached so a reload can retry. */
export function loadPluginModule(web: string, importer = defaultImporter): Promise<CctuiPluginModule> {
	const hit = cache.get(web);
	if (hit) return hit;
	const p = importer(web).then(validatePluginModule);
	cache.set(web, p);
	p.catch(() => cache.delete(web));
	return p;
}

export function resetPluginModuleCache() {
	cache.clear();
}

function defaultImporter(url: string): Promise<unknown> {
	return import(/* @vite-ignore */ url);
}
