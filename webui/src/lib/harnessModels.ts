// Single source of truth for harness model + effort option lists (CCT-626).
// The server has no model allowlist; these strings pass through verbatim.
export interface ModelOption {
	v: string;
	label: string;
}

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

export const claudeModels: ModelOption[] = [
	{ v: '', label: 'Default' },
	{ v: 'haiku', label: 'Haiku' },
	{ v: 'sonnet', label: 'Sonnet' },
	{ v: 'opus', label: 'Opus' },
	{ v: 'fable', label: 'Fable' }
];
export const claudeEfforts = ['', 'low', 'medium', 'high', 'xhigh', 'max'];
