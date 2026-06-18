// Pure helpers for the accounts page — no Svelte/reactive state, so they live
// outside the component and are unit-testable on their own.
import { compact } from '$lib/format';

/** Display name for an account provider id (anthropic → Claude, openai → Codex,
 *  plus the compatible-endpoint variants — CCT-399). */
export const providerLabel = (p: string) =>
	p === 'anthropic'
		? 'Claude'
		: p === 'openai'
			? 'Codex'
			: p === 'anthropic-compatible'
				? 'Anthropic-compatible'
				: p === 'openai-compatible'
					? 'OpenAI-compatible'
					: p;

// Estimated cost (CCT-273): sub-cent → "<$0.01", small → 2 dp, large → compact.
export const usd = (v: number) =>
	!v ? '$0' : v < 0.01 ? '<$0.01' : v < 1000 ? `$${v.toFixed(2)}` : `$${compact(v)}`;
