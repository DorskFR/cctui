import { loadPluginModule } from './discovery';
import type { CctuiPluginModule, PluginInfo } from './types';

export type LoadedPlugin =
	| { info: PluginInfo; status: 'loading' }
	| { info: PluginInfo; status: 'ready'; module: CctuiPluginModule }
	| { info: PluginInfo; status: 'failed'; error: string };

/** Reactive view over the plugin bundles the page has imported, keyed by
 *  the bundle URL. Components read `entries`; `ensure` kicks off a load. */
export class PluginLoader {
	entries = $state<Record<string, LoadedPlugin>>({});

	constructor(private readonly load: (web: string) => Promise<CctuiPluginModule> = loadPluginModule) {}

	ensure(info: PluginInfo): LoadedPlugin | null {
		if (!info.web) return null;
		const key = info.web;
		const hit = this.entries[key];
		if (hit) return hit;
		this.entries[key] = { info, status: 'loading' };
		this.load(key).then(
			(module) => {
				this.entries[key] = { info, status: 'ready', module };
			},
			(e: unknown) => {
				this.entries[key] = { info, status: 'failed', error: e instanceof Error ? e.message : String(e) };
			}
		);
		return this.entries[key];
	}

	/** Loaded modules for the given plugins, in list order; failed and
	 *  in-flight ones are skipped. */
	ready(list: readonly PluginInfo[]): { info: PluginInfo; module: CctuiPluginModule }[] {
		const out: { info: PluginInfo; module: CctuiPluginModule }[] = [];
		for (const info of list) {
			const e = info.web ? this.entries[info.web] : undefined;
			if (e?.status === 'ready') out.push({ info, module: e.module });
		}
		return out;
	}
}

export const pluginLoader = new PluginLoader();
