import type { LaunchPreset } from '@bindings/LaunchPreset';
import type { Webhook } from '@bindings/Webhook';
import type { WebhookRequest } from '@bindings/WebhookRequest';
import { DEFAULT_TEMPLATE } from './template';

export const RATE_MIN = 1;
export const RATE_MAX = 600;
export const RATE_DEFAULT = 30;
export const WINDOW_MIN = 1;
export const WINDOW_MAX = 720;
export const WINDOW_DEFAULT = 24;
export const USD_MAX = 100_000;

export interface WebhookForm {
	name: string;
	preset_id: string;
	prompt_template: string;
	rate_per_minute: string;
	max_usd: string;
	budget_window_hours: string;
	enabled: boolean;
}

export interface WebhookFormProblems {
	name?: 'required';
	preset?: 'required';
	template?: 'required';
	rate?: 'range';
	budget?: 'range';
	window?: 'range';
}

export function requestFromWebhook(hook: Webhook): WebhookRequest {
	return {
		preset_id: hook.preset_id,
		name: hook.name,
		prompt_template: hook.prompt_template,
		rate_per_minute: hook.rate_per_minute,
		max_usd: hook.max_usd,
		budget_window_hours: hook.budget_window_hours,
		enabled: hook.enabled
	};
}

export const withEnabled = (hook: Webhook, enabled: boolean): WebhookRequest => ({
	...requestFromWebhook(hook),
	enabled
});

export function emptyForm(presetId = ''): WebhookForm {
	return {
		name: '',
		preset_id: presetId,
		prompt_template: DEFAULT_TEMPLATE,
		rate_per_minute: String(RATE_DEFAULT),
		max_usd: '',
		budget_window_hours: String(WINDOW_DEFAULT),
		enabled: true
	};
}

export function formFromWebhook(hook: Webhook): WebhookForm {
	return {
		name: hook.name,
		preset_id: hook.preset_id,
		prompt_template: hook.prompt_template,
		rate_per_minute: String(hook.rate_per_minute),
		max_usd: hook.max_usd === null ? '' : String(hook.max_usd),
		budget_window_hours: String(hook.budget_window_hours),
		enabled: hook.enabled
	};
}

export function parseRate(raw: string | number): number | null {
	const text = String(raw).trim();
	if (!/^\d+$/.test(text)) return null;
	const n = Number(text);
	return n >= RATE_MIN && n <= RATE_MAX ? n : null;
}

export function parseWindow(raw: string | number): number | null {
	const text = String(raw).trim();
	if (!/^\d+$/.test(text)) return null;
	const n = Number(text);
	return n >= WINDOW_MIN && n <= WINDOW_MAX ? n : null;
}

/** `null` is no cap; `undefined` is not a valid amount. */
export function parseUsd(raw: string | number): number | null | undefined {
	const text = String(raw).trim();
	if (!text) return null;
	if (!/^\d+(\.\d{1,2})?$/.test(text)) return undefined;
	const n = Number(text);
	return n > 0 && n <= USD_MAX ? n : undefined;
}

export function formProblems(form: WebhookForm): WebhookFormProblems {
	const out: WebhookFormProblems = {};
	if (!form.name.trim()) out.name = 'required';
	if (!form.preset_id) out.preset = 'required';
	if (!form.prompt_template.trim()) out.template = 'required';
	if (parseRate(form.rate_per_minute) === null) out.rate = 'range';
	if (parseUsd(form.max_usd) === undefined) out.budget = 'range';
	if (parseWindow(form.budget_window_hours) === null) out.window = 'range';
	return out;
}

export const hasProblems = (problems: WebhookFormProblems): boolean => Object.keys(problems).length > 0;

export function requestFromForm(form: WebhookForm): WebhookRequest {
	return {
		preset_id: form.preset_id,
		name: form.name.trim(),
		prompt_template: form.prompt_template,
		rate_per_minute: parseRate(form.rate_per_minute),
		max_usd: parseUsd(form.max_usd) ?? null,
		budget_window_hours: parseWindow(form.budget_window_hours),
		enabled: form.enabled
	};
}

export function deliveryUrl(apiBase: string, webhookId: string): string {
	return `${apiBase.replace(/\/+$/, '')}/hooks/${encodeURIComponent(webhookId)}`;
}

export function presetChoices<P extends Pick<LaunchPreset, 'external_sources' | 'name'>>(
	presets: readonly P[]
): P[] {
	return [...presets].sort(
		(a, b) => Number(b.external_sources) - Number(a.external_sources) || a.name.localeCompare(b.name)
	);
}

export function hookUrl(hook: Pick<Webhook, 'id' | 'url'>, apiBase: string): string {
	return /^https?:\/\//i.test(hook.url) ? hook.url : deliveryUrl(apiBase, hook.id);
}

export const SIGNATURE_HEADER = 'X-Hub-Signature-256';
