import type { MenuItem } from '@dorsk/tsumikit';
import type { PluginButton } from '#lib/plugins/types.ts';
import { m } from '#lib/paraglide/messages.js';

export type TerminalPanel = { open: boolean; toggle: () => void };

/** Rows of the toolbar's Panels menu: the live terminal, then each plugin pane. */
export function panelItems(terminal: TerminalPanel | null, plugins: PluginButton[]): MenuItem[] {
	return [
		...(terminal
			? [
					{
						label: m.drawer_terminal_label(),
						icon: 'tv' as const,
						pressed: terminal.open,
						attrs: { title: m.conversation_terminal_title(), 'data-journey': 'terminal' },
						onselect: terminal.toggle
					}
				]
			: []),
		...plugins.map((p) => ({
			label: p.label,
			icon: p.icon,
			pressed: p.open,
			attrs: { title: m.drawer_plugin_toggle({ name: p.label }), 'data-journey': 'plugin', 'data-plugin': p.id },
			onselect: p.onselect
		}))
	];
}
