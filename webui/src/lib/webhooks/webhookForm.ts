import type { LaunchPreset } from '@bindings/LaunchPreset';
import type { Webhook } from '@bindings/Webhook';
import type { WebhookRequest } from '@bindings/WebhookRequest';
import { DEFAULT_TEMPLATE } from './template';

export const RATE_MIN = 1;
export const RATE_MAX = 600;
export const RATE_DEFAULT = 30;

export interface WebhookForm {
	name: string;
	preset_id: string;
	prompt_template: string;
	rate_per_minute: string;
	enabled: boolean;
}

export interface WebhookFormProblems {
	name?: 'required';
	preset?: 'required';
	template?: 'required';
	rate?: 'range';
}

export function requestFromWebhook(hook: Webhook): WebhookRequest {
	return {
		preset_id: hook.preset_id,
		name: hook.name,
		prompt_template: hook.prompt_template,
		rate_per_minute: hook.rate_per_minute,
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
		enabled: true
	};
}

export function formFromWebhook(hook: Webhook): WebhookForm {
	return {
		name: hook.name,
		preset_id: hook.preset_id,
		prompt_template: hook.prompt_template,
		rate_per_minute: String(hook.rate_per_minute),
		enabled: hook.enabled
	};
}

export function parseRate(raw: string | number): number | null {
	const text = String(raw).trim();
	if (!/^\d+$/.test(text)) return null;
	const n = Number(text);
	return n >= RATE_MIN && n <= RATE_MAX ? n : null;
}

export function formProblems(form: WebhookForm): WebhookFormProblems {
	const out: WebhookFormProblems = {};
	if (!form.name.trim()) out.name = 'required';
	if (!form.preset_id) out.preset = 'required';
	if (!form.prompt_template.trim()) out.template = 'required';
	if (parseRate(form.rate_per_minute) === null) out.rate = 'range';
	return out;
}

export const hasProblems = (problems: WebhookFormProblems): boolean => Object.keys(problems).length > 0;

export function requestFromForm(form: WebhookForm): WebhookRequest {
	return {
		preset_id: form.preset_id,
		name: form.name.trim(),
		prompt_template: form.prompt_template,
		rate_per_minute: parseRate(form.rate_per_minute),
		enabled: form.enabled
	};
}

export function deliveryUrl(apiBase: string, webhookId: string): string {
	return `${apiBase.replace(/\/+$/, '')}/hooks/${encodeURIComponent(webhookId)}`;
}

export function eligiblePresets<P extends Pick<LaunchPreset, 'external_sources' | 'name'>>(
	presets: readonly P[]
): P[] {
	return presets.filter((p) => p.external_sources).sort((a, b) => a.name.localeCompare(b.name));
}

export const SIGNATURE_HEADER = 'X-Hub-Signature-256';
