import type { IconName } from '@dorsk/tsumikit';
import type { Line } from '$lib/components/organisms/conversation/types';
import { DEFAULT_PLUGIN_ICON, enabledWebPlugins } from './discovery';
import { type PluginLoader, pluginLoader } from './loader.svelte';
import { type ActionSource, AutoOpenOnce, collectMessageActions } from './messageActions';
import type { PluginActionButton, PluginButton, PluginInfo } from './types';

export interface OpenPane {
	id: string;
	params: Record<string, string>;
}

/** The runtime plugins one conversation drawer shows: which are enabled and
 *  loaded, which pane is open (and with what params), the buttons on
 *  assistant lines, and the once-only auto-open a plugin may request. */
export class DrawerPlugins {
	open = $state<OpenPane | null>(null);
	private readonly auto = new AutoOpenOnce();

	constructor(
		private readonly deps: {
			sessionId: () => string;
			installed: () => readonly PluginInfo[];
			enabled: () => Record<string, boolean>;
		},
		private readonly loader: PluginLoader = pluginLoader
	) {}

	get enabledList(): PluginInfo[] {
		return enabledWebPlugins(this.deps.installed(), this.deps.enabled());
	}

	/** Start importing every enabled bundle; call from an effect. */
	ensureLoaded() {
		for (const p of this.enabledList) this.loader.ensure(p);
	}

	get ready(): ActionSource[] {
		return this.loader.ready(this.enabledList);
	}

	get panes(): ActionSource[] {
		return this.ready.filter((r) => !!r.module.sessionPane);
	}

	get current(): ActionSource | null {
		const id = this.open?.id;
		return id ? (this.panes.find((p) => p.info.id === id) ?? null) : null;
	}

	get buttons(): PluginButton[] {
		return this.panes.map(({ info }) => ({
			id: info.id,
			label: info.name,
			icon: (info.icon || DEFAULT_PLUGIN_ICON) as IconName,
			open: this.open?.id === info.id,
			onselect: () => this.toggle(info.id)
		}));
	}

	toggle(id: string) {
		this.open = this.open?.id === id ? null : { id, params: {} };
	}

	openWith(pluginId: string, params: Record<string, string>) {
		this.open = { id: pluginId, params };
	}

	close() {
		this.open = null;
	}

	actionsFor(ln: Line): PluginActionButton[] {
		if (ln.role !== 'assistant' || !ln.text) return [];
		return collectMessageActions(this.ready, { role: ln.role, text: ln.text });
	}

	/** Open the pane a plugin asked for on the newest assistant line, once. */
	autoOpenFrom(lines: readonly Line[]) {
		if (this.open) return;
		const newest = [...lines].reverse().find((l) => l.role === 'assistant' && !!l.text);
		if (!newest) return;
		const hit = this.auto.take(this.deps.sessionId(), this.actionsFor(newest));
		if (hit) this.openWith(hit.pluginId, hit.params);
	}
}
