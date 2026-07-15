const MIN = 60 * 1000;

export const ANTHROPIC_TTL_MS = 60 * MIN;
export const OPENAI_GPT56_TTL_MS = 30 * MIN;
export const DEFAULT_TTL_MS = 5 * MIN;

function isAnthropic(adapterId: string | null): boolean {
	return !!adapterId && adapterId.toLowerCase().includes("claude");
}

function isOpenai(adapterId: string | null): boolean {
	if (!adapterId) return false;
	const a = adapterId.toLowerCase();
	return a.includes("codex") || a.includes("openai");
}

/**
 * Whether an OpenAI model string is GPT-5.6 or later (the 30-minute default
 * prompt-cache TTL). Parses the first `gpt-<major>.<minor>` token, so
 * `gpt-5.6`, `gpt-5.6-codex`, `gpt-6` all qualify while `gpt-5.5` / `gpt-4.1`
 * do not.
 */
export function isGpt56OrLater(model: string | null): boolean {
	if (!model) return false;
	const m = model.toLowerCase().match(/gpt-(\d+)(?:\.(\d+))?/);
	if (!m) return false;
	const major = Number(m[1]);
	const minor = m[2] ? Number(m[2]) : 0;
	return major > 5 || (major === 5 && minor >= 6);
}

/**
 * The prompt-cache TTL window for a session, from its provider family
 * (`adapterId`) and `model`. Anthropic → 60m, OpenAI GPT-5.6+ → 30m, else 5m.
 */
export function cacheTtlMs(adapterId: string | null, model: string | null): number {
	if (isAnthropic(adapterId)) return ANTHROPIC_TTL_MS;
	if (isOpenai(adapterId) && isGpt56OrLater(model)) return OPENAI_GPT56_TTL_MS;
	return DEFAULT_TTL_MS;
}
