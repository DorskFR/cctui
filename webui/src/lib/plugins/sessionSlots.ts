import type { IconName } from '@dorsk/tsumikit';

/** One session's per-plugin data slots, as stored in
 *  `sessions.metadata.plugins.<plugin_id>`. */
export type PluginSlots = Record<string, Record<string, unknown>>;

export interface PluginChip {
	pluginId: string;
	label: string;
	/** Hover text; newline-separated lines. */
	title: string;
	href: string | null;
	icon: IconName;
}

export type PluginChipRenderer = (
	data: Record<string, unknown>
) => Omit<PluginChip, 'pluginId'> | null;

/** The YouTrack slot, the first renderer. Only `issue` is required: without a
 *  connector there is nothing to look the rest up with. */
export interface YouTrackSlot {
	issue: string;
	summary?: string;
	state?: string;
	url?: string;
}

const renderers = new Map<string, PluginChipRenderer>();

export function registerPluginChipRenderer(pluginId: string, render: PluginChipRenderer): void {
	renderers.set(pluginId, render);
}

export function pluginChipRenderer(pluginId: string): PluginChipRenderer | undefined {
	return renderers.get(pluginId);
}

export function registeredChipRenderers(): string[] {
	return [...renderers.keys()].sort();
}

function str(v: unknown): string | null {
	return typeof v === 'string' && v.trim() ? v.trim() : null;
}

export function youtrackChip(data: Record<string, unknown>): Omit<PluginChip, 'pluginId'> | null {
	const issue = str(data.issue);
	if (!issue) return null;
	const summary = str(data.summary);
	const state = str(data.state);
	const lines = [issue];
	if (summary) lines.push(summary);
	if (state) lines.push(state);
	return { label: issue, title: lines.join('\n'), href: str(data.url), icon: 'bookmark' };
}

registerPluginChipRenderer('youtrack', youtrackChip);

/** The slots under `metadata.plugins`, ignoring anything that is not an object
 *  so a hand-edited row can never break a card. */
export function readPluginSlots(metadata: unknown): PluginSlots {
	if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) return {};
	const raw = (metadata as Record<string, unknown>).plugins;
	if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {};
	const out: PluginSlots = {};
	for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
		if (value && typeof value === 'object' && !Array.isArray(value)) {
			out[id] = value as Record<string, unknown>;
		}
	}
	return out;
}

export function readPluginSlot(metadata: unknown, pluginId: string): Record<string, unknown> | null {
	return readPluginSlots(metadata)[pluginId] ?? null;
}

/** Chips for every slot that has a registered renderer. A slot with no
 *  renderer contributes nothing rather than a raw-JSON chip, and a renderer
 *  that throws is skipped. */
export function pluginChips(metadata: unknown): PluginChip[] {
	const out: PluginChip[] = [];
	for (const [pluginId, data] of Object.entries(readPluginSlots(metadata))) {
		const render = renderers.get(pluginId);
		if (!render) continue;
		let chip: ReturnType<PluginChipRenderer>;
		try {
			chip = render(data);
		} catch (e) {
			console.warn(`plugin chip renderer ${pluginId} threw`, e);
			continue;
		}
		if (chip) out.push({ pluginId, ...chip });
	}
	return out.sort((a, b) => a.pluginId.localeCompare(b.pluginId));
}
