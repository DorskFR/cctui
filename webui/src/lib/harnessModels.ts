// Picker composition over the option lists the server derives
// (`GET /models/{harness}`). No model rules live here: the server has no
// allowlist either, and every picker also accepts a free-text id.
import type { ModelHint } from '@bindings/ModelHint';
import type { ModelOption } from '@bindings/ModelOption';
import { m as msg } from '$lib/paraglide/messages';

export type { ModelOption };

// Select sentinel for the free-text "Other model…" entry; never a real id.
export const OTHER_MODEL = '\u0000other';

/** Shown while the server list is still in flight, and for a harness with
 *  nothing to offer but the harness's own default. */
export const DEFAULT_MODELS: ModelOption[] = [{ v: '', label: 'Default', disabled: false }];

export function modelHintText(hint: ModelHint | undefined): string | undefined {
	if (!hint) return undefined;
	return hint.kind === 'gated'
		? msg.codex_model_gated({ version: hint.version, current: hint.current })
		: msg.codex_model_needs_version({ version: hint.version });
}

// The models a provider declares, in the order the operator listed them; a row
// with no id is a half-filled editor row and is dropped.
export function declaredModelOptions(
	models: { model: string; label: string }[] | null | undefined
): ModelOption[] {
	return (models ?? [])
		.filter((mo) => mo.model.trim())
		.map((mo) => ({ v: mo.model.trim(), label: mo.label.trim() || mo.model.trim(), disabled: false }));
}

// Declared models first, then whatever the server list adds that they don't
// already cover, so an operator's curated set leads the picker without hiding
// the rest.
export function withDeclaredModels(
	models: { model: string; label: string }[] | null | undefined,
	fallback: ModelOption[]
): ModelOption[] {
	const declared = declaredModelOptions(models);
	if (!declared.length) return fallback;
	const seen = new Set(declared.map((o) => o.v));
	return [
		...(fallback.some((o) => o.v === '') ? [{ v: '', label: 'Default', disabled: false }] : []),
		...declared,
		...fallback.filter((o) => o.v && !seen.has(o.v))
	];
}

// Reads a free-text model id: whitespace-trimmed, empty meaning "Default".
export function customModelValue(text: string): string {
	return text.trim();
}

// Keeps a value the option list doesn't know (a free-text or remembered id)
// selectable by listing it as its own option.
export function withCurrentModel(options: ModelOption[], current: string): ModelOption[] {
	if (!current || options.some((o) => o.v === current)) return options;
	return [...options, { v: current, label: current, disabled: false }];
}
