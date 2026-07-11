// Single source of truth for harness model + effort option lists (CCT-626).
// The server has no model allowlist; these strings pass through verbatim.
import type { CodexModelCatalog } from '@bindings/CodexModelCatalog';

export interface ModelOption {
	v: string;
	label: string;
}

// Static offline fallback for codex (CCT-641). Used when the machine has no
// live `model/list` catalog cached (daemon offline, older daemon, codex
// missing). A machine-scoped catalog supersedes it via codexModelsFor/…Efforts.
export const codexModels: ModelOption[] = [
	{ v: '', label: 'Default' },
	{ v: 'gpt-5.6-sol', label: 'GPT-5.6 Sol' },
	{ v: 'gpt-5.6-terra', label: 'GPT-5.6 Terra' },
	{ v: 'gpt-5.6-luna', label: 'GPT-5.6 Luna' },
	{ v: 'gpt-5.5', label: 'GPT-5.5' },
	{ v: 'gpt-5.4', label: 'GPT-5.4' },
	{ v: 'gpt-5.4-mini', label: 'GPT-5.4 Mini' }
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

// Effort levels a given model supports (CCT-641), `''` (default) first so the
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
