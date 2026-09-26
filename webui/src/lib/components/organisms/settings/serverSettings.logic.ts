import type { SettingSource } from '@bindings/SettingSource';
import type { SpawnDefaults } from '@bindings/SpawnDefaults';
import { m } from '$lib/paraglide/messages';

export function sourceLabel(source: SettingSource): string {
	if (source === 'settings') return m.settings_source_settings();
	if (source === 'env') return m.settings_source_env();
	return m.settings_source_default();
}

/** Empty fields become `null`; `null` overall when any field is invalid. */
export function parseSpawnDraft(draft: Record<keyof SpawnDefaults, string>): SpawnDefaults | null {
	const int = (raw: string): number | null | undefined => {
		const s = raw.trim();
		if (!s) return null;
		const n = Number(s);
		return Number.isInteger(n) && n >= 1 && n <= 4294967295 ? n : undefined;
	};
	const budget = (raw: string): number | null | undefined => {
		const s = raw.trim();
		if (!s) return null;
		const n = Number(s);
		return Number.isFinite(n) && n >= 0 ? n : undefined;
	};
	const out = {
		max_children: int(draft.max_children),
		max_depth: int(draft.max_depth),
		max_tree_budget_usd: budget(draft.max_tree_budget_usd)
	};
	if (Object.values(out).some((v) => v === undefined)) return null;
	return out as SpawnDefaults;
}
