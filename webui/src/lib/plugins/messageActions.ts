import type { IconName } from '@dorsk/tsumikit';
import { DEFAULT_PLUGIN_ICON } from './discovery';
import type { CctuiPluginModule, PluginActionButton, PluginInfo, PluginMessage } from './types';

export interface ActionSource {
	info: PluginInfo;
	module: CctuiPluginModule;
}

/** Every action the ready plugins contribute for one assistant message. A
 *  plugin that throws contributes nothing rather than breaking the line. */
export function collectMessageActions(sources: readonly ActionSource[], msg: PluginMessage): PluginActionButton[] {
	if (msg.role !== 'assistant' || !msg.text) return [];
	const out: PluginActionButton[] = [];
	for (const { info, module } of sources) {
		if (!module.messageActions || !module.sessionPane) continue;
		let actions: unknown;
		try {
			actions = module.messageActions(msg);
		} catch (e) {
			console.warn(`plugin ${info.id} messageActions threw`, e);
			continue;
		}
		if (!Array.isArray(actions)) continue;
		for (const a of actions) {
			if (!a || typeof a !== 'object' || a.open !== 'sessionPane' || typeof a.label !== 'string') continue;
			out.push({
				pluginId: info.id,
				label: a.label,
				icon: (typeof a.icon === 'string' ? a.icon : info.icon || DEFAULT_PLUGIN_ICON) as IconName,
				params: cleanParams(a.params),
				autoOpen: a.autoOpen === true
			});
		}
	}
	return out;
}

function cleanParams(v: unknown): Record<string, string> {
	const out: Record<string, string> = {};
	if (!v || typeof v !== 'object') return out;
	for (const [k, val] of Object.entries(v as Record<string, unknown>)) if (typeof val === 'string') out[k] = val;
	return out;
}

export function autoOpenKey(sessionId: string, pluginId: string, params: Record<string, string>): string {
	const sorted = Object.keys(params)
		.sort()
		.map((k) => [k, params[k]]);
	return `${sessionId}\u0000${pluginId}\u0000${JSON.stringify(sorted)}`;
}

/** Honours a plugin's `autoOpen` at most once per (session, plugin, params)
 *  for the lifetime of the page, so closing the pane stays closed. */
export class AutoOpenOnce {
	private readonly seen = new Set<string>();

	/** The first `autoOpen` action of `actions` not yet honoured, or null. */
	take(sessionId: string, actions: readonly PluginActionButton[]): PluginActionButton | null {
		for (const a of actions) {
			if (!a.autoOpen) continue;
			const key = autoOpenKey(sessionId, a.pluginId, a.params);
			if (this.seen.has(key)) continue;
			this.seen.add(key);
			return a;
		}
		return null;
	}
}
