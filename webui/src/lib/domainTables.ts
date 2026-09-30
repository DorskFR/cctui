// The closed domain tables, as constants: end-reason tones, provider metadata
// and the per-harness static model lists. `fixtures/parity/domainTables.json`
// is the source of truth both these and the Rust derivations are checked
// against, so the rules answer synchronously — no server round-trip, and the
// same answer in a test, on first paint and in the TUI.
//
// Only genuinely dynamic data stays on the wire: the codex model catalog
// (`GET /models/{harness}`) and the server's quota-probe registry.
import type { EndReasonInfo } from '@bindings/EndReasonInfo';
import type { HarnessModels } from '@bindings/HarnessModels';
import type { PermissionMode } from '@bindings/PermissionMode';
import type { ProviderInfo } from '@bindings/ProviderInfo';
import type { SessionEndReason } from '@bindings/SessionEndReason';

export const END_REASONS: EndReasonInfo[] = [
	{ reason: 'completed', tone: 'ok', muted: false, failed_start: false },
	{ reason: 'killed', tone: 'neutral', muted: false, failed_start: false },
	{ reason: 'crashed', tone: 'danger', muted: false, failed_start: false },
	{ reason: 'daemon_lost', tone: 'warn', muted: false, failed_start: false },
	{ reason: 'machine_offline', tone: 'warn', muted: false, failed_start: false },
	{ reason: 'reaped_inactive', tone: 'neutral', muted: true, failed_start: false },
	{ reason: 'resume_failed', tone: 'danger', muted: false, failed_start: true },
	{ reason: 'spawn_failed', tone: 'danger', muted: false, failed_start: true },
	{ reason: 'other', tone: 'neutral', muted: false, failed_start: false }
];

export const PROVIDERS: ProviderInfo[] = [
	{
		id: 'anthropic',
		label: 'Claude',
		picker_label: 'Claude (anthropic)',
		family: 'anthropic',
		static_credential: false
	},
	{
		id: 'openai',
		label: 'Codex',
		picker_label: 'Codex (openai)',
		family: 'openai',
		static_credential: false
	},
	{
		id: 'anthropic-compatible',
		label: 'Anthropic-compatible',
		picker_label: 'Anthropic-compatible endpoint',
		family: 'anthropic',
		static_credential: true
	},
	{
		id: 'openai-compatible',
		label: 'OpenAI-compatible',
		picker_label: 'OpenAI-compatible endpoint',
		family: 'openai',
		static_credential: true
	},
	{
		id: 'fireworks',
		label: 'Fireworks',
		picker_label: 'Fireworks',
		family: 'fireworks',
		static_credential: true
	}
];

export const HARNESS_MODELS: HarnessModels[] = [
	{
		harness: 'claude-code',
		models: [
			{ v: '', label: 'Default', disabled: false },
			{ v: 'haiku', label: 'Haiku', disabled: false },
			{ v: 'sonnet', label: 'Sonnet', disabled: false },
			{ v: 'opus', label: 'Opus', disabled: false },
			{ v: 'fable', label: 'Fable', disabled: false }
		],
		efforts: ['', 'low', 'medium', 'high', 'xhigh', 'max']
	},
	{
		harness: 'codex',
		models: [{ v: '', label: 'Default', disabled: false }],
		efforts: ['', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra']
	},
	{
		harness: 'opencode',
		models: [{ v: '', label: 'Default', disabled: false }],
		efforts: []
	}
];

export const PERMISSION_MODES: PermissionMode[] = ['ask', 'auto', 'yolo', 'whip'];

export function endReasonInfo(reason: SessionEndReason): EndReasonInfo | undefined {
	return END_REASONS.find((r) => r.reason === reason);
}

export function providerInfo(id: string): ProviderInfo | undefined {
	return PROVIDERS.find((p) => p.id === id);
}

/** The lists a picker starts from; an unknown harness gets the claude shape,
 *  which is also what a free-text picker needs. */
export function harnessModelsFallback(harness: string): HarnessModels {
	return HARNESS_MODELS.find((h) => h.harness === harness) ?? HARNESS_MODELS[0];
}
