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
