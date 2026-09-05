// Harness model + effort option lists. The server has no model allowlist;
// these strings pass through verbatim, and every picker accepts a free-text id.
import type { CodexModelCatalog } from '@bindings/CodexModelCatalog';

// Select sentinel for the free-text "Other model…" entry; never a real id.
export const OTHER_MODEL = '\u0000other';

export interface ModelOption {
	v: string;
	label: string;
}

// Static offline fallback for codex, used only when no live `model/list`
// catalog is known (daemon offline, older daemon, codex missing). Kept short on
// purpose: the live catalog is the source of truth and free text covers the rest.
export const codexModels: ModelOption[] = [
	{ v: '', label: 'Default' },
	{ v: 'gpt-6-astra', label: 'GPT-6-Astra' },
	{ v: 'gpt-5.6-sol', label: 'GPT-5.6-Sol' },
	{ v: 'gpt-5.6-terra', label: 'GPT-5.6-Terra' },
	{ v: 'gpt-5.6-luna', label: 'GPT-5.6-Luna' },
	{ v: 'gpt-5.5', label: 'GPT-5.5' }
];
export const codexEfforts = ['', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra'];

// Model options from a machine's live catalog, hidden models dropped and
// superseded ones (with an `upgrade`) suffixed. `Default` stays first so an
// unset model keeps codex's own pick. Empty/absent catalog → the static list.
export function codexModelsFor(catalog: CodexModelCatalog | undefined): ModelOption[] {
	const models = catalog?.models ?? [];
	if (!models.length) return codexModels;
	const options: ModelOption[] = [{ v: '', label: 'Default' }];
	for (const m of models) {
		if (m.hidden) continue;
		const label = m.upgrade ? `${m.display_name} (superseded)` : m.display_name;
		options.push({ v: m.id, label });
	}
	return options;
}

// Effort levels a given model supports, `''` (default) first so the
// picker can leave codex its own default. An unknown model or empty catalog
// falls back to the full static effort list.
export function codexEffortsFor(
	catalog: CodexModelCatalog | undefined,
	modelId: string
): string[] {
	const models = catalog?.models ?? [];
	if (!models.length) return codexEfforts;
	const model = modelId ? models.find((m) => m.id === modelId) : models.find((m) => m.is_default);
	const supported = model?.supported_efforts ?? [];
	if (!supported.length) return codexEfforts;
	return ['', ...supported];
}

export const claudeModels: ModelOption[] = [
	{ v: '', label: 'Default' },
	{ v: 'haiku', label: 'Haiku' },
	{ v: 'sonnet', label: 'Sonnet' },
	{ v: 'opus', label: 'Opus' },
	{ v: 'fable', label: 'Fable' }
];
export const claudeEfforts = ['', 'low', 'medium', 'high', 'xhigh', 'max'];

// Reads a free-text model id: whitespace-trimmed, empty meaning "Default".
export function customModelValue(text: string): string {
	return text.trim();
}

// Keeps a value the option list doesn't know (a free-text or remembered id)
// selectable by listing it as its own option.
export function withCurrentModel(options: ModelOption[], current: string): ModelOption[] {
	if (!current || options.some((o) => o.v === current)) return options;
	return [...options, { v: current, label: current }];
}

// The live catalog to drive a codex picker with: the machine's own when it
// has one, else the cross-machine merge, else nothing (static fallback).
export function preferCatalog(
	...catalogs: (CodexModelCatalog | undefined)[]
): CodexModelCatalog | undefined {
	return catalogs.find((c) => c?.models.length);
}
