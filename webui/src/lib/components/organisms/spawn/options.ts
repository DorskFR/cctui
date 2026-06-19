// Shared spawn-form option lists, extracted from SpawnModal (no behavior
// change). Used by the machine + dispatch field groups for their model/effort
// selectors and permission-mode picker.
import type { PermissionMode } from '@bindings/PermissionMode';

export const modes: { v: PermissionMode; label: string; hint: string }[] = [
	{ v: 'ask', label: 'Ask', hint: 'Prompt on every action' },
	{ v: 'auto', label: 'Auto', hint: 'Auto-apply, sandbox on' },
	{ v: 'yolo', label: 'Yolo', hint: 'No prompts, full access' },
	{ v: 'whip', label: 'Whip 🐎', hint: 'Yolo + no asking, no stalling' }
];

// Per-adapter effort levels, index 0 = "" (adapter default). Claude and codex
// expose different reasoning-effort vocabularies.
export const claudeEfforts = ['', 'low', 'medium', 'high', 'xhigh', 'max'];
export const codexEfforts = ['', 'low', 'medium', 'high', 'xhigh'];

// Model families per adapter (CCT-274). `''` = the adapter's own default.
// claude resolves the family alias (opus/sonnet/haiku/fable) to a concrete
// model; codex takes the model slug directly.
export const claudeModels = [
	{ v: '', label: 'Default' },
	{ v: 'haiku', label: 'Haiku' },
	{ v: 'sonnet', label: 'Sonnet' },
	{ v: 'opus', label: 'Opus' },
	{ v: 'fable', label: 'Fable' }
];
export const codexModels = [
	{ v: '', label: 'Default' },
	{ v: 'gpt-5.5-codex', label: 'GPT-5.5 Codex' },
	{ v: 'gpt-5.4-codex', label: 'GPT-5.4 Codex' }
];

// Annotate native-family options with the per-account alias target (CCT-406)
// so the picker reads e.g. "Opus (claude-opus-4-8[1m])" instead of a bare
// "Opus" — making it obvious which concrete model the family resolves to (and
// that the alias is in effect) for the selected account. A no-op when the
// account has no matching alias for that family.
export const withAliasTargets = (
	models: { v: string; label: string }[],
	aliases: Record<string, string> | null | undefined
): { v: string; label: string }[] =>
	models.map((m) => {
		const target = m.v ? aliases?.[m.v]?.trim() : undefined;
		return target ? { v: m.v, label: `${m.label} (${target})` } : m;
	});

// The harness/adapter an account's provider locks to (CCT-399): anything in the
// openai family runs Codex; everything else (anthropic / anthropic-compatible)
// runs Claude Code. Mirrors the server's `Family::from_provider`.
export const adapterForProvider = (provider: string): 'claude-code' | 'codex' =>
	provider.includes('openai') ? 'codex' : 'claude-code';

// A compatible-endpoint account carries its own model list; a native
// subscription account uses the harness's native families.
export const isCompatibleProvider = (provider: string): boolean =>
	provider.endsWith('-compatible');

export const adapterLabel = (adapter: string): string =>
	adapter === 'codex' ? 'Codex' : 'Claude Code';
